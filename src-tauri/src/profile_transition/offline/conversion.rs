//! Selected domain conversion. Original roots and grants never enter the new owner.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use mdreader_core::{
    device::{self, assets::AssetRuntime},
    ReaderStore,
};
use sha2::{Digest, Sha256};

pub(super) fn documents(store: &ReaderStore) -> Result<Vec<device::DeviceDocument>> {
    let mut all = Vec::new();
    let mut after = None;
    let mut snapshot = None;
    loop {
        let page = device::catalog::page(store, after.as_deref(), snapshot.as_ref(), 100)
            .map_err(|_| Error::Conflict)?;
        all.extend(page.documents);
        if all.len() > 1000 {
            return Err(Error::Bounds);
        }
        after = page.next_after;
        snapshot = page.snapshot;
        if after.is_none() {
            return Ok(all);
        }
    }
}

pub(super) fn owned_document(
    store: &ReaderStore,
    profile: &Path,
    document: &device::DeviceDocument,
) -> Result<()> {
    store.with_library_connection(|db| -> Result<()> {
            use std::os::unix::ffi::OsStringExt;
            let (path, identity): (Vec<u8>, String) = db.query_row(
                "SELECT path,identity FROM sources WHERE id=?1 AND enabled=1 AND length(path)<=16384 AND length(identity)<=16384",
                [&document.source_id], |row| Ok((row.get(0)?, row.get(1)?))
            ).map_err(|_| Error::Conflict)?;
            let path = PathBuf::from(std::ffi::OsString::from_vec(path));
            let parent = profile.join("reader-v1/device-markdown-v1");
            let relative = path.strip_prefix(&parent).map_err(|_| Error::Conflict)?;
            if relative.components().count() != 1 || relative.file_name()
                .is_none_or(|name| uuid::Uuid::parse_str(&name.to_string_lossy()).is_err()) {
                return Err(Error::Conflict);
            }
            mdreader_core::SourceRoot::restore(path, serde_json::from_str(&identity).map_err(|_| Error::Integrity)?)
                .and_then(|root| root.verify()).map_err(|_| Error::Conflict)
        })?;
    Ok(())
}

pub(super) fn convert(
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
        // A device record must belong to the selected profile's private owned
        // materialization namespace; an old external source grant is not copied
        // or read merely because its row uses a document UUID.
        owned_document(original, original_root, &document)?;
        let body = device::catalog::read(original, &device::catalog::receipt(&document))
            .map_err(|_| Error::Integrity)?;
        let archive = source_assets
            .as_ref()
            .map(|assets| {
                device::publication::prepare_archive(
                    original,
                    &document.id,
                    assets,
                    &source_runtime,
                )
                .and_then(|plan| plan.read())
                .map_err(|_| Error::Integrity)
            })
            .transpose()?;
        if archive
            .as_ref()
            .is_some_and(|archive| archive.markdown() != body.markdown.as_bytes())
        {
            return Err(Error::Conflict);
        }
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
        if let Some(archive) = archive {
            for asset in archive.assets() {
                // Partial/retired attachments remain losslessly in the raw archive;
                // they never become ready authority in the new profile.
                if asset.state != "ready" {
                    continue;
                }
                let bytes = asset.bytes().ok_or(Error::Integrity)?;
                if bytes.len() != asset.expected_bytes as usize
                    || asset.received != asset.expected_bytes
                {
                    return Err(Error::Integrity);
                }
                let manifest = assets
                    .manifest(destination, &imported.id, &imported.revision)
                    .map_err(|_| Error::Integrity)?;
                let id = assets
                    .begin(
                        destination,
                        &imported.id,
                        &imported.revision,
                        &asset.target,
                        asset.expected_bytes,
                        &manifest.manifest,
                    )
                    .map_err(|_| Error::Integrity)?;
                for (index, chunk) in bytes.chunks(65536).enumerate() {
                    assets
                        .write(
                            destination,
                            &id,
                            (index * 65536) as u32,
                            &STANDARD.encode(chunk),
                        )
                        .map_err(|_| Error::Storage)?;
                }
                let permit = runtime.validation_permit().map_err(|_| Error::Storage)?;
                let validated = assets
                    .prepare_commit(destination, &id, permit)
                    .and_then(|prepared| prepared.validate())
                    .map_err(|_| Error::Integrity)?;
                assets
                    .publish(destination, validated)
                    .map_err(|_| Error::Integrity)?;
                hash.update((asset.target.len() as u64).to_le_bytes());
                hash.update(asset.target.as_bytes());
                hash.update(Sha256::digest(bytes));
            }
        }
        device::pin(destination, &imported.id, document.pinned).map_err(|_| Error::Storage)?;
        device::position(destination, &imported.id, &imported.revision, document.line)
            .map_err(|_| Error::Integrity)?;
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
