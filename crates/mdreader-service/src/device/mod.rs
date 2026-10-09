//! Same-owner, local-only device reading and bounded attachment leases.
//! This adapter never initializes a namespace or creates a cache directory.
mod lifecycle;
mod model;
mod search;
mod state;
pub use lifecycle::FrozenDevices;
use mdreader_core::{
    device::{assets::AssetRuntime, catalog},
    ReaderStore,
};
pub use model::*;
use model::{asset_error, document_error};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};
type Result<T> = std::result::Result<T, DeviceError>;

pub struct Coordinator {
    owner: Arc<Mutex<ReaderStore>>,
    queries: Arc<Mutex<ReaderStore>>,
    documents: PathBuf,
    assets: PathBuf,
    runtime: AssetRuntime,
    admission: Arc<lifecycle::Admission>,
    leases: Mutex<std::collections::HashSet<String>>,
}
impl Coordinator {
    pub fn new(
        owner: Arc<Mutex<ReaderStore>>,
        queries: Arc<Mutex<ReaderStore>>,
        documents: PathBuf,
        assets: PathBuf,
        runtime: AssetRuntime,
    ) -> Self {
        Self {
            owner,
            queries,
            documents,
            assets,
            runtime,
            admission: Arc::default(),
            leases: Mutex::default(),
        }
    }
    fn owner(&self) -> Result<MutexGuard<'_, ReaderStore>> {
        self.owner.try_lock().map_err(|error| match error {
            std::sync::TryLockError::WouldBlock => DeviceError::Busy,
            std::sync::TryLockError::Poisoned(_) => DeviceError::Persistence,
        })
    }
    fn check(&self, store: &ReaderStore, receipt: &catalog::Receipt) -> Result<()> {
        catalog::document_in(store, &self.documents, receipt).map_err(document_error)?;
        Ok(())
    }
    pub(crate) fn with_catalog<T>(
        &self,
        operation: impl FnOnce(&std::path::Path) -> T,
    ) -> Result<T> {
        let _admission = self.admission.begin()?;
        Ok(operation(&self.documents))
    }
    pub fn execute(&self, request: DeviceRequest) -> Result<DeviceReply> {
        request.validate()?;
        let _admission = if matches!(&request, DeviceRequest::Close { .. }) {
            None
        } else {
            Some(self.admission.begin()?)
        };
        match request {
            DeviceRequest::HistoryBindings {
                archive_id,
                archive_sha256,
            } => {
                let owner = self.queries.try_lock().map_err(|_| DeviceError::Busy)?;
                let bindings = mdreader_core::device::archive::history::bindings(
                    &owner,
                    &archive_id,
                    &archive_sha256,
                )
                .map_err(document_error)?;
                let digest = mdreader_core::device::archive::history::digest(&bindings)
                    .map_err(document_error)?;
                Ok(DeviceReply::HistoryBindings { bindings, digest })
            }
            DeviceRequest::BindHistory {
                archive_id,
                archive_sha256,
                entry_id,
                expected_digest,
                reference,
            } => {
                let mut owner = self.owner()?;
                mdreader_core::device::archive::history::bind(
                    &mut owner,
                    &archive_id,
                    &archive_sha256,
                    &entry_id,
                    &expected_digest,
                    &reference,
                )
                .map_err(document_error)?;
                let bindings = mdreader_core::device::archive::history::bindings(
                    &owner,
                    &archive_id,
                    &archive_sha256,
                )
                .map_err(document_error)?;
                let digest = mdreader_core::device::archive::history::digest(&bindings)
                    .map_err(document_error)?;
                Ok(DeviceReply::HistoryBindings { bindings, digest })
            }
            DeviceRequest::Resolve {
                receipt,
                kind,
                target,
            } => {
                let owner = self.queries.try_lock().map_err(|_| DeviceError::Busy)?;
                let found = if kind == "wikilink" {
                    vec![]
                } else {
                    mdreader_core::device::archive::resolve(
                        &owner,
                        &self.documents,
                        &receipt,
                        &kind,
                        &target,
                    )
                    .map_err(document_error)?
                };
                Ok(DeviceReply::ResolvedLinks {
                    references: found
                        .into_iter()
                        .map(|d| ResolvedReference {
                            document_id: d.id,
                            source_id: d.source_id,
                            revision: d.revision,
                            title: d.name,
                            heading: None,
                            line: 1,
                        })
                        .collect(),
                })
            }
            DeviceRequest::State { receipt } => self.state(receipt),
            DeviceRequest::SetState {
                receipt,
                expected_generation,
                pinned,
                line,
            } => self.set_state(receipt, expected_generation, pinned, line),
            DeviceRequest::Close { session_id } => {
                // Release permits even after source/schema/directory revocation.
                if self
                    .leases
                    .lock()
                    .map_err(|_| DeviceError::Persistence)?
                    .remove(&session_id)
                {
                    self.runtime.close(&session_id).map_err(asset_error)?;
                }
                Ok(DeviceReply::Complete)
            }
            DeviceRequest::Read { receipt } => {
                let query = self.queries.try_lock().map_err(|_| DeviceError::Busy)?;
                let body =
                    catalog::read_in(&query, &self.documents, &receipt).map_err(document_error)?;
                Ok(DeviceReply::ReadDocument {
                    receipt,
                    name: body.document.name,
                    markdown: body.markdown,
                    line: body.document.line,
                })
            }
            DeviceRequest::Manifest { receipt } => {
                let mut owner = self.owner()?;
                self.check(&owner, &receipt)?;
                let assets = self
                    .runtime
                    .existing_assets(&mut owner, &self.assets)
                    .map_err(asset_error)?;
                let value = assets
                    .map(|assets| {
                        assets.manifest(&mut owner, &receipt.document_id, &receipt.revision)
                    })
                    .transpose()
                    .map_err(asset_error)?;
                // Even absent attachment metadata cannot bypass exact body authority.
                catalog::read_in(&owner, &self.documents, &receipt).map_err(document_error)?;
                Ok(DeviceReply::Manifest { value })
            }
            DeviceRequest::Open {
                receipt,
                asset_id,
                manifest,
            } => {
                let permit = self
                    .runtime
                    .read_permit(Instant::now())
                    .map_err(asset_error)?;
                let (assets, prepared) = {
                    let mut owner = self.owner()?;
                    self.check(&owner, &receipt)?;
                    let assets = self
                        .runtime
                        .existing_assets(&mut owner, &self.assets)
                        .map_err(asset_error)?
                        .ok_or(DeviceError::Unavailable)?;
                    let prepared = assets
                        .prepare_open(
                            &mut owner,
                            &receipt.document_id,
                            &receipt.revision,
                            &asset_id,
                            &manifest,
                            permit,
                        )
                        .map_err(asset_error)?;
                    (assets, prepared)
                };
                // Hash verification runs with no SQLite/owner guard held.
                let verified = prepared.verify().map_err(asset_error)?;
                let mut owner = self.owner()?;
                self.check(&owner, &receipt)?;
                self.runtime
                    .existing_assets(&mut owner, &self.assets)
                    .map_err(asset_error)?
                    .ok_or(DeviceError::Unavailable)?;
                let (session_id, receipt) = self
                    .runtime
                    .install(&assets, &mut owner, verified, Instant::now())
                    .map_err(asset_error)?;
                let mut leases = match self.leases.lock() {
                    Ok(leases) => leases,
                    Err(_) => {
                        let _ = self.runtime.close(&session_id);
                        return Err(DeviceError::Persistence);
                    }
                };
                leases.retain(|id| self.runtime.selected_receipt(id, Instant::now()).is_ok());
                leases.insert(session_id.clone());
                Ok(DeviceReply::Opened {
                    session_id,
                    receipt,
                })
            }
            DeviceRequest::Chunk { session_id, offset } => {
                let result = (|| {
                    if !self
                        .leases
                        .lock()
                        .map_err(|_| DeviceError::Persistence)?
                        .contains(&session_id)
                    {
                        return Err(DeviceError::Unavailable);
                    }
                    let mut owner = self.owner()?;
                    let selected = self
                        .runtime
                        .selected_receipt(&session_id, Instant::now())
                        .map_err(asset_error)?;
                    let metadata =
                        catalog::document(&owner, &selected.document_id).map_err(document_error)?;
                    self.check(
                        &owner,
                        &catalog::Receipt {
                            document_id: selected.document_id,
                            source_id: metadata.source_id,
                            revision: selected.document_revision,
                        },
                    )?;
                    let assets = self
                        .runtime
                        .existing_assets(&mut owner, &self.assets)
                        .map_err(asset_error)?
                        .ok_or(DeviceError::Unavailable)?;
                    let chunk = self
                        .runtime
                        .chunk(&assets, &mut owner, &session_id, offset, Instant::now())
                        .map_err(asset_error)?;
                    Ok(DeviceReply::Chunk {
                        offset: chunk.offset,
                        base64: chunk.base64,
                        complete: chunk.complete,
                    })
                })();
                if result.is_err()
                    || matches!(&result, Ok(DeviceReply::Chunk { complete: true, .. }))
                {
                    self.leases
                        .lock()
                        .map_err(|_| DeviceError::Persistence)?
                        .remove(&session_id);
                    let _ = self.runtime.close(&session_id);
                }
                result
            }
        }
    }
}

#[cfg(test)]
mod tests;
