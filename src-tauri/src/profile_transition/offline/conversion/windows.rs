//! Exact selected Windows copies. Canonical attachment bytes use the same live
//! device owner and bounded read permits; original grants never enter the copy.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use device::assets::{AssetManifest, AssetReceipt, AssetRuntime, AssetStore};
use sha2::{Digest, Sha256};
use std::{ffi::OsString, os::windows::ffi::OsStringExt, time::Instant};
use uuid::Uuid;

pub(in crate::profile_transition::offline) fn owned_document(
    store: &mut ReaderStore,
    profile: &Path,
    document: &device::DeviceDocument,
) -> Result<()> {
    store.with_library_connection(|db| {
        let (path, identity): (Vec<u8>, String) = db.query_row(
            "SELECT path,identity FROM sources WHERE id=?1 AND length(path)<=16384 AND length(identity)<=16384",
            [&document.source_id], |r| Ok((r.get(0)?, r.get(1)?)),
        ).map_err(|_| Error::Conflict)?;
        if !path.len().is_multiple_of(2) { return Err(Error::Integrity); }
        let units: Vec<_> = path.chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
        let path = PathBuf::from(OsString::from_wide(&units));
        let profile = profile.canonicalize().map_err(|_| Error::Conflict)?;
        let parent = profile.join("reader-v1/device-markdown-v1");
        let relative = path.strip_prefix(&parent).map_err(|_| Error::Conflict)?;
        if relative.components().count() != 1 || relative.file_name().is_none_or(|name| Uuid::parse_str(&name.to_string_lossy()).is_err()) {
            return Err(Error::Conflict);
        }
        let root = mdreader_core::SourceRoot::restore(path, serde_json::from_str(&identity).map_err(|_| Error::Integrity)?).map_err(|_| Error::Conflict)?;
        mdreader_capture::PrivateDirectory::open(&root.path).map_err(|_| Error::Conflict)?;
        root.verify().map_err(|_| Error::Conflict)
    })
}
struct Ready {
    receipt: AssetReceipt,
    bytes: Vec<u8>,
}
fn copy_bytes(
    source: &mut ReaderStore,
    runtime: &AssetRuntime,
    assets: &AssetStore,
    selected: &AssetManifest,
) -> Result<Vec<Ready>> {
    let mut ready = Vec::new();
    let mut total = 0;
    for receipt in &selected.assets {
        let snapshot = assets
            .prepare_open(
                source,
                &selected.document_id,
                &selected.document_revision,
                &receipt.asset_id,
                &selected.manifest,
                runtime
                    .read_permit(Instant::now())
                    .map_err(|_| Error::Storage)?,
            )
            .and_then(|snapshot| snapshot.verify())
            .map_err(|_| Error::Integrity)?;
        let (session, actual) = runtime
            .install(assets, source, snapshot, Instant::now())
            .map_err(|_| Error::Conflict)?;
        if serde_json::to_vec(&actual).map_err(|_| Error::Integrity)?
            != serde_json::to_vec(receipt).map_err(|_| Error::Integrity)?
        {
            return Err(Error::Conflict);
        }
        let mut bytes = Vec::with_capacity(receipt.bytes as usize);
        loop {
            let chunk = runtime
                .chunk(
                    assets,
                    source,
                    &session,
                    u32::try_from(bytes.len()).map_err(|_| Error::Bounds)?,
                    Instant::now(),
                )
                .map_err(|_| Error::Conflict)?;
            let decoded = STANDARD
                .decode(&chunk.base64)
                .map_err(|_| Error::Integrity)?;
            if decoded.is_empty()
                || decoded.len() > 65536
                || bytes.len() + decoded.len() > receipt.bytes as usize
            {
                return Err(Error::Integrity);
            }
            bytes.extend_from_slice(&decoded);
            if chunk.complete {
                break;
            }
        }
        total += bytes.len();
        if bytes.len() != receipt.bytes as usize || total > 16 * 1024 * 1024 || ready.len() >= 8 {
            return Err(Error::Bounds);
        }
        ready.push(Ready {
            receipt: receipt.clone(),
            bytes,
        });
    }
    let current = assets
        .manifest(source, &selected.document_id, &selected.document_revision)
        .map_err(|_| Error::Conflict)?;
    if serde_json::to_vec(&current).map_err(|_| Error::Integrity)?
        != serde_json::to_vec(selected).map_err(|_| Error::Integrity)?
    {
        return Err(Error::Conflict);
    }
    Ok(ready)
}
pub(in crate::profile_transition::offline) fn convert(
    original: &mut ReaderStore,
    destination: &mut ReaderStore,
    original_root: &Path,
    candidate_root: &Path,
    selected: &[Selection],
) -> Result<(u64, String)> {
    let source_runtime = AssetRuntime::default();
    let source_assets = source_runtime
        .existing_assets(original, &original_root.join("reader-v1/device-assets-v1"))
        .map_err(|_| Error::Integrity)?;
    let runtime = AssetRuntime::default();
    let assets = runtime
        .assets(
            destination,
            &candidate_root.join("reader-v1/device-assets-v1"),
        )
        .map_err(|_| Error::Storage)?;
    let mut hash = Sha256::new();
    hash.update(b"mdreader-profile-selected-import-v1\0");
    for selection in selected {
        let document = device::catalog::document(original, &selection.document_id)
            .map_err(|_| Error::Conflict)?;
        if document.revision != selection.revision {
            return Err(Error::Conflict);
        }
        owned_document(original, original_root, &document)?;
        let body = device::catalog::read(original, &device::catalog::receipt(&document))
            .map_err(|_| Error::Integrity)?;
        let images = match &source_assets {
            Some(assets) => {
                let manifest = assets
                    .manifest(original, &document.id, &document.revision)
                    .map_err(|_| Error::Integrity)?;
                copy_bytes(original, &source_runtime, assets, &manifest)?
            }
            None => Vec::new(),
        };
        let imported = device::import(
            destination,
            &candidate_root.join("reader-v1/device-markdown-v1"),
            document.name.clone(),
            body.markdown,
        )
        .map_err(|_| Error::Storage)?;
        if imported.revision != document.revision {
            return Err(Error::Integrity);
        }
        for image in images {
            let manifest = assets
                .manifest(destination, &imported.id, &imported.revision)
                .map_err(|_| Error::Integrity)?;
            let id = assets
                .begin(
                    destination,
                    &imported.id,
                    &imported.revision,
                    &image.receipt.target,
                    image.receipt.bytes,
                    &manifest.manifest,
                )
                .map_err(|_| Error::Storage)?;
            for (index, chunk) in image.bytes.chunks(65536).enumerate() {
                assets
                    .write(
                        destination,
                        &id,
                        (index * 65536) as u32,
                        &STANDARD.encode(chunk),
                    )
                    .map_err(|_| Error::Storage)?;
            }
            let prepared = assets
                .prepare_commit(
                    destination,
                    &id,
                    runtime.validation_permit().map_err(|_| Error::Storage)?,
                )
                .and_then(|prepared| prepared.validate())
                .map_err(|_| Error::Integrity)?;
            let (actual, _) = assets
                .publish(destination, prepared)
                .map_err(|_| Error::Integrity)?;
            if actual.content_hash != image.receipt.content_hash
                || actual.mime != image.receipt.mime
                || actual.width != image.receipt.width
                || actual.height != image.receipt.height
            {
                return Err(Error::Integrity);
            }
            hash.update((image.receipt.target.len() as u64).to_le_bytes());
            hash.update(image.receipt.target.as_bytes());
            hash.update(Sha256::digest(&image.bytes));
        }
        device::pin(destination, &imported.id, document.pinned).map_err(|_| Error::Storage)?;
        device::position(destination, &imported.id, &imported.revision, document.line)
            .map_err(|_| Error::Integrity)?;
        if device::catalog::document(original, &document.id).map_err(|_| Error::Conflict)?
            != document
        {
            return Err(Error::Conflict);
        }
        for value in [
            &document.id,
            &document.revision,
            &imported.id,
            &imported.source_id,
        ] {
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value.as_bytes());
        }
        hash.update([u8::from(document.pinned)]);
        hash.update(document.line.to_le_bytes());
    }
    Ok((selected.len() as u64, format!("{:x}", hash.finalize())))
}
