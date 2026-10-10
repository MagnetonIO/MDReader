use super::index::{import_status_guarded, validate_import_root};
#[cfg(unix)]
use super::index::display_names_root;
use super::navigation::{fs_listing, wikilink_guard};
use super::*;
use mdreader_core::SourceRootIdentity;
use std::fs;
use tempfile::TempDir;
fn synthetic_db_path(base: &Path, _path: PathBuf, device: u64, inode: u64) -> PathBuf {
    crate::library::legacy_layout::workspace_index_path_for_stable_identity(
        base,
        &SourceRootIdentity::Unix { device, inode },
    )
}

#[test]
fn db_path_is_deterministic_and_isolated_from_the_corpus() {
    let base = Path::new("/data");
    let root = PathBuf::from("/Users/me/notes");
    let a = synthetic_db_path(base, root.clone(), 7, 11);
    let b = synthetic_db_path(base, root.clone(), 7, 11);
    assert_eq!(a, b, "same root maps to the same db file");
    assert!(a.starts_with("/data/workspaces"));
    assert!(a.ends_with("catdb.db"));
    // A different root maps somewhere else — never inside the corpus.
    let other = synthetic_db_path(base, PathBuf::from("/Users/me/other"), 7, 12);
    assert_ne!(a, other);
    assert!(!a.starts_with(&root));
}

#[test]
fn the_root_key_requires_object_identity_not_only_the_canonical_path() {
    let base = Path::new("/data");
    let admitted = synthetic_db_path(base, PathBuf::from("/Users/me/notes"), 7, 11);
    let replacement = synthetic_db_path(base, PathBuf::from("/Users/me/notes"), 7, 12);

    assert_ne!(
        admitted, replacement,
        "a replacement at the same path must not inherit the admitted index"
    );
}

#[cfg(unix)]
#[test]
fn distinct_native_roots_that_share_a_lossy_display_keep_distinct_indexes() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let base = Path::new("/data");
    let first = PathBuf::from("/").join(OsString::from_vec(vec![b'n', 0xff]));
    let second = PathBuf::from("/").join(OsString::from_vec(vec![b'n', 0xfe]));
    assert_eq!(first.to_string_lossy(), second.to_string_lossy());
    assert_ne!(
        synthetic_db_path(base, first, 7, 11),
        synthetic_db_path(base, second, 7, 12)
    );
}

#[test]
fn fs_listing_flags_gitignored_files_without_hiding_them() {
    let dir = TempDir::new().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    // A git repo so .gitignore applies.
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join(".gitignore"), "generated/\n").unwrap();
    fs::write(root.join("keep.md"), "# Keep").unwrap();
    fs::create_dir_all(root.join("generated")).unwrap();
    fs::write(root.join("generated/skip.md"), "# Skip").unwrap();

    let source = SourceRoot::capture(root.clone()).unwrap();
    let entries = fs_listing(&source, Path::new("")).unwrap();
    let by_name = |name: &str| entries.iter().find(|e| e.name == name).cloned();

    let keep = by_name("keep.md").expect("keep.md listed");
    assert!(!keep.ignored, "tracked file is not dimmed");
    let skip = by_name("skip.md").expect("gitignored file still listed (dimmed)");
    assert!(skip.ignored, "gitignored file is flagged");
}

fn workspace_dir() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    (dir, root)
}

#[test]
fn import_root_rejects_a_foreign_path_and_accepts_the_grant() {
    let (_g, granted) = workspace_dir();
    let (_f, foreign) = workspace_dir();
    let epoch = Epoch::new(
        SourceRoot::capture(granted.clone()).expect("capture granted root"),
        1,
    );

    // The granted root is accepted, as is a contained subdirectory.
    assert!(validate_import_root(&epoch, granted.to_str().unwrap()).is_ok());
    let sub = granted.join("sub");
    fs::create_dir_all(&sub).unwrap();
    assert!(validate_import_root(&epoch, sub.to_str().unwrap()).is_ok());

    // A foreign root is a hard error.
    assert!(validate_import_root(&epoch, foreign.to_str().unwrap()).is_err());
}

fn replaced_workspace() -> (TempDir, PathBuf, PathBuf, WorkspaceState) {
    let temp = TempDir::new().expect("tempdir");
    let granted = temp.path().join("notes");
    fs::create_dir(&granted).expect("create granted root");
    fs::write(granted.join("original.md"), "# original").expect("write original");
    let granted = fs::canonicalize(granted).expect("canonical granted root");
    let ws = WorkspaceState::granted(granted.clone());

    let moved = temp.path().join("moved-original");
    fs::rename(&granted, &moved).expect("move granted filesystem object");
    fs::create_dir(&granted).expect("create replacement at exact path");
    fs::write(granted.join("replacement.md"), "# replacement").expect("write replacement");
    (temp, granted, moved, ws)
}

