//! Windows completed-archive identity. No live owner, writable SQLite admission,
//! stage publication or original source grant is created by this read adapter.
use crate::{archive_io, hash_file, reviews::ArchiveIdentity, Limits, Result};
use std::path::Path;
pub fn identity(archive: &Path, limits: &Limits) -> Result<ArchiveIdentity> {
    let manifest = crate::verify(archive, limits)?;
    let mut file = archive_io::Source::open(&archive.join("manifest.json"), false)?;
    let (_, hash) = hash_file(&mut file.file, limits.max_manifest_bytes as u64)?;
    file.validate(&hash, limits.max_manifest_bytes as u64)?;
    Ok(ArchiveIdentity {
        export_id: manifest.export_id,
        manifest_sha256: hash,
    })
}
