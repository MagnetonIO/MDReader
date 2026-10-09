//! Archive-fed materialization at a FIXED new root. Source grants store native
//! paths, so the root must never be renamed after the converter publishes them.
use super::*;
use crate::{hash_file, unixfs};
use rustix::fs::{Mode, OFlags};
use std::{fs::File, io::Write, os::unix::fs::DirBuilderExt};
fn create(directory: &File, name: &str) -> Result<File> {
    Ok(File::from(
        rustix::fs::openat(
            directory,
            name,
            OFlags::CREATE | OFlags::EXCL | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|e| std::io::Error::from_raw_os_error(e.raw_os_error()))?,
    ))
}
pub fn identity(archive: &Path, limits: &Limits) -> Result<ArchiveIdentity> {
    let manifest = crate::verify(archive, limits)?;
    let mut file = unixfs::Source::open(&archive.join("manifest.json"), false)?;
    let (_, hash) = hash_file(&mut file.file, limits.max_manifest_bytes as u64)?;
    file.validate(&hash, limits.max_manifest_bytes as u64)?;
    Ok(ArchiveIdentity {
        export_id: manifest.export_id,
        manifest_sha256: hash,
    })
}
/// Copies every verified member, including unsupported raw data. Partial copies
/// remain visibly unprepared after failure; originals are never removed.
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
    let stage = unixfs::Stage::create(destination)?;
    for member in &manifest.members {
        let mut source = unixfs::Source::open(&archive.join(&member.name), false)?;
        source.validate(&member.sha256, limits.max_total_bytes)?;
        let mut target = stage.create_file(&member.name)?;
        let (bytes, hash) = crate::copy_hash(&mut source.file, &mut target, member.bytes)?;
        if bytes != member.bytes || hash != member.sha256 {
            return Err(Error::Integrity);
        }
        source.validate(&member.sha256, limits.max_total_bytes)?;
    }
    let mut source = unixfs::Source::open(&archive.join("manifest.json"), false)?;
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
    stage.sync()?;
    stage.publish()?;
    if identity(destination, limits)? != *expected {
        return Err(Error::Conflict);
    }
    Ok(())
}
/// The native converter only receives a newly created private fixed root. It
/// must not restore original credentials/grants. A completed marker is written
/// LAST, after conversion and exact source/archive revalidation.
pub fn prepare(
    archive: &Path,
    expected: &ArchiveIdentity,
    destination: &Path,
    limits: &Limits,
    materialize: impl FnOnce(&Path, &ArchiveIdentity, &Path) -> Result<u64>,
) -> Result<PreparedProfile> {
    limits.validate()?;
    unixfs::valid_destination(destination)?;
    let parent = unixfs::private_directory(destination.parent().ok_or(Error::Invalid)?)?;
    std::fs::DirBuilder::new().mode(0o700).create(destination)?;
    parent.sync_all()?;
    let root = unixfs::private_directory(destination)?;
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
    // Legacy settings/session are preserved RAW. They are not installed as
    // active native files: old connection refs/grants/tokens cannot propagate.
    let bytes = serde_json::to_vec(&profile)?;
    if bytes.len() > limits.max_manifest_bytes {
        return Err(Error::Bounds);
    }
    let mut marker = create(&root, MARKER)?;
    marker.write_all(&bytes)?;
    marker.sync_all()?;
    root.sync_all()?;
    unixfs::validate_directory(&root, destination)?;
    verify(destination, limits)?;
    Ok(profile)
}
