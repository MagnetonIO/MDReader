//! Rollback requires the exact original snapshot, not just an unchanged root.
use super::*;
use mdreader_core::source_root::file_stamp::single_link_stamp;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io::{Seek, SeekFrom},
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    version: u32,
    entries: Vec<Entry>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: PathBuf,
    kind: String,
    mode: u32,
    uid: u32,
    gid: u32,
    device: u64,
    inode: u64,
    mtime: [i64; 2],
    ctime: [i64; 2],
    bytes: Option<u64>,
    blake3: Option<String>,
    links: Option<u64>,
    #[serde(rename = "targetBase64")]
    target_base64: Option<String>,
}
fn same(entry: &Entry, metadata: &std::fs::Metadata) -> bool {
    metadata.dev() == entry.device
        && metadata.ino() == entry.inode
        && metadata.mode() == entry.mode
        && metadata.uid() == entry.uid
        && metadata.gid() == entry.gid
        && [metadata.mtime(), metadata.mtime_nsec()] == entry.mtime
        && [metadata.ctime(), metadata.ctime_nsec()] == entry.ctime
}
pub(in crate::profile_transition::offline) fn verify_originals(
    root: &SourceRoot,
    archive: &Path,
) -> Result<()> {
    let manifest = legacy::verify(archive, &Default::default())
        .map_err(super::super::archive_import::failure)?;
    let mut originals = manifest.artifacts.iter().filter(|a| a.role == Role::Raw);
    let original = originals.next().ok_or(Error::Integrity)?;
    if originals.next().is_some() {
        return Err(Error::Integrity);
    }
    let member = manifest
        .members
        .iter()
        .find(|m| m.name == original.raw)
        .ok_or(Error::Integrity)?;
    let source = SourceRoot::capture(archive.to_owned()).map_err(|_| Error::Conflict)?;
    let mut file = source
        .open_optional_file_relative(Path::new(&original.raw))
        .map_err(|_| Error::Conflict)?
        .ok_or(Error::Integrity)?;
    let stamp = single_link_stamp(&file).map_err(|_| Error::Conflict)?;
    let mut sha = Sha256::new();
    let mut buffer = [0; 65536];
    let mut bytes = 0u64;
    loop {
        let n = file.read(&mut buffer).map_err(|_| Error::Storage)?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        if bytes > MAX_BYTES {
            return Err(Error::Bounds);
        }
        sha.update(&buffer[..n]);
    }
    if bytes != member.bytes || format!("{:x}", sha.finalize()) != member.sha256 {
        return Err(Error::Integrity);
    }
    file.seek(SeekFrom::Start(0)).map_err(|_| Error::Storage)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| Error::Integrity)?;
    let mut metadata = Vec::new();
    {
        let entry = zip
            .by_name("original-metadata.json")
            .map_err(|_| Error::Integrity)?;
        if entry.size() > 4 * 1024 * 1024 {
            return Err(Error::Bounds);
        }
        entry
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut metadata)
            .map_err(|_| Error::Integrity)?;
    }
    let metadata: Metadata = serde_json::from_slice(&metadata).map_err(|_| Error::Integrity)?;
    if metadata.version != 1 || metadata.entries.len() > MAX_FILES {
        return Err(Error::Unsupported);
    }
    let (files, dirs, links) = scan(root)?;
    let actual_files = files.into_iter().collect::<BTreeSet<_>>();
    let actual_dirs = dirs.keys().cloned().collect::<BTreeSet<_>>();
    let mut observed = Vec::new();
    let mut expected_files = BTreeSet::new();
    let mut expected_dirs = BTreeSet::new();
    let mut expected_links = BTreeSet::new();
    for entry in metadata.entries {
        if entry.path.is_absolute()
            || entry
                .path
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(Error::Integrity);
        }
        match entry.kind.as_str() {
            "directory" => {
                if entry.bytes.is_some()
                    || entry.blake3.is_some()
                    || entry.links.is_some()
                    || entry.target_base64.is_some()
                    || !expected_dirs.insert(entry.path.clone())
                {
                    return Err(Error::Integrity);
                }
                root.verify_directory_relative(&entry.path)
                    .map_err(|_| Error::Conflict)?;
                let m = std::fs::symlink_metadata(root.path.join(&entry.path))
                    .map_err(|_| Error::Conflict)?;
                if !m.is_dir() || m.file_type().is_symlink() || !same(&entry, &m) {
                    return Err(Error::Conflict);
                }
            }
            "file" => {
                if entry.path.as_os_str().is_empty()
                    || entry.links.is_some()
                    || entry.target_base64.is_some()
                    || !expected_files.insert(entry.path.clone())
                {
                    return Err(Error::Integrity);
                }
                let mut input = root
                    .open_optional_file_relative(&entry.path)
                    .map_err(|_| Error::Conflict)?
                    .ok_or(Error::Conflict)?;
                let before = single_link_stamp(&input).map_err(|_| Error::Conflict)?;
                let m = input.metadata().map_err(|_| Error::Storage)?;
                if !same(&entry, &m)
                    || entry.bytes != Some(m.len())
                    || entry.blake3.as_deref() != Some(digest(&mut input)?.to_hex().as_str())
                {
                    return Err(Error::Conflict);
                }
                let fresh = root
                    .open_optional_file_relative(&entry.path)
                    .map_err(|_| Error::Conflict)?
                    .ok_or(Error::Conflict)?;
                observed.push((
                    entry.path.clone(),
                    single_link_stamp(&input).map_err(|_| Error::Conflict)?,
                ));
                if before != single_link_stamp(&input).map_err(|_| Error::Conflict)?
                    || before != single_link_stamp(&fresh).map_err(|_| Error::Conflict)?
                {
                    return Err(Error::Conflict);
                }
            }
            "symlink" => {
                if entry.path.as_os_str().is_empty()
                    || entry.blake3.is_some()
                    || !expected_links.insert(entry.path.clone())
                {
                    return Err(Error::Integrity);
                }
                let encoded = entry.target_base64.as_deref().ok_or(Error::Integrity)?;
                if encoded.len() > 21848 {
                    return Err(Error::Bounds);
                }
                let target = STANDARD.decode(encoded).map_err(|_| Error::Integrity)?;
                let observed = root
                    .read_symlink_relative(&entry.path)
                    .map_err(|_| Error::Conflict)?;
                if links.get(&entry.path) != Some(&observed)
                    || observed.target != target
                    || entry.bytes != Some(observed.bytes)
                    || entry.links != Some(observed.links)
                    || entry.device != observed.device
                    || entry.inode != observed.inode
                    || entry.mode != observed.mode
                    || entry.uid != observed.uid
                    || entry.gid != observed.gid
                    || entry.mtime != observed.mtime
                    || entry.ctime != observed.ctime
                {
                    return Err(Error::Conflict);
                }
            }
            _ => return Err(Error::Integrity),
        }
    }
    if actual_files != expected_files
        || actual_dirs != expected_dirs
        || links.keys().cloned().collect::<BTreeSet<_>>() != expected_links
    {
        return Err(Error::Conflict);
    }
    for (path, stamp) in observed {
        let named = root
            .open_optional_file_relative(&path)
            .map_err(|_| Error::Conflict)?
            .ok_or(Error::Conflict)?;
        if stamp != single_link_stamp(&named).map_err(|_| Error::Conflict)? {
            return Err(Error::Conflict);
        }
    }
    let (final_files, final_dirs, final_links) = scan(root)?;
    if final_files.into_iter().collect::<BTreeSet<_>>() != actual_files
        || final_dirs != dirs
        || final_links != links
    {
        return Err(Error::Conflict);
    }
    let file = zip.into_inner();
    let fresh = source
        .open_optional_file_relative(Path::new(&original.raw))
        .map_err(|_| Error::Conflict)?
        .ok_or(Error::Integrity)?;
    if stamp != single_link_stamp(&file).map_err(|_| Error::Conflict)?
        || stamp != single_link_stamp(&fresh).map_err(|_| Error::Conflict)?
    {
        return Err(Error::Conflict);
    }
    root.verify().map_err(|_| Error::Conflict)?;
    source.verify().map_err(|_| Error::Conflict)
}
