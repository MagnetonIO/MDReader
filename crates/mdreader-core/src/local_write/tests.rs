use super::*;
fn root() -> (tempfile::TempDir, SourceRoot) {
    let temp = tempfile::tempdir().unwrap();
    let root = SourceRoot::capture(temp.path().canonicalize().unwrap()).unwrap();
    (temp, root)
}
#[test]
fn exclusive_create_and_guarded_update_keep_external_bytes() {
    let (temp, root) = root();
    let first = PreparedWrite::stage(root.clone(), "note.md", None, "# First")
        .unwrap()
        .commit(|| Ok(()))
        .unwrap();
    assert!(PreparedWrite::stage(root.clone(), "note.md", None, "overwrite").is_err());
    let next = PreparedWrite::stage(root.clone(), "note.md", Some(&first), "# Second").unwrap();
    std::fs::write(temp.path().join("note.md"), "# External").unwrap();
    assert!(matches!(next.commit(|| Ok(())), Err(ReaderError::Conflict)));
    assert_eq!(
        std::fs::read_to_string(temp.path().join("note.md")).unwrap(),
        "# External"
    );
}
#[test]
fn create_race_never_overwrites_destination_and_cleans_stage() {
    let (temp, root) = root();
    let staged = PreparedWrite::stage(root, "note.md", None, "# Intended").unwrap();
    std::fs::write(temp.path().join("note.md"), "# External").unwrap();
    assert!(matches!(
        staged.commit(|| Ok(())),
        Err(ReaderError::Conflict)
    ));
    assert_eq!(
        std::fs::read_to_string(temp.path().join("note.md")).unwrap(),
        "# External"
    );
    assert!(!std::fs::read_dir(temp.path()).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".tmp")));
}
#[test]
fn revoked_fence_and_replaced_root_never_publish() {
    let (temp, root) = root();
    let staged = PreparedWrite::stage(root, "note.md", None, "# Intended").unwrap();
    assert!(staged.commit(|| Err(ReaderError::Cancelled)).is_err());
    assert!(!temp.path().join("note.md").exists());
}
