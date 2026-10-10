//! Windows archive-fed materialization at a fixed new private root. Archived
//! bytes remain immutable; source grants and credentials are never reactivated.
use super::*;
use crate::{archive_io, hash_file};
use std::io::Write;
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

/// Copy every verified member, including unsupported raw records. No existing
/// destination is adopted or replaced; failed stages remain visibly unfinished.
pub fn copy(
    archive: &Path,
    expected: &ArchiveIdentity,
    destination: &Path,
    limits: &Limits,
) -> Result<()> {
    if identity(archive, limits)? != *expected {
        return Err(Error::Conflict);
    }
    let manifest = crate::verify(archive, limits)?;
    let stage = archive_io::Stage::create(destination)?;
    for member in &manifest.members {
        let mut source = archive_io::Source::open(&archive.join(&member.name), false)?;
        source.validate(&member.sha256, limits.max_total_bytes)?;
        let mut target = stage.create_file(&member.name)?;
        let (bytes, hash) = crate::copy_hash(&mut source.file, &mut target, member.bytes)?;
        if bytes != member.bytes || hash != member.sha256 {
            return Err(Error::Integrity);
        }
        source.validate(&member.sha256, limits.max_total_bytes)?;
    }
    let mut source = archive_io::Source::open(&archive.join("manifest.json"), false)?;
    let mut target = stage.create_file("manifest.json")?;
    let (_, hash) = crate::copy_hash(
        &mut source.file,
        &mut target,
        limits.max_manifest_bytes as u64,
    )?;
    if hash != expected.manifest_sha256 {
        return Err(Error::Conflict);
    }
    source.validate(&hash, limits.max_manifest_bytes as u64)?;
    drop(target);
    stage.sync()?;
    stage.publish()?;
    if identity(destination, limits)? != *expected || identity(archive, limits)? != *expected {
        return Err(Error::Conflict);
    }
    Ok(())
}

/// Use a fixed new root because imported device identities contain native paths.
/// Write the preparation marker last, after useful-record conversion, archived
/// review staging and exact archive revalidation. Failures retain all originals.
pub fn prepare(
    archive: &Path,
    expected: &ArchiveIdentity,
    destination: &Path,
    limits: &Limits,
    materialize: impl FnOnce(&Path, &ArchiveIdentity, &Path) -> Result<u64>,
) -> Result<PreparedProfile> {
    limits.validate()?;
    archive_io::valid_destination(destination)?;
    let parent_path = destination.parent().ok_or(Error::Invalid)?;
    let parent =
        mdreader_capture::PrivateDirectory::open(parent_path).map_err(|_| Error::Invalid)?;
    let root = parent
        .create_child(Path::new(destination.file_name().ok_or(Error::Invalid)?))
        .map_err(|_| Error::Conflict)?;
    let retained = destination.join("legacy-archive");
    copy(archive, expected, &retained, limits)?;
    let reviews = crate::reviews::stage_reviews(
        &retained,
        expected,
        &destination.join("archived-reviews"),
        limits,
    )?;
    let materialized_records = materialize(&retained, expected, destination)?;
    if materialized_records > 1000 {
        return Err(Error::Bounds);
    }
    let manifest = crate::verify(&retained, limits)?;
    if identity(archive, limits)? != *expected || identity(&retained, limits)? != *expected {
        return Err(Error::Conflict);
    }
    let profile = PreparedProfile {
        format: "mdreader-reader-profile".into(),
        version: 2,
        archive: expected.clone(),
        settings_artifact: settings_artifact(&manifest)?,
        review_records: reviews.records,
        unavailable_artifacts: reviews.unavailable_artifacts,
        sources_require_selection: true,
        credentials_require_reconnect: true,
        materialized_records: Some(materialized_records),
    };
    let bytes = serde_json::to_vec(&profile)?;
    if bytes.len() > limits.max_manifest_bytes {
        return Err(Error::Bounds);
    }
    let mut marker = root
        .create_database_file(Path::new(MARKER))
        .map_err(|_| Error::Conflict)?;
    marker.write_all(&bytes)?;
    marker.sync_all()?;
    drop(marker);
    root.verify().map_err(|_| Error::Conflict)?;
    parent.verify().map_err(|_| Error::Conflict)?;
    verify(destination, limits)?;
    Ok(profile)
}
