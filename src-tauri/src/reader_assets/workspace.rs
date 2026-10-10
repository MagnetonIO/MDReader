use super::{
    path, AssetFailure as F, AssetObservation, AssetPreview, AssetReadError, BoundedFile, Permit,
    WorkspaceAssetRequest,
};
use crate::commands::WorkspaceState;

pub(super) fn read(
    state: &WorkspaceState,
    request: WorkspaceAssetRequest,
) -> Result<AssetPreview, AssetReadError> {
    read_with_hook(state, request, || {})
}
fn read_with_hook(
    state: &WorkspaceState,
    request: WorkspaceAssetRequest,
    before_finish: impl FnOnce(),
) -> Result<AssetPreview, AssetReadError> {
    let _permit = Permit::acquire()?;
    path::origin(&request.origin.path)?;
    if request.origin.epoch.len() != 36
        || request.origin.hash.len() != 64
        || !request
            .origin
            .hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(F::Invalid.error());
    }
    if !crate::commands::is_markdown(std::path::Path::new(&request.origin.path)) {
        return Err(F::Unsupported.error());
    }
    let epoch = state.current().ok_or_else(|| F::Unauthorized.error())?;
    if request.origin.epoch != epoch.read_receipt.to_string() {
        return Err(F::Stale.error());
    }
    let root = epoch.source_root();
    let origin = BoundedFile::read(
        &root,
        &request.origin.path,
        mdreader_core::MAX_DOCUMENT_BYTES,
    )?;
    if blake3::hash(&origin.bytes).to_hex().as_str() != request.origin.hash {
        return Err(F::Stale.error());
    }
    let markdown = std::str::from_utf8(&origin.bytes).map_err(|_| F::Unsupported.error())?;
    let asset = AssetObservation::read(
        &root,
        &request.origin.path,
        markdown,
        &request.target,
        request.kind,
    )?;
    before_finish();
    origin.revalidate(&root)?;
    asset.revalidate(&root)?;
    epoch
        .finish(state, asset.preview)
        .map_err(|_| F::Stale.error())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader_assets::{AssetContent, AssetReferenceKind, WorkspaceOriginReceipt};
    use std::{fs, path::Path};
    fn request(
        state: &WorkspaceState,
        target: &str,
        kind: AssetReferenceKind,
    ) -> WorkspaceAssetRequest {
        let epoch = state.current().unwrap();
        let bytes = epoch
            .source_root()
            .read_relative(Path::new("docs/note.md"))
            .unwrap()
            .bytes;
        WorkspaceAssetRequest {
            origin: WorkspaceOriginReceipt::from_read(
                &epoch,
                Path::new("docs/note.md"),
                &blake3::hash(&bytes).to_hex(),
            )
            .unwrap(),
            target: target.into(),
            kind,
        }
    }
    fn fixture(markdown: &str) -> (tempfile::TempDir, WorkspaceState) {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("docs")).unwrap();
        fs::write(directory.path().join("docs/note.md"), markdown).unwrap();
        let state = WorkspaceState::granted(directory.path().canonicalize().unwrap());
        (directory, state)
    }
    #[test]
    fn saved_ast_links_preview_inert_templates_compose_sql_diagrams_and_raster() {
        let (directory, state) = fixture("[Template](../foundation.bicep)\n\n[Compose](../compose.target.yml)\n\n[SQL](../schema.sql)\n\n[Diagram](../rad.drawio)\n\n![Image](../diagram.png)\n\n[PNG](../diagram.png)");
        for (name, content, format) in [
            ("foundation.bicep", "resource x 'a@b' = {}", "bicep"),
            ("compose.target.yml", "services:\n  api: {}", "yaml"),
            ("schema.sql", "DROP TABLE inert_preview_only;", "sql"),
            (
                "rad.drawio",
                "<mxfile><script>inert</script></mxfile>",
                "xml",
            ),
        ] {
            fs::write(directory.path().join(name), content).unwrap();
            let preview = read(
                &state,
                request(
                    &state,
                    &format!("../{name}"),
                    AssetReferenceKind::Attachment,
                ),
            )
            .unwrap();
            assert_eq!(preview.path, name);
            assert_eq!(preview.revision.len(), 64);
            let AssetContent::Text {
                mime,
                text,
                format: observed,
            } = preview.content
            else {
                panic!("not a text preview")
            };
            assert_eq!(mime, "text/plain");
            assert_eq!(observed, format);
            assert_eq!(text, content);
            assert_eq!(
                fs::read_to_string(directory.path().join(name)).unwrap(),
                content
            );
        }
        let png = include_bytes!("../../../fixtures/markdown-navigation/assets/diagram.png");
        fs::write(directory.path().join("diagram.png"), png).unwrap();
        for kind in [AssetReferenceKind::Image, AssetReferenceKind::Attachment] {
            let preview = read(&state, request(&state, "../diagram.png", kind)).unwrap();
            let AssetContent::Raster {
                mime,
                width,
                height,
                ..
            } = preview.content
            else {
                panic!("not a raster preview")
            };
            assert_eq!(mime, "image/png");
            assert!(width > 0 && height > 0);
        }
    }
    #[test]
    fn receipt_and_final_epoch_origin_asset_fences_reject_stale_bytes() {
        let (directory, state) = fixture("[Selected](../selected.sql)");
        fs::write(directory.path().join("selected.sql"), "SELECT 1;").unwrap();
        let old = request(&state, "../selected.sql", AssetReferenceKind::Attachment);
        state
            .switch(directory.path().canonicalize().unwrap())
            .unwrap();
        assert!(matches!(read(&state, old).err().unwrap().kind, F::Stale));
        let changed = request(&state, "../selected.sql", AssetReferenceKind::Attachment);
        assert!(matches!(
            read_with_hook(&state, changed, || {
                fs::write(
                    directory.path().join("docs/note.md"),
                    "[Different](../elsewhere.sql)",
                )
                .unwrap();
            })
            .err()
            .unwrap()
            .kind,
            F::Stale
        ));
        fs::write(
            directory.path().join("docs/note.md"),
            "[Selected](../selected.sql)",
        )
        .unwrap();
        let changed = request(&state, "../selected.sql", AssetReferenceKind::Attachment);
        assert!(matches!(
            read_with_hook(&state, changed, || {
                fs::rename(
                    directory.path().join("selected.sql"),
                    directory.path().join("old.sql"),
                )
                .unwrap();
                fs::write(directory.path().join("selected.sql"), "SELECT 1;").unwrap();
            })
            .err()
            .unwrap()
            .kind,
            F::Stale
        ));
        let changed = request(&state, "../selected.sql", AssetReferenceKind::Attachment);
        assert!(matches!(
            read_with_hook(&state, changed, || {
                state
                    .switch(directory.path().canonicalize().unwrap())
                    .unwrap();
            })
            .err()
            .unwrap()
            .kind,
            F::Stale
        ));
    }
    #[test]
    fn code_unsaved_targets_wrong_ast_kinds_binary_utf8_and_size_refuse() {
        let (directory, state) = fixture("[Selected](../selected.sql)\n\n`[Secret](../secret.sql)`\n\n```md\n[Hidden](../hidden.sql)\n```\n\n[Binary](../program.exe)\n\n[Invalid](../invalid.sql)\n\n[Big](../big.sql)\n\n![Only image](../image.png)");
        fs::write(directory.path().join("selected.sql"), "SELECT 1;").unwrap();
        fs::write(directory.path().join("secret.sql"), "secret fixture").unwrap();
        fs::write(directory.path().join("invalid.sql"), [0xff]).unwrap();
        fs::write(
            directory.path().join("big.sql"),
            vec![b'x'; 1024 * 1024 + 1],
        )
        .unwrap();
        for target in [
            "../secret.sql",
            "../hidden.sql",
            "../unsaved.sql",
            "../../outside.sql",
            "file:///selected.sql",
            "../selected.sql?q=x",
        ] {
            assert!(
                read(
                    &state,
                    request(&state, target, AssetReferenceKind::Attachment)
                )
                .is_err(),
                "accepted {target}"
            );
        }
        for (target, kind, failure) in [
            (
                "../program.exe",
                AssetReferenceKind::Attachment,
                F::Unsupported,
            ),
            (
                "../invalid.sql",
                AssetReferenceKind::Attachment,
                F::Unsupported,
            ),
            ("../big.sql", AssetReferenceKind::Attachment, F::TooLarge),
            (
                "../selected.sql",
                AssetReferenceKind::Image,
                F::Unauthorized,
            ),
            (
                "../image.png",
                AssetReferenceKind::Attachment,
                F::Unauthorized,
            ),
        ] {
            assert_eq!(
                read(&state, request(&state, target, kind))
                    .err()
                    .unwrap()
                    .kind
                    .as_str(),
                failure.as_str()
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn attachment_symlink_hardlink_and_replaced_root_are_not_grants() {
        use std::os::unix::fs::symlink;
        let (directory, state) = fixture(
            "[Leaf](../leaf.sql)\n\n[Parent](../escape/target.sql)\n\n[Hardlink](../hard.sql)",
        );
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("target.sql"), "outside fixture").unwrap();
        symlink(
            outside.path().join("target.sql"),
            directory.path().join("leaf.sql"),
        )
        .unwrap();
        symlink(outside.path(), directory.path().join("escape")).unwrap();
        fs::hard_link(
            outside.path().join("target.sql"),
            directory.path().join("hard.sql"),
        )
        .unwrap();
        for target in ["../leaf.sql", "../escape/target.sql", "../hard.sql"] {
            assert!(read(
                &state,
                request(&state, target, AssetReferenceKind::Attachment)
            )
            .is_err());
        }
        let stale = request(&state, "../leaf.sql", AssetReferenceKind::Attachment);
        let moved = directory.path().with_extension("retained");
        fs::rename(directory.path(), &moved).unwrap();
        fs::create_dir(directory.path()).unwrap();
        assert!(read(&state, stale).is_err());
        fs::remove_dir_all(moved).unwrap();
    }
}
