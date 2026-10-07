use mdreader_core::{ReaderStore, SourceRoot};
use mdreader_sync::SyncScope;
#[test]
fn encrypted_sync_uses_the_reader_owner_and_preserves_durable_user_state() {
    let data = tempfile::tempdir().unwrap();
    let notes = tempfile::tempdir().unwrap();
    let mut owner = ReaderStore::open(data.path()).unwrap();
    let source = owner
        .add_local_source(
            "Notes",
            SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    owner.set_bookmark(&source.id, "plan.md", 7).unwrap();
    mdreader_service::sync::initialize(&mut owner).unwrap();
    let scope = SyncScope::new(
        "a".repeat(64),
        "11111111-1111-4111-8111-111111111111".into(),
        "A".repeat(43),
    )
    .unwrap();
    mdreader_service::sync::with_store(&mut owner, scope.clone(), |sync| {
        assert!(sync.pending(10)?.is_empty());
        Ok(())
    })
    .unwrap();
    drop(owner);
    let mut reopened = ReaderStore::open(data.path()).unwrap();
    assert_eq!(reopened.bookmark(&source.id, "plan.md").unwrap(), Some(7));
    mdreader_service::sync::with_store(&mut reopened, scope, |sync| {
        assert!(sync.pending(10)?.is_empty());
        Ok(())
    })
    .unwrap();
    assert!(data.path().join("library.sqlite").is_file());
    assert!(!data.path().join("sync.sqlite").exists());
}