#[test]
fn import_refuses_a_replacement_at_the_active_epochs_exact_path() {
    let (_temp, granted, moved, ws) = replaced_workspace();
    let epoch = ws.current().expect("active epoch");
    let import_error = validate_import_root(&epoch, granted.to_str().expect("utf8 path"))
        .expect_err("import must reject the replacement root");
    assert!(
        !import_error.is_empty() && import_error.contains("identity"),
        "import refusal must be explicit: {import_error}"
    );
    assert_eq!(
        fs::read_to_string(moved.join("original.md")).expect("original object remains"),
        "# original"
    );
}

#[test]
fn finish_revalidates_identity_even_when_the_same_epoch_arc_is_current() {
    let (_temp, _granted, moved, ws) = replaced_workspace();
    let epoch = ws.current().expect("active epoch");

    assert!(
        epoch.finish(&ws, ()).is_err(),
        "Arc identity alone must not publish a value after the admitted root was replaced"
    );
    assert_eq!(
        fs::read_to_string(moved.join("original.md")).expect("original object remains"),
        "# original"
    );
}

#[test]
fn import_status_surfaces_an_identity_change_instead_of_defaulting_to_idle() {
    let temp = TempDir::new().expect("tempdir");
    let granted = temp.path().join("notes");
    fs::create_dir(&granted).expect("create granted root");
    let granted = fs::canonicalize(granted).expect("canonical granted root");
    let ws = WorkspaceState::granted(granted.clone());
    let epoch = ws.current().expect("active epoch");
    record_status(
        &epoch,
        &ImportProgress {
            phase: "importing".into(),
            files_total: 3,
            ..ImportProgress::default()
        },
    );
    let moved = temp.path().join("moved-original");

    let error = import_status_guarded(&ws, || {
        fs::rename(&granted, &moved).expect("move admitted root");
        fs::create_dir(&granted).expect("create same-path replacement");
    })
    .expect_err("identity failure must not be rendered as idle");

    assert!(
        error.contains("identity"),
        "the typed identity refusal remains visible: {error}"
    );
}

#[cfg(unix)]
#[test]
fn the_lossy_display_of_a_native_epoch_root_resolves_back_to_the_epoch_root() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let native = OsString::from_vec(vec![b'n', b'o', b't', b'e', b's', b'-', 0xff]);
    // Some macOS filesystems reject creating this byte sequence, but an
    // epoch may originate on any Unix filesystem. This helper performs no
    // I/O for the exact-root case, so exercise the native identity directly.
    let native_root = PathBuf::from("/").join(native);
    let display = native_root.to_string_lossy();

    assert!(
            display_names_root(&native_root, &display),
            "the display string identifies the already-granted native root; it is never re-canonicalized as authority"
        );
}

#[test]
fn wikilink_guard_rejects_a_foreign_origin_document() {
    let (_g, granted) = workspace_dir();
    let (_f, foreign) = workspace_dir();
    let epoch = Epoch::new(
        SourceRoot::capture(granted.clone()).expect("capture granted root"),
        1,
    );

    // An origin document inside the grant (and the empty origin) are accepted.
    let inside = granted.join("note.md");
    fs::write(&inside, "# Note").unwrap();
    assert!(wikilink_guard(&epoch, inside.to_str().unwrap()).is_ok());
    assert!(wikilink_guard(&epoch, "").is_ok());

    // An origin document outside the grant is rejected.
    let outside = foreign.join("secret.md");
    fs::write(&outside, "# Secret").unwrap();
    assert!(wikilink_guard(&epoch, outside.to_str().unwrap()).is_err());
}

#[test]
fn active_import_is_coalesced_per_epoch() {
    let (_g, granted) = workspace_dir();
    let epoch = Epoch::new(
        SourceRoot::capture(granted).expect("capture granted root"),
        1,
    );

    // The first claim wins; a second for the SAME epoch coalesces (false →
    // the caller returns the live status instead of spawning a duplicate).
    assert!(claim_active(&epoch).unwrap());
    assert!(!claim_active(&epoch).unwrap());

    // Dropping the pass's guard frees the slot so a later run can re-claim.
    {
        let _guard = ActiveGuard {
            epoch: Arc::clone(&epoch),
        };
    }
    assert!(claim_active(&epoch).unwrap());
}
