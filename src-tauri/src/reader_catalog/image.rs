//! Compatibility image reply backed by the shared retained-origin asset proof.
use super::{failure, CatalogError, CatalogReply};
use crate::{
    reader::ReaderBackend,
    reader_assets::{AssetContent, AssetReferenceKind},
};
use tauri::AppHandle;

pub(super) fn local(
    app: &AppHandle,
    backend: &ReaderBackend,
    source: &str,
    path: &str,
    revision: &str,
    target: &str,
) -> Result<CatalogReply, CatalogError> {
    let preview = super::asset::local(
        app,
        backend,
        source,
        path,
        revision,
        target,
        AssetReferenceKind::Image,
    )?;
    let AssetContent::Raster {
        mime,
        width,
        height,
        base64,
    } = preview.content
    else {
        return Err(failure("unsupported"));
    };
    Ok(CatalogReply::Image {
        source_id: source.into(),
        path: path.into(),
        revision: revision.into(),
        target: target.into(),
        mime,
        width,
        height,
        base64,
    })
}

#[cfg(test)]
use super::asset::exact;
#[cfg(all(test, unix))]
use mdreader_core::source_root::file_stamp::single_link_stamp;
#[cfg(test)]
use mdreader_core::{Document, SourceRoot};
#[cfg(test)]
use mdreader_service::Request;
#[cfg(test)]
use std::path::{Path, PathBuf};
#[cfg(test)]
fn relative(origin: &str, target: &str) -> Result<PathBuf, crate::reader_assets::AssetReadError> {
    crate::reader_assets::image_path(origin, target)
}
#[cfg(test)]
fn referenced(markdown: &str, origin: &str, target: &Path) -> bool {
    crate::reader_assets::image_referenced(markdown, origin, target)
}
#[cfg(all(test, unix))]
fn open(root: &SourceRoot, path: &Path) -> Result<std::fs::File, CatalogError> {
    root.open_optional_file_relative(path)
        .map_err(|_| failure("unauthorized"))?
        .ok_or_else(|| failure("unavailable"))
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
        let root = SourceRoot::capture(notes.canonicalize().unwrap()).unwrap();
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
        let root = SourceRoot::capture(scratch.path().canonicalize().unwrap()).unwrap();
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
        let root = SourceRoot::capture(selected.canonicalize().unwrap()).unwrap();
        symlink(outside.join("image.png"), selected.join("leaf.png")).unwrap();
        symlink(&outside, selected.join("parent")).unwrap();
        assert!(open(&root, Path::new("leaf.png")).is_err());
        assert!(open(&root, Path::new("parent/image.png")).is_err());
    }
}
