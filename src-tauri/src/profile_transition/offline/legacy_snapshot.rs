//! Complete raw originals plus separately recovered SQLite conversion copies.
//! Reads never migrate, checkpoint or recover the original databases.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use mdreader_core::{
    source_root::file_stamp::{single_link_stamp, SourceFileStamp},
    source_root::SourceEntryKind,
    source_root::SourceSymlinkSnapshot,
    SourceRoot,
};
use mdreader_legacy_export::{self as legacy, Input, Role};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
const MAX_FILES: usize = 4096;
const MAX_BYTES: u64 = 256 * 1024 * 1024;
struct Original {
    file: File,
    stamp: SourceFileStamp,
    hash: blake3::Hash,
}
#[derive(PartialEq, Eq)]
struct Directory(u64, u64, i64, i64, i64, i64);
fn directory(path: &Path) -> Result<Directory> {
    let m = std::fs::symlink_metadata(path).map_err(|_| Error::Storage)?;
    if !m.is_dir() || m.file_type().is_symlink() {
        return Err(Error::Unsupported);
    }
    Ok(Directory(
        m.dev(),
        m.ino(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    ))
}
fn inventory(
    root: &SourceRoot,
    relative: &Path,
    files: &mut Vec<PathBuf>,
    dirs: &mut BTreeMap<PathBuf, Directory>,
    links: &mut BTreeMap<PathBuf, SourceSymlinkSnapshot>,
) -> Result<()> {
    if relative.components().count() > 32 || files.len() + dirs.len() + links.len() >= MAX_FILES {
        return Err(Error::Bounds);
    }
    root.verify_directory_relative(relative)
        .map_err(|_| Error::Conflict)?;
    let path = root.path.join(relative);
    let before = directory(&path)?;
    for entry in root
        .read_dir_relative_bounded(relative, MAX_FILES)
        .map_err(|_| Error::Conflict)?
    {
        if files.len() + dirs.len() + links.len() >= MAX_FILES {
            return Err(Error::Bounds);
        }
        let child = relative.join(entry.name);
        match entry.kind {
            SourceEntryKind::Directory => inventory(root, &child, files, dirs, links)?,
            SourceEntryKind::File => {
                if files.len() >= MAX_FILES {
                    return Err(Error::Bounds);
                }
                files.push(child);
            }
            SourceEntryKind::SymlinkOrReparse => {
                let link = root
                    .read_symlink_relative(&child)
                    .map_err(|_| Error::Conflict)?;
                links.insert(child, link);
            }
            _ => return Err(Error::Unsupported),
        }
    }
    if before != directory(&path)? {
        return Err(Error::Conflict);
    }
    dirs.insert(relative.to_owned(), before);
    root.verify().map_err(|_| Error::Conflict)
}
type Scan = (
    Vec<PathBuf>,
    BTreeMap<PathBuf, Directory>,
    BTreeMap<PathBuf, SourceSymlinkSnapshot>,
);
fn scan(root: &SourceRoot) -> Result<Scan> {
    let mut files = Vec::new();
    let mut dirs = BTreeMap::new();
    let mut links = BTreeMap::new();
    inventory(root, Path::new(""), &mut files, &mut dirs, &mut links)?;
    files.sort();
    Ok((files, dirs, links))
}
fn role(path: &Path) -> Option<Role> {
    match path.file_name()?.to_str()? {
        "cloud-library.catdb" => Some(Role::CloudCatalog),
        "catdb.db" => Some(Role::Catalog),
        "proposals.db" => Some(Role::Proposals),
        "settings.json" if path.components().count() == 1 => Some(Role::Settings),
        "session.json" if path.components().count() == 1 => Some(Role::Session),
        "library.json" if path.components().count() == 1 => Some(Role::Library),
        _ => None,
    }
}
fn digest(file: &mut File) -> Result<blake3::Hash> {
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(0)).map_err(|_| Error::Storage)?;
    let mut hasher = blake3::Hasher::new();
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
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize())
}
fn copy_bounded(input: &mut File, output: &mut File) -> Result<()> {
    let copied =
        std::io::copy(&mut input.take(MAX_BYTES + 1), output).map_err(|_| Error::Storage)?;
    if copied > MAX_BYTES {
        return Err(Error::Bounds);
    }
    Ok(())
}
fn revalidate(
    root: &SourceRoot,
    originals: &mut BTreeMap<PathBuf, Original>,
    files: &Vec<PathBuf>,
    dirs: &BTreeMap<PathBuf, Directory>,
    links: &BTreeMap<PathBuf, SourceSymlinkSnapshot>,
) -> Result<()> {
    for (path, original) in originals {
        let fresh = root
            .open_optional_file_relative(path)
            .map_err(|_| Error::Conflict)?
            .ok_or(Error::Conflict)?;
        if original.stamp != single_link_stamp(&fresh).map_err(|_| Error::Conflict)?
            || original.stamp != single_link_stamp(&original.file).map_err(|_| Error::Conflict)?
            || digest(&mut original.file)? != original.hash
        {
            return Err(Error::Conflict);
        }
    }
    let (current_files, current_dirs, current_links) = scan(root)?;
    if current_files != *files || current_dirs != *dirs || current_links != *links {
        return Err(Error::Conflict);
    }
    Ok(())
}
fn standalone(
    source: &SourceRoot,
    relative: &Path,
    working: &SourceRoot,
    ordinal: usize,
) -> Result<PathBuf> {
    let target = format!("snapshot-{ordinal}.sqlite");
    let mut fence = super::closed_database::recover_to(source, relative, working, &target)?;
    fence.verify()?;
    Ok(working.path.join(target))
}

