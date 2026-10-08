use super::*;
#[test]
fn final_fence_rejects_source_aba_and_index_commit_after_rows_were_observed() {
    let t = tempfile::tempdir().unwrap();
    let notes = t.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    std::fs::write(notes.join("plan.md"), "# Original").unwrap();
    let mut writer = ReaderStore::open(&t.path().join("profile")).unwrap();
    let id = writer
        .add_local_source(
            "Selected",
            crate::SourceRoot::capture(notes.clone()).unwrap(),
        )
        .unwrap()
        .id;
    writer.index_source(&id).unwrap();
    let reader = ReaderStore::open_readonly(&t.path().join("profile")).unwrap();
    let revoked = reader.catalog_page_checked(&id, None, None, 100, || {
        writer.set_source_enabled(&id, false).unwrap();
        writer.set_source_enabled(&id, true).unwrap();
    });
    assert!(matches!(revoked, Err(ReaderError::Conflict)));
    let changed = reader.catalog_page_checked(&id, None, None, 100, || {
        std::fs::write(notes.join("plan.md"), "# New indexed observation").unwrap();
        writer.index_source(&id).unwrap();
    });
    assert!(matches!(changed, Err(ReaderError::Conflict)));
    assert_eq!(
        reader.catalog_page(&id, None, None, 100).unwrap().documents[0].title,
        "New indexed observation"
    );
}
