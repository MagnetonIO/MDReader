//! Bounded images referenced by an exact Markdown receipt under an existing
//! native Library grant. No arbitrary paths, provider/cache grants or file URLs.
use super::{failure, CatalogError, CatalogReply, LocalMembership};
use crate::reader::ReaderBackend;
use base64::{engine::general_purpose::STANDARD, Engine};
use mdreader_core::{source_root::file_stamp::single_link_stamp, Document, SourceRoot};
use mdreader_service::Request;
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
use tauri::AppHandle;

const MAX_BYTES: u64 = 4 * 1024 * 1024;
static READING: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Permit {
    fn acquire() -> Result<Self, CatalogError> {
        READING
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < 2).then_some(count + 1)
            })
            .map_err(|_| failure("busy"))?;
        Ok(Self)
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        READING.fetch_sub(1, Ordering::AcqRel);
    }
}

fn relative(origin: &str, raw: &str) -> Result<PathBuf, CatalogError> {
    if raw.is_empty() || raw.len() > 4096 {
        return Err(failure("invalid"));
    }
    // Decode URI escapes once; leave no percent syntax for another consumer.
    let mut bytes = Vec::with_capacity(raw.len());
    let mut offset = 0;
    while offset < raw.len() {
        if raw.as_bytes()[offset] == b'%' {
            let digits = raw
                .as_bytes()
                .get(offset + 1..offset + 3)
                .ok_or_else(|| failure("invalid"))?;
            let value = std::str::from_utf8(digits)
                .ok()
                .and_then(|s| u8::from_str_radix(s, 16).ok())
                .ok_or_else(|| failure("invalid"))?;
            bytes.push(value);
            offset += 3;
        } else {
            bytes.push(raw.as_bytes()[offset]);
            offset += 1;
        }
    }
    let decoded = String::from_utf8(bytes).map_err(|_| failure("invalid"))?;
    if decoded.starts_with('/')
        || decoded
            .chars()
            .any(|c| c.is_control() || matches!(c, ':' | '\\' | '?' | '#' | '%'))
    {
        return Err(failure("invalid"));
    }
    let mut parts: Vec<&str> = origin.split('/').collect();
    parts.pop();
    for part in decoded.split('/') {
        match part {
            "" => return Err(failure("invalid")),
            "." => {}
            ".." => {
                parts.pop().ok_or_else(|| failure("unauthorized"))?;
            }
            value => parts.push(value),
        }
    }
    if parts.is_empty() || parts.len() > 128 {
        return Err(failure("invalid"));
    }
    Ok(parts.into_iter().collect())
}

fn exact(
    backend: &ReaderBackend,
    source: &str,
    path: &str,
    revision: &str,
) -> Result<Document, CatalogError> {
    let document: Document = backend
        .typed(Request::Read {
            source_id: source.into(),
            path: path.into(),
            revision: Some(revision.into()),
        })
        .map_err(|_| failure("stale"))?;
    if document.source_id != source || document.path != path || document.revision != revision {
        return Err(failure("stale"));
    }
    Ok(document)
}

fn open(root: &SourceRoot, path: &Path) -> Result<std::fs::File, CatalogError> {
    root.open_optional_file_relative(path)
        .map_err(|_| failure("unauthorized"))?
        .ok_or_else(|| failure("unavailable"))
}

pub(super) fn local(
    app: &AppHandle,
    backend: &ReaderBackend,
    source: &str,
    path: &str,
    revision: &str,
    target: &str,
) -> Result<CatalogReply, CatalogError> {
    if source.len() != 64
        || revision.len() != 64
        || !source
            .bytes()
            .chain(revision.bytes())
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || path.is_empty()
        || path.len() > 4096
        || path.contains([':', '\\'])
        || path.chars().any(char::is_control)
        || Path::new(path)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(failure("invalid"));
    }
    let relative = relative(path, target)?;
    let _permit = Permit::acquire()?;
    if crate::reader_owned::capture_selected(backend, source).map_err(|_| failure("unavailable"))? {
        return Err(failure("unsupported"));
    }
    let membership = LocalMembership::capture(app, backend, source)?;
    membership.revalidate(app, backend)?;
    let root = membership
        .member
        .root()
        .map_err(|_| failure("unauthorized"))?;
    let document = exact(backend, source, path, revision)?;
    if !mdreader_core::parse_markdown(&document.markdown, path)
        .sections
        .iter()
        .flat_map(|section| &section.links)
        .any(|link| link.kind == "image" && link.raw_target == target)
    {
        return Err(failure("unauthorized"));
    }
    let mut file = open(&root, &relative)?;
    let before = single_link_stamp(&file).map_err(|_| failure("unauthorized"))?;
    let length = file.metadata().map_err(|_| failure("unavailable"))?.len();
    if length == 0 || length > MAX_BYTES {
        return Err(failure("too_large"));
    }
    let mut bytes = Vec::with_capacity(length as usize + 1);
    Read::by_ref(&mut file)
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| failure("unavailable"))?;
    if bytes.len() as u64 != length {
        return Err(failure("stale"));
    }
    let raster = mdreader_core::device::assets::validate_raster(&bytes)
        .map_err(|_| failure("unsupported"))?;
    // Decoding and service IPC run outside Library/SQLite owner locks. Recheck
    // the exact document, membership, root and both file handles afterward.
    exact(backend, source, path, revision)?;
    membership.revalidate(app, backend)?;
    root.verify().map_err(|_| failure("stale"))?;
    if single_link_stamp(&file).map_err(|_| failure("stale"))? != before
        || single_link_stamp(&open(&root, &relative)?).map_err(|_| failure("stale"))? != before
    {
        return Err(failure("stale"));
    }
    Ok(CatalogReply::Image {
        source_id: source.into(),
        path: path.into(),
        revision: revision.into(),
        target: target.into(),
        mime: raster.mime,
        base64: STANDARD.encode(bytes),
    })
}
