//! Offline, additive migration into a distinct private native profile.
//! Raw snapshots remain authoritative. Source membership and credentials are
//! never activated from an archive; source folders must be selected again.
use crate::{
    reviews::{ArchiveIdentity, ReviewArchive},
    Conversion, Error, Limits, Manifest, Result, Role,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
#[cfg(unix)]
use std::{
    fs::File,
    io::{Read, Write},
};

#[cfg(unix)]
pub mod archive;
#[cfg(windows)]
#[path = "migration/archive_readonly.rs"]
pub mod archive;

const MARKER: &str = "reader-profile.json";
// Match the native JSON owner: larger preferences remain in the raw archive,
// but cannot be activated as a reader preference file.
const MAX_ACTIVE_SETTINGS_BYTES: u64 = 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedProfile {
    pub format: String,
    pub version: u8,
    pub archive: ArchiveIdentity,
    /// Raw archive index copied losslessly into settings.json, if supported.
    pub settings_artifact: Option<usize>,
    pub review_records: u64,
    pub unavailable_artifacts: usize,
    pub sources_require_selection: bool,
    pub credentials_require_reconnect: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialized_records: Option<u64>,
}

/// The caller must first stop the original app/service and supply consistent
/// standalone snapshots. This API neither discovers nor freezes any old owner.
/// Publication never replaces an existing profile, input, or source document.
#[cfg(unix)]
pub fn prepare(spec: &Path, destination: &Path, limits: &Limits) -> Result<PreparedProfile> {
    let inputs = crate::read_spec(spec, limits)?;
    let stage = crate::unixfs::Stage::create(destination)?;
    let archive_path = stage.path.join("legacy-archive");
    let manifest = crate::export(&inputs, &archive_path, limits)?;
    stage.validate()?;
    let mut file = File::open(archive_path.join("manifest.json"))?;
    let (_, hash) = crate::hash_file(&mut file, limits.max_manifest_bytes as u64)?;
    let identity = ArchiveIdentity {
        export_id: manifest.export_id,
        manifest_sha256: hash,
    };
    let receipt = crate::reviews::stage_reviews(
        &archive_path,
        &identity,
        &stage.path.join("archived-reviews"),
        limits,
    )?;
    stage.validate()?;
    let settings = settings_artifact(&manifest)?;
    if let Some(index) = settings {
        let artifact = &manifest.artifacts[index];
        let mut input = File::open(archive_path.join(&artifact.raw))?;
        let mut output = stage.create_file("settings.json")?;
        let (bytes, hash) = crate::copy_hash(
            &mut input,
            &mut output,
            MAX_ACTIVE_SETTINGS_BYTES.min(limits.max_json_bytes as u64),
        )?;
        let member = manifest
            .members
            .iter()
            .find(|m| m.name == artifact.raw)
            .ok_or(Error::Integrity)?;
        if bytes != member.bytes || hash != member.sha256 {
            return Err(Error::Integrity);
        }
    }
    let profile = PreparedProfile {
        format: "mdreader-reader-profile".into(),
        version: 1,
        archive: identity,
        settings_artifact: settings,
        review_records: receipt.records,
        unavailable_artifacts: receipt.unavailable_artifacts,
        sources_require_selection: true,
        credentials_require_reconnect: true,
        materialized_records: None,
    };
    let bytes = serde_json::to_vec(&profile)?;
    if bytes.len() > limits.max_manifest_bytes {
        return Err(Error::Bounds);
    }
    let mut marker = stage.create_file(MARKER)?;
    marker.write_all(&bytes)?;
    marker.sync_all()?;
    stage.sync()?;
    // Validate the same finished artifact that native startup will admit.
    verify(&stage.path, limits)?;
    stage.publish()?;
    Ok(profile)
}
#[cfg(not(unix))]
pub fn prepare(_: &Path, _: &Path, _: &Limits) -> Result<PreparedProfile> {
    Err(Error::Unsupported)
}

fn settings_artifact(manifest: &Manifest) -> Result<Option<usize>> {
    let selected: Vec<_> = manifest
        .artifacts
        .iter()
        .enumerate()
        .filter(|(_, a)| a.role == Role::Settings)
        .collect();
    if selected.len() > 1 {
        return Err(Error::Invalid);
    }
    Ok(selected
        .first()
        .and_then(|(index, a)| matches!(a.conversion, Conversion::Known { .. }).then_some(*index)))
}
/// Verifies the immutable migration anchor, not current mutable preferences or
/// reader state. Later edits to settings.json are ordinary native persistence.
#[cfg(unix)]
pub fn verify(directory: &Path, limits: &Limits) -> Result<PreparedProfile> {
    limits.validate()?;
    let root = crate::unixfs::private_directory(directory)?;
    let mut marker = crate::unixfs::Source::open(&directory.join(MARKER), false)?;
    let mut bytes = Vec::new();
    (&mut marker.file)
        .take(limits.max_manifest_bytes as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limits.max_manifest_bytes {
        return Err(Error::Bounds);
    }
    let profile: PreparedProfile = serde_json::from_slice(&bytes)?;
    if profile.format != "mdreader-reader-profile"
        || !matches!(profile.version, 1 | 2)
        || profile.version == 2 && profile.materialized_records.is_none_or(|n| n > 1000)
        || profile.version == 1 && profile.materialized_records.is_some()
        || !profile.sources_require_selection
        || !profile.credentials_require_reconnect
    {
        return Err(Error::Invalid);
    }
    let archive = directory.join("legacy-archive");
    let manifest = crate::verify(&archive, limits)?;
    let mut manifest_file = crate::unixfs::Source::open(&archive.join("manifest.json"), false)?;
    manifest_file.validate(
        &profile.archive.manifest_sha256,
        limits.max_manifest_bytes as u64,
    )?;
    if manifest.export_id != profile.archive.export_id
        || settings_artifact(&manifest)? != profile.settings_artifact
    {
        return Err(Error::Conflict);
    }
    ReviewArchive::open(
        &directory.join("archived-reviews"),
        &profile.archive,
        limits,
    )?;
    use sha2::{Digest, Sha256};
    marker.validate(
        &crate::hex(&Sha256::digest(&bytes)),
        limits.max_manifest_bytes as u64,
    )?;
    crate::unixfs::validate_directory(&root, directory)?;
    Ok(profile)
}
#[cfg(not(unix))]
pub fn verify(_: &Path, _: &Limits) -> Result<PreparedProfile> {
    Err(Error::Unsupported)
}
