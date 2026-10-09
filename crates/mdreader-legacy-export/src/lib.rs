//! Bounded, private archives of explicitly selected standalone offline snapshots.
//! Never discovers a profile, opens an SDK, imports data, or accesses a vault.
pub mod binding;
pub mod catalog;
pub mod cli;
pub mod cloud;
pub mod history;
#[cfg(any(unix, windows))]
mod history_sources;
pub mod migration;
mod model;
mod records;
pub mod reviews;
#[cfg(unix)]
mod unixfs;
#[cfg(unix)]
use unixfs as archive_io;
#[cfg(windows)]
mod windowsfs;
pub use model::*;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};
#[cfg(windows)]
use windowsfs as archive_io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("archive operation unsupported on this host")]
    Unsupported,
    #[error("invalid explicit offline snapshot input")]
    Invalid,
    #[error("archive limit exceeded")]
    Bounds,
    #[error("snapshot changed or destination already exists")]
    Conflict,
    #[error("archive or snapshot integrity check failed")]
    Integrity,
    #[error("local archive I/O failed")]
    Io(#[from] std::io::Error),
    #[error("offline SQLite snapshot validation failed")]
    Sqlite(#[from] rusqlite::Error),
    #[error("archive JSON validation failed")]
    Json(#[from] serde_json::Error),
    #[cfg(test)]
    #[error("synthetic publication interrupted")]
    Interrupted,
}
pub type Result<T> = std::result::Result<T, Error>;
/// Bounded explicit specification read; never discovers a profile or source.
pub fn read_spec(path: &Path, limits: &Limits) -> Result<Vec<Input>> {
    limits.validate()?;
    #[cfg(any(unix, windows))]
    {
        let mut source = archive_io::Source::open(path, false)?;
        let mut bytes = Vec::new();
        (&mut source.file)
            .take(limits.max_manifest_bytes as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > limits.max_manifest_bytes {
            return Err(Error::Bounds);
        }
        source.validate(
            &hex(&Sha256::digest(&bytes)),
            limits.max_manifest_bytes as u64,
        )?;
        let spec: SnapshotSpec = serde_json::from_slice(&bytes)?;
        spec.selected_inputs(limits)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(Error::Unsupported)
    }
}
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        text.push(DIGITS[(b >> 4) as usize] as char);
        text.push(DIGITS[(b & 15) as usize] as char);
    }
    text
}
fn hash_file(file: &mut File, bound: u64) -> Result<(u64, String)> {
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut bytes = [0; 65536];
    loop {
        let n = file.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        total = total.checked_add(n as u64).ok_or(Error::Bounds)?;
        if total > bound {
            return Err(Error::Bounds);
        }
        hash.update(&bytes[..n]);
    }
    Ok((total, hex(&hash.finalize())))
}
fn copy_hash(input: &mut File, output: &mut File, bound: u64) -> Result<(u64, String)> {
    input.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut bytes = [0; 65536];
    loop {
        let n = input.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        total = total.checked_add(n as u64).ok_or(Error::Bounds)?;
        if total > bound {
            return Err(Error::Bounds);
        }
        output.write_all(&bytes[..n])?;
        hash.update(&bytes[..n]);
    }
    output.sync_all()?;
    Ok((total, hex(&hash.finalize())))
}
/// Inputs must be standalone snapshots made by an offline/owner-coordinated
/// process. A filesystem check cannot prove that another process is idle.
pub fn export(inputs: &[Input], destination: &Path, limits: &Limits) -> Result<Manifest> {
    export_inner(inputs, destination, limits, || Ok(()))
}
#[cfg(not(unix))]
fn export_inner(
    _: &[Input],
    _: &Path,
    _: &Limits,
    _: impl FnMut() -> Result<()>,
) -> Result<Manifest> {
    Err(Error::Unsupported)
}
#[cfg(unix)]
fn export_inner(
    inputs: &[Input],
    destination: &Path,
    limits: &Limits,
    mut checkpoint: impl FnMut() -> Result<()>,
) -> Result<Manifest> {
    limits.validate()?;
    if inputs.is_empty() || inputs.len() > limits.max_artifacts {
        return Err(Error::Bounds);
    }
    let stage = unixfs::Stage::create(destination)?;
    let mut manifest = Manifest::new();
    let mut total = 0u64;
    let mut sources = Vec::with_capacity(inputs.len());
    let mut seen = std::collections::HashSet::new();
    for (index, input) in inputs.iter().enumerate() {
        input.validate()?;
        let mut source = unixfs::Source::open(&input.path, input.role.sqlite())?;
        if !seen.insert(source.identity()) {
            return Err(Error::Invalid);
        }
        let raw = format!("{index:04}.raw");
        let bound = limits
            .max_artifact_bytes
            .min(limits.max_total_bytes - total);
        let expected = source.snapshot_hash(bound)?;
        let mut output = stage.create_file(&raw)?;
        let (bytes, sha256) = copy_hash(&mut source.file, &mut output, bound)?;
        if (bytes, sha256.as_str()) != (expected.0, expected.1.as_str()) {
            return Err(Error::Conflict);
        }
        total = total.checked_add(bytes).ok_or(Error::Bounds)?;
        let source_hash = sha256.clone();
        manifest.members.push(Member {
            name: raw.clone(),
            bytes,
            sha256,
        });
        checkpoint()?;
        stage.validate()?;
        let record_name = format!("{index:04}.records.jsonl");
        let mut record_file = stage.create_file(&record_name)?;
        let conversion =
            records::convert(input.role, &stage.path.join(&raw), &mut record_file, limits)?;
        let (status, records, sqlite) = match conversion {
            records::Converted::Known { rows, sqlite } => {
                record_file.sync_all()?;
                let (bytes, sha256) = hash_file(&mut record_file, limits.max_records_bytes)?;
                if bytes > limits.max_total_bytes - total {
                    return Err(Error::Bounds);
                }
                total += bytes;
                manifest.members.push(Member {
                    name: record_name.clone(),
                    bytes,
                    sha256,
                });
                (Conversion::Known { rows }, Some(record_name), sqlite)
            }
            records::Converted::Unavailable { reason, sqlite } => {
                drop(record_file);
                stage.remove_file(&record_name)?;
                (Conversion::Unavailable { reason }, None, sqlite)
            }
        };
        stage.validate()?;
        source.validate(&source_hash, limits.max_artifact_bytes)?;
        manifest.artifacts.push(Artifact {
            role: input.role,
            locator: Locator::capture(&input.path)?,
            raw,
            records,
            conversion: status,
            sqlite,
        });
        sources.push((source, source_hash));
    }
    for (source, hash) in &mut sources {
        source.validate(hash, limits.max_artifact_bytes)?;
    }
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    if bytes.len() > limits.max_manifest_bytes
        || total + bytes.len() as u64 > limits.max_total_bytes
    {
        return Err(Error::Bounds);
    }
    let mut file = stage.create_file("manifest.json")?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    stage.sync()?;
    checkpoint()?;
    verify_inner(&stage.path, limits, false)?;
    stage.publish()?;
    Ok(manifest)
}
/// Integrity verification is not authentication against a trusted signer.
pub fn verify(path: &Path, limits: &Limits) -> Result<Manifest> {
    verify_inner(path, limits, true)
}
#[cfg(not(any(unix, windows)))]
fn verify_inner(_: &Path, _: &Limits, _: bool) -> Result<Manifest> {
    Err(Error::Unsupported)
}
#[cfg(any(unix, windows))]
fn verify_inner(path: &Path, limits: &Limits, completed_name: bool) -> Result<Manifest> {
    limits.validate()?;
    if completed_name {
        archive_io::valid_destination(path)?;
    }
    let directory = archive_io::private_directory(path)?;
    #[cfg(unix)]
    let _private_manifest = unixfs::open_file(&directory, "manifest.json")?;
    let mut manifest_source = archive_io::Source::open(&path.join("manifest.json"), false)?;
    let file = &mut manifest_source.file;
    let mut bytes = Vec::new();
    file.take(limits.max_manifest_bytes as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limits.max_manifest_bytes {
        return Err(Error::Bounds);
    }
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    manifest.validate(limits)?;
    let mut total = bytes.len() as u64;
    let mut expected = std::collections::BTreeSet::from([String::from("manifest.json")]);
    for member in &manifest.members {
        if !expected.insert(member.name.clone()) {
            return Err(Error::Integrity);
        }
        #[cfg(unix)]
        let _private_member = unixfs::open_file(&directory, &member.name)?;
        let mut file = archive_io::Source::open(&path.join(&member.name), false)?;
        let bound = if member.name.ends_with(".raw") {
            limits.max_artifact_bytes
        } else {
            limits.max_records_bytes
        };
        let (length, hash) = file.snapshot_hash(bound)?;
        if length != member.bytes || hash != member.sha256 {
            return Err(Error::Integrity);
        }
        total = total.checked_add(length).ok_or(Error::Bounds)?;
        if total > limits.max_total_bytes {
            return Err(Error::Bounds);
        }
    }
    manifest_source.validate(
        &hex(&Sha256::digest(&bytes)),
        limits.max_manifest_bytes as u64,
    )?;
    let actual = std::fs::read_dir(path)?
        .take(expected.len() + 1)
        .map(|entry| {
            entry
                .map(|e| e.file_name().into_string().map_err(|_| Error::Integrity))
                .map_err(Error::from)
                .and_then(|v| v)
        })
        .collect::<Result<std::collections::BTreeSet<_>>>()?;
    if actual != expected {
        return Err(Error::Integrity);
    }
    archive_io::validate_directory(&directory, path)?;
    Ok(manifest)
}
#[cfg(all(test, unix))]
mod cloud_tests;
#[cfg(test)]
mod tests;
