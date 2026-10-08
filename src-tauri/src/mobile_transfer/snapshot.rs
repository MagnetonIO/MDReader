//! Exact original and ready-resource snapshot, using the existing embedded owner
//! and its bounded read sessions. Hashing and ZIP writes never hold that owner.
use super::types::Reference;
use crate::{mobile_assets::AssetRuntime, reader::ReaderBackend};
use base64::{engine::general_purpose::STANDARD, Engine};
use mdreader_core::device::{
    assets::{AssetManifest, AssetStore},
    catalog,
};
use std::{
    io::Write,
    path::{Component, Path},
    time::Instant,
};

pub(super) struct Snapshot {
    pub markdown: String,
    pub name: String,
    pub assets: Option<AssetStore>,
    pub manifest: Option<AssetManifest>,
}
fn owner<T>(
    backend: &ReaderBackend,
    f: impl FnOnce(&mut mdreader_core::ReaderStore) -> Result<T, String>,
) -> Result<T, String> {
    backend.with_mobile_store(f).map_err(|_| "stale".into())
}
pub(super) fn prepare(
    backend: &ReaderBackend,
    runtime: &AssetRuntime,
    documents: &Path,
    images: &Path,
    reference: &Reference,
) -> Result<Snapshot, String> {
    let receipt = reference.receipt()?;
    owner(backend, |store| {
        let read = catalog::read_in(store, documents, &receipt).map_err(|_| "stale")?;
        let assets = runtime
            .existing_assets(store, images)
            .map_err(|_| "unavailable")?;
        let manifest = assets
            .as_ref()
            .map(|a| a.manifest(store, &receipt.document_id, &receipt.revision))
            .transpose()
            .map_err(|_| "stale")?;
        if manifest.as_ref().is_some_and(|m| {
            m.assets.len() > 8
                || m.assets.iter().map(|a| u64::from(a.bytes)).sum::<u64>() > 16 * 1024 * 1024
        }) {
            return Err("too_large".into());
        }
        Ok(Snapshot {
            markdown: read.markdown,
            name: read.document.name,
            assets,
            manifest,
        })
    })
}
pub(super) fn target(target: &str) -> Result<(), String> {
    if target.is_empty()
        || target.len() > 512
        || target.contains(['\\', ':'])
        || target.chars().any(char::is_control)
        || Path::new(target)
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
        || target
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err("unavailable".into());
    }
    Ok(())
}
struct Session<'a>(&'a AssetRuntime, String);
impl Drop for Session<'_> {
    fn drop(&mut self) {
        let _ = self.0.close(&self.1);
    }
}
pub(super) fn copy_asset(
    backend: &ReaderBackend,
    runtime: &AssetRuntime,
    snapshot: &Snapshot,
    reference: &Reference,
    id: &str,
    output: &mut impl Write,
) -> Result<(), String> {
    let assets = snapshot.assets.as_ref().ok_or("stale")?;
    let manifest = snapshot.manifest.as_ref().ok_or("stale")?;
    let permit = runtime.read_permit(Instant::now()).map_err(|_| "busy")?;
    let prepared = owner(backend, |store| {
        assets
            .prepare_open(
                store,
                &reference.document_id,
                &reference.revision,
                id,
                &manifest.manifest,
                permit,
            )
            .map_err(|_| "stale".into())
    })?;
    let verified = prepared.verify().map_err(|_| "stale")?;
    let (session, receipt) = owner(backend, |store| {
        runtime
            .install(assets, store, verified, Instant::now())
            .map_err(|_| "stale".into())
    })?;
    let session = Session(runtime, session);
    let mut offset = 0;
    loop {
        let chunk = owner(backend, |store| {
            runtime
                .chunk(assets, store, &session.1, offset, Instant::now())
                .map_err(|_| "stale".into())
        })?;
        let bytes = STANDARD.decode(chunk.base64).map_err(|_| "unavailable")?;
        if chunk.offset != offset || bytes.is_empty() || bytes.len() > 65536 {
            return Err("unavailable".into());
        }
        offset = offset.checked_add(bytes.len() as u32).ok_or("too_large")?;
        if offset > receipt.bytes {
            return Err("unavailable".into());
        }
        output
            .write_all(&bytes)
            .map_err(|_| "transfer_unconfirmed")?;
        if chunk.complete {
            return if offset == receipt.bytes {
                Ok(())
            } else {
                Err("stale".into())
            };
        }
    }
}
pub(super) fn revalidate(
    backend: &ReaderBackend,
    runtime: &AssetRuntime,
    documents: &Path,
    images: &Path,
    reference: &Reference,
    snapshot: &Snapshot,
) -> Result<(), String> {
    let current = prepare(backend, runtime, documents, images, reference)?;
    if current.markdown != snapshot.markdown
        || current.manifest.as_ref().map(|m| &m.manifest)
            != snapshot.manifest.as_ref().map(|m| &m.manifest)
    {
        return Err("stale".into());
    }
    Ok(())
}
