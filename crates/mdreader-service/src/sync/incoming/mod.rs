//! Native owner composition for authenticated, contiguous NEW incoming content.
//! No RPC/plaintext/authentication bypass, profile discovery, network or renderer DTO.
//! Construct one coordinator per live owner; reopening requires a fresh coordinator.
use mdreader_core::{
    device::{assets::AssetRuntime, publication},
    ReaderStore,
};
use mdreader_sync::{
    authority::{NativeAuthority, ProjectionGuard, ProjectionPermit},
    incoming::PreparedIncoming,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, RwLock, Weak},
};
use uuid::Uuid;

mod identity;
mod ledger;
mod schema;
mod storage;
mod types;
pub use types::*;

/// Minted only by selecting a real native authority on this exact live owner.
/// Neither Clone/Debug nor serializable; does not keep the authority alive.
pub struct SelectedIncoming {
    permit: ProjectionPermit,
    token: Uuid,
    coordinator: Weak<()>,
}
pub struct IncomingCoordinator {
    owner: Arc<Mutex<ReaderStore>>,
    documents: PathBuf,
    assets: PathBuf,
    runtime: AssetRuntime,
    operation: Mutex<()>,
    selection: RwLock<Option<Uuid>>,
    identity: Arc<()>,
    remote_library: Uuid,
}
impl IncomingCoordinator {
    /// Explicit owner/directories only. Does not activate a scope or read any profiles.
    pub fn new(
        owner: Arc<Mutex<ReaderStore>>,
        remote_library: Uuid,
        documents: PathBuf,
        assets: PathBuf,
    ) -> Result<Self> {
        {
            let mut owner = owner.try_lock().map_err(|_| Error::Busy)?;
            crate::sync::initialize(&mut owner)?;
            owner.with_library_connection(|db| -> Result<()> {
                let tx = db.transaction()?;
                schema::bind(&tx, remote_library)?;
                tx.commit()?;
                Ok(())
            })?;
        }
        Ok(Self {
            owner,
            documents,
            assets,
            runtime: AssetRuntime::default(),
            operation: Mutex::new(()),
            selection: RwLock::new(None),
            identity: Arc::new(()),
            remote_library,
        })
    }
    /// Native selection rotates even if account/library identifiers are unchanged.
    /// Hold the actual NativeAuthority elsewhere for the selected lifetime.
    pub fn select(&self, authority: &NativeAuthority) -> Result<SelectedIncoming> {
        let permit = authority.projection_permit()?;
        if permit.scope().library_id() != self.remote_library.to_string() {
            return Err(Error::Scope);
        }
        let token = Uuid::new_v4();
        permit.with_current(|_| -> Result<()> {
            *self.selection.write().map_err(|_| Error::Inactive)? = Some(token);
            Ok(())
        })??;
        Ok(SelectedIncoming {
            permit,
            token,
            coordinator: Arc::downgrade(&self.identity),
        })
    }
    pub fn clear_selection(&self) -> Result<()> {
        *self.selection.write().map_err(|_| Error::Inactive)? = None;
        Ok(())
    }
    fn short<T>(
        &self,
        selected: &SelectedIncoming,
        action: impl FnOnce(&mut ReaderStore, &ProjectionGuard<'_>) -> Result<T>,
    ) -> Result<T> {
        if !selected
            .coordinator
            .upgrade()
            .is_some_and(|id| Arc::ptr_eq(&id, &self.identity))
        {
            return Err(Error::Scope);
        }
        selected.permit.with_current(|session| {
            let selection = self.selection.read().map_err(|_| Error::Inactive)?;
            if *selection != Some(selected.token) {
                return Err(Error::Inactive);
            }
            let mut owner = self.owner.try_lock().map_err(|_| Error::Busy)?;
            owner.with_library_connection(|db| -> Result<()> {
                let tx = db.transaction()?;
                schema::require_library(&tx, &self.remote_library.to_string())?;
                tx.commit()?;
                Ok(())
            })?;
            session.check()?;
            action(&mut owner, session)
        })?
    }
    /// Synchronous blocking-worker API: never run on the async/UI executor.
    /// Operation lease spans reserve/files/decode/commit. No cleanup API is exposed:
    /// outstanding old-session reservations require explicit future discard/reauthorization.
    pub fn apply(&self, selected: &SelectedIncoming, input: &PreparedIncoming) -> Result<Outcome> {
        let _operation = self.operation.try_lock().map_err(|_| Error::Busy)?;
        if input.scope() != selected.permit.scope() {
            return Err(Error::Scope);
        }
        if let Some(reason) = identity::unsupported(input) {
            return self.short(selected, |owner, _| {
                Ok(Outcome::Pending(
                    storage::classify(owner, input.scope(), input.change())?.unwrap_or(reason),
                ))
            });
        }
        // Hashing/copy allocation is outside owner and authority/selection barriers.
        let identity = identity::Identity::from_prepared(input)?;
        let document = identity::document(input)?;
        let origin = identity::origin(
            input.scope(),
            input,
            selected.permit.session_token(),
            selected.token,
        )?;
        let start = self.short(selected, |owner, _| {
            storage::start(
                owner,
                input.scope(),
                &identity,
                &origin,
                &document,
                storage::Materialization {
                    documents: &self.documents,
                    assets: &self.assets,
                    runtime: &self.runtime,
                },
            )
        })?;
        match start {
            storage::Start::Pending(reason) => Ok(Outcome::Pending(reason)),
            storage::Start::Applied(receipt) => self.short(selected, |owner, _| {
                let index_queued = storage::queue_index(owner, &receipt);
                Ok(Outcome::Applied {
                    receipt,
                    replayed: true,
                    index_queued,
                })
            }),
            storage::Start::Plan(plan) => {
                let ready = plan.write(document)?.validate(
                    self.runtime
                        .validation_permit()
                        .map_err(publication::PublicationError::from)?,
                )?;
                self.short(selected, |owner, session| {
                    let receipt =
                        storage::commit(owner, input.scope(), &identity, &origin, &ready, |_| {
                            session.check()?;
                            Ok(())
                        })?;
                    let index_queued = storage::queue_index(owner, &receipt);
                    Ok(Outcome::Applied {
                        receipt: Box::new(receipt),
                        replayed: false,
                        index_queued,
                    })
                })
            }
        }
    }
    /// Resolve an uncertain commit using exact verified content; never reserve or regrant.
    /// A new valid selection may inspect an old durable receipt, without rebinding its origin.
    pub fn reconcile(
        &self,
        selected: &SelectedIncoming,
        input: &PreparedIncoming,
    ) -> Result<Option<Outcome>> {
        let _operation = self.operation.try_lock().map_err(|_| Error::Busy)?;
        if input.scope() != selected.permit.scope() {
            return Err(Error::Scope);
        }
        let identity = identity::Identity::from_prepared(input)?;
        let origin = identity::origin(
            input.scope(),
            input,
            selected.permit.session_token(),
            selected.token,
        )?;
        self.short(selected, |owner, _| {
            Ok(
                storage::reconcile(owner, input.scope(), &identity, &origin)?.map(|receipt| {
                    let index_queued = storage::queue_index(owner, &receipt);
                    Outcome::Applied {
                        receipt,
                        replayed: true,
                        index_queued,
                    }
                }),
            )
        })
    }
}

#[cfg(all(test, unix))]
mod tests;