pub(super) fn capture(
    root: &SourceRoot,
    mut closed: impl FnMut() -> Result<()>,
) -> Result<PathBuf> {
    closed()?;
    let parent = std::env::temp_dir()
        .canonicalize()
        .map_err(|_| Error::Storage)?;
    let destination = super::workspace::private_directory(&parent, "MDReader-legacy-snapshot-")?;
    let output = SourceRoot::capture(destination).map_err(|_| Error::Storage)?;
    let (files, dirs, links) = scan(root)?;
    let mut total = 0u64;
    let mut originals = BTreeMap::new();
    let mut metadata = Vec::new();
    let raw = output
        .create_new_relative(Path::new("complete-originals.zip"))
        .map_err(|_| Error::Storage)?;
    let mut zip = ZipWriter::new(raw);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o600);
    let m = std::fs::symlink_metadata(&root.path).map_err(|_| Error::Storage)?;
    metadata.push(serde_json::json!({"path":"","kind":"directory","mode":m.mode(),"uid":m.uid(),"gid":m.gid(),"device":m.dev(),"inode":m.ino(),"mtime":[m.mtime(),m.mtime_nsec()],"ctime":[m.ctime(),m.ctime_nsec()]}));
    for path in dirs.keys().filter(|p| !p.as_os_str().is_empty()) {
        let m = std::fs::symlink_metadata(root.path.join(path)).map_err(|_| Error::Storage)?;
        metadata.push(serde_json::json!({"path":path,"kind":"directory","mode":m.mode(),"uid":m.uid(),"gid":m.gid(),"device":m.dev(),"inode":m.ino(),"mtime":[m.mtime(),m.mtime_nsec()],"ctime":[m.ctime(),m.ctime_nsec()]}));
        zip.add_directory(
            format!("original/{}", path.to_str().ok_or(Error::Unsupported)?),
            options.unix_permissions(m.mode() & 0o7777),
        )
        .map_err(|_| Error::Storage)?;
    }
    for (path, link) in &links {
        total = total
            .checked_add(link.target.len() as u64)
            .ok_or(Error::Bounds)?;
        if total > MAX_BYTES {
            return Err(Error::Bounds);
        }
        metadata.push(serde_json::json!({"path":path,"kind":"symlink","mode":link.mode,"uid":link.uid,"gid":link.gid,"bytes":link.bytes,"device":link.device,"inode":link.inode,"links":link.links,"mtime":link.mtime,"ctime":link.ctime,"targetBase64":STANDARD.encode(&link.target)}));
        // The metadata always preserves exact bytes (including non-UTF8). ZIP
        // symlinks are additional portable evidence, never extracted/restored.
        if let Ok(target) = std::str::from_utf8(&link.target) {
            zip.add_symlink(
                format!("original/{}", path.to_str().ok_or(Error::Unsupported)?),
                target,
                options.unix_permissions(link.mode & 0o7777),
            )
            .map_err(|_| Error::Storage)?;
        }
    }
    for path in &files {
        let mut file = root
            .open_optional_file_relative(path)
            .map_err(|_| Error::Conflict)?
            .ok_or(Error::Conflict)?;
        let stamp = single_link_stamp(&file).map_err(|_| Error::Conflict)?;
        let m = file.metadata().map_err(|_| Error::Storage)?;
        let bytes = m.len();
        total = total.checked_add(bytes).ok_or(Error::Bounds)?;
        if total > MAX_BYTES {
            return Err(Error::Bounds);
        }
        zip.start_file(
            format!("original/{}", path.to_str().ok_or(Error::Unsupported)?),
            options.unix_permissions(m.mode() & 0o7777),
        )
        .map_err(|_| Error::Storage)?;
        let mut hasher = blake3::Hasher::new();
        let mut copied = 0u64;
        let mut buffer = [0; 65536];
        loop {
            let n = file.read(&mut buffer).map_err(|_| Error::Storage)?;
            if n == 0 {
                break;
            }
            copied += n as u64;
            if copied > bytes {
                return Err(Error::Conflict);
            }
            hasher.update(&buffer[..n]);
            zip.write_all(&buffer[..n]).map_err(|_| Error::Storage)?;
        }
        if copied != bytes || stamp != single_link_stamp(&file).map_err(|_| Error::Conflict)? {
            return Err(Error::Conflict);
        }
        metadata.push(serde_json::json!({"path":path,"kind":"file","mode":m.mode(),"uid":m.uid(),"gid":m.gid(),"bytes":bytes,"device":m.dev(),"inode":m.ino(),"mtime":[m.mtime(),m.mtime_nsec()],"ctime":[m.ctime(),m.ctime_nsec()],"blake3":hasher.finalize().to_hex().as_str()}));
        originals.insert(
            path.clone(),
            Original {
                file,
                stamp,
                hash: hasher.finalize(),
            },
        );
    }
    let mapping = files
        .iter()
        .filter_map(|path| role(path).map(|kind| (path, kind)))
        .collect::<Vec<_>>();
    zip.start_file("original-metadata.json", options)
        .map_err(|_| Error::Storage)?;
    let metadata_bytes = serde_json::to_vec(&serde_json::json!({"version":1,"entries":metadata}))
        .map_err(|_| Error::Storage)?;
    if metadata.len() > MAX_FILES || metadata_bytes.len() > 4 * 1024 * 1024 {
        return Err(Error::Bounds);
    }
    zip.write_all(&metadata_bytes).map_err(|_| Error::Storage)?;
    zip.start_file("snapshot-inputs.json", options)
        .map_err(|_| Error::Storage)?;
    zip.write_all(&serde_json::to_vec(&mapping).map_err(|_| Error::Storage)?)
        .map_err(|_| Error::Storage)?;
    zip.finish()
        .map_err(|_| Error::Storage)?
        .sync_all()
        .map_err(|_| Error::Storage)?;
    let mut inputs = vec![Input {
        role: Role::Raw,
        path: output.path.join("complete-originals.zip"),
    }];
    for path in &files {
        let Some(role) = role(path) else {
            continue;
        };
        if inputs.len() >= 128 {
            return Err(Error::Bounds);
        }
        let ordinal = inputs.len();
        let snapshot = if matches!(role, Role::Catalog | Role::CloudCatalog | Role::Proposals) {
            standalone(root, path, &output, ordinal)?
        } else {
            let mut input = root
                .open_optional_file_relative(path)
                .map_err(|_| Error::Conflict)?
                .ok_or(Error::Conflict)?;
            let name = format!("snapshot-{ordinal}.json");
            let mut target = output
                .create_new_relative(Path::new(&name))
                .map_err(|_| Error::Storage)?;
            copy_bounded(&mut input, &mut target)?;
            target.sync_all().map_err(|_| Error::Storage)?;
            output.path.join(name)
        };
        inputs.push(Input {
            role,
            path: snapshot,
        });
    }
    closed()?;
    if let Some(input) = super::modern_copies::capture(root, &output)? {
        if inputs.len() >= 128 {
            return Err(Error::Bounds);
        }
        inputs.push(input);
    }
    revalidate(root, &mut originals, &files, &dirs, &links)?;
    let archive = output.path.join("archive");
    legacy::export(&inputs, &archive, &Default::default())
        .map_err(super::archive_import::failure)?;
    closed()?;
    revalidate(root, &mut originals, &files, &dirs, &links)?;
    root.verify().map_err(|_| Error::Conflict)?;
    Ok(archive)
}

#[cfg(test)]
#[path = "legacy_snapshot_tests.rs"]
mod tests;

#[path = "legacy_snapshot_verification.rs"]
mod verification;
pub(super) use verification::verify_originals;
