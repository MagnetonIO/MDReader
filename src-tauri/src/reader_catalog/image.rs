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

fn referenced(markdown: &str, path: &str, target: &Path) -> bool {
    mdreader_core::parse_markdown(markdown, path)
        .sections
        .iter()
        .flat_map(|section| &section.links)
        .any(|link| {
            link.kind == "image"
                && relative(path, &link.raw_target).is_ok_and(|resolved| resolved == target)
        })
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
    if !referenced(&document.markdown, path, &relative) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn relative_mapping_allows_in_root_images_and_refuses_uri_or_encoded_escape() {
        assert_eq!(
            relative("notes/readme.md", "../images/a%20b.png").unwrap(),
            Path::new("images/a b.png")
        );
        assert_eq!(
            relative("notes/readme.md", "./images/猫.png").unwrap(),
            Path::new("notes/images/猫.png")
        );
        for target in [
            "../../outside.png",
            "%2e%2e/%2e%2e/outside.png",
            "/absolute.png",
            "https://example.com/image.png",
            "data:image/png;base64,AA",
            "file:///image.png",
            "image%00.png",
            "image%5c.png",
            "image%3astream.png",
            "image.png?x=1",
            "image.png#fragment",
            "image%252e.png",
            "image%ff.png",
            "image%.png",
        ] {
            assert!(
                relative("notes/readme.md", target).is_err(),
                "refuse {target}"
            );
        }
    }

    #[test]
    fn only_actual_ast_images_authorize_exact_targets() {
        let markdown = "# Images\n\n![Selected](images/selected.png)\n\n[Reference image][photo]\n\n![Named][photo]\n\n[photo]: images/named.jpg\n\n[ordinary](images/link.png)\n\n`![code](images/inline.png)`\n\n```md\n![fenced](images/fenced.png)\n```\n\n<img src=\"images/raw.png\">\n";
        let authorizes =
            |target: &str| referenced(markdown, "note.md", &relative("note.md", target).unwrap());
        assert!(authorizes("images/selected.png"));
        assert!(authorizes("./images/selected.png"));
        assert!(authorizes("images/named.jpg"));
        let encoded = "![Unicode](café.png)\n\n![Spaces](<my diagram.png>)\n";
        for target in ["caf%C3%A9.png", "my%20diagram.png"] {
            assert!(referenced(
                encoded,
                "note.md",
                &relative("note.md", target).unwrap()
            ));
        }
        for target in [
            "images/link.png",
            "images/inline.png",
            "images/fenced.png",
            "images/raw.png",
            "images/unselected.png",
        ] {
            assert!(!authorizes(target), "no authority for {target}");
        }
    }

    #[test]
    fn exact_read_rejects_changed_markdown_revision() {
        let scratch = tempfile::tempdir().unwrap();
        let notes = scratch.path().join("notes");
        fs::create_dir(&notes).unwrap();
        fs::write(notes.join("note.md"), "![Old](old.png)\n").unwrap();
        let root = SourceRoot::capture(&notes.canonicalize().unwrap()).unwrap();
        let backend = ReaderBackend::embedded(&scratch.path().join("profile")).unwrap();
        let source = backend.admit(&root).unwrap();
        let first: Document = backend
            .typed(Request::Read {
                source_id: source.clone(),
                path: "note.md".into(),
                revision: None,
            })
            .unwrap();
        assert!(exact(&backend, &source, "note.md", &first.revision).is_ok());
        fs::write(notes.join("note.md"), "![New](new.png)\n").unwrap();
        assert!(exact(&backend, &source, "note.md", &first.revision).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn retained_file_stamps_detect_replacement_and_reject_hardlinks() {
        let scratch = tempfile::tempdir().unwrap();
        let root = SourceRoot::capture(&scratch.path().canonicalize().unwrap()).unwrap();
        fs::write(root.path.join("image.png"), b"same bytes").unwrap();
        let retained = open(&root, Path::new("image.png")).unwrap();
        let stamp = single_link_stamp(&retained).unwrap();
        assert!(single_link_stamp(&retained).unwrap() == stamp);
        fs::rename(root.path.join("image.png"), root.path.join("original.png")).unwrap();
        fs::write(root.path.join("image.png"), b"same bytes").unwrap();
        let replacement = open(&root, Path::new("image.png")).unwrap();
        assert!(single_link_stamp(&replacement).unwrap() != stamp);
        fs::hard_link(root.path.join("image.png"), root.path.join("alias.png")).unwrap();
        assert!(single_link_stamp(&replacement).is_err());
        assert!(single_link_stamp(&open(&root, Path::new("alias.png")).unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn retained_walk_refuses_leaf_and_parent_symlinks() {
        use std::os::unix::fs::symlink;
        let scratch = tempfile::tempdir().unwrap();
        let selected = scratch.path().join("selected");
        let outside = scratch.path().join("outside");
        fs::create_dir(&selected).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("image.png"), b"outside bytes").unwrap();
        let root = SourceRoot::capture(&selected.canonicalize().unwrap()).unwrap();
        symlink(outside.join("image.png"), selected.join("leaf.png")).unwrap();
        symlink(&outside, selected.join("parent")).unwrap();
        assert!(open(&root, Path::new("leaf.png")).is_err());
        assert!(open(&root, Path::new("parent/image.png")).is_err());
    }
}
