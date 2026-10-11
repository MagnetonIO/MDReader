use mdreader_core::{editor_drafts::*, ReaderError, ReaderStore};

fn key() -> DraftKey {
    DraftKey::Local {
        source_id: "public-locator-not-a-grant".into(),
        path: "/public/Note.md".into(),
    }
}
fn input(text: &str) -> DraftInput {
    DraftInput {
        base_revision: "original-exact-revision".into(),
        base_hash: Some("a".repeat(64)),
        title: "Public note.md".into(),
        markdown: text.into(),
    }
}
fn checkpoint(store: &mut ReaderStore, key: DraftKey, gen: u64, text: &str) -> DraftSlot {
    match store
        .editor_drafts(DraftRequest::Checkpoint {
            key,
            expected_generation: gen,
            draft: input(text),
        })
        .unwrap()
    {
        DraftReply::Slot { slot } => slot,
        _ => panic!("expected slot"),
    }
}
#[test]
fn prior_library_upgrade_reopen_preserves_originals_and_private_exact_draft() {
    let data = tempfile::tempdir().unwrap();
    let original = data.path().join("public-original.md");
    std::fs::write(&original, b"# Original\r\nunchanged\r\n").unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let before = store.with_library_connection(|db| {
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name GLOB 'reader_editor_draft*'",
                [],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
            0
        );
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, u64>(0))
            .unwrap()
    });
    assert!(store.list_sources().unwrap().is_empty());
    let slot = checkpoint(&mut store, key(), 0, "unsaved\r\nα😀\u{0}\"\\\n");
    let meta = slot.draft.unwrap();
    assert_eq!(meta.base_revision, "original-exact-revision");
    assert_eq!(meta.base_hash, Some("a".repeat(64)));
    assert!(store.list_sources().unwrap().is_empty());
    assert_eq!(
        store.with_library_connection(|db| db
            .query_row("PRAGMA user_version", [], |r| r.get::<_, u64>(0))
            .unwrap()),
        before
    );
    drop(store);
    let mut reopened = ReaderStore::open(data.path()).unwrap();
    match reopened
        .editor_drafts(DraftRequest::Read {
            draft_id: meta.draft_id,
            generation: meta.generation,
        })
        .unwrap()
    {
        DraftReply::Read { draft, markdown } => {
            assert_eq!(markdown, "unsaved\r\nα😀\u{0}\"\\\n");
            assert_eq!(
                draft.draft_hash,
                blake3::hash(markdown.as_bytes()).to_hex().to_string()
            );
        }
        _ => panic!("expected exact draft"),
    }
    assert_eq!(
        std::fs::read(&original).unwrap(),
        b"# Original\r\nunchanged\r\n"
    );
    let other = tempfile::tempdir().unwrap();
    let mut other_owner = ReaderStore::open(other.path()).unwrap();
    let unrelated = checkpoint(&mut other_owner, key(), 0, "other");
    let DraftReply::Listed { drafts } = reopened.editor_drafts(DraftRequest::List).unwrap() else {
        panic!("list")
    };
    assert_eq!(drafts.len(), 1);
    assert_ne!(drafts[0].draft_id, unrelated.draft_id);
    assert!(reopened
        .editor_drafts(DraftRequest::Read {
            draft_id: unrelated.draft_id,
            generation: unrelated.generation
        })
        .is_err());
}
#[test]
fn late_checkpoint_cannot_resurrect_discarded_or_overwrite_new_generation_after_reopen() {
    let data = tempfile::tempdir().unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let first = checkpoint(&mut store, key(), 0, "first unsaved");
    let second = checkpoint(&mut store, key(), first.generation, "latest unsaved");
    assert!(matches!(
        store.editor_drafts(DraftRequest::Checkpoint {
            key: key(),
            expected_generation: first.generation,
            draft: input("late older")
        }),
        Err(ReaderError::Conflict)
    ));
    assert!(matches!(
        store.editor_drafts(DraftRequest::Read {
            draft_id: first.draft_id.clone(),
            generation: first.generation
        }),
        Err(ReaderError::Conflict)
    ));
    let DraftReply::Slot { slot: tombstone } = store
        .editor_drafts(DraftRequest::Discard {
            draft_id: second.draft_id,
            expected_generation: second.generation,
        })
        .unwrap()
    else {
        panic!("slot")
    };
    assert!(tombstone.draft.is_none());
    drop(store);
    let mut store = ReaderStore::open(data.path()).unwrap();
    assert!(matches!(
        store.editor_drafts(DraftRequest::Checkpoint {
            key: key(),
            expected_generation: second.generation,
            draft: input("late after save/discard")
        }),
        Err(ReaderError::Conflict)
    ));
    let DraftReply::Listed { drafts } = store.editor_drafts(DraftRequest::List).unwrap() else {
        panic!("list")
    };
    assert!(drafts.is_empty());
    let DraftReply::Slot { slot } = store
        .editor_drafts(DraftRequest::Inspect { key: key() })
        .unwrap()
    else {
        panic!("slot")
    };
    assert_eq!(slot.generation, tombstone.generation);
    let next = checkpoint(
        &mut store,
        key(),
        slot.generation,
        "explicit new editor buffer",
    );
    assert_eq!(next.generation, slot.generation + 1);
}
#[test]
fn schema_future_version_shadow_and_producer_refuse_without_losing_drafts() {
    for sql in [
        "CREATE TEMP TABLE reader_editor_drafts_v1 AS SELECT * FROM main.reader_editor_drafts_v1",
        "CREATE TRIGGER unrelated AFTER UPDATE ON reader_editor_drafts_v1 BEGIN DELETE FROM reader_editor_drafts_v1; END",
        "CREATE TEMP TRIGGER unrelated AFTER UPDATE ON main.reader_editor_drafts_v1 BEGIN DELETE FROM reader_editor_drafts_v1; END",
        "ALTER TABLE reader_editor_draft_schema_v1 RENAME TO saved_schema; CREATE TABLE reader_editor_draft_schema_v1(singleton INTEGER PRIMARY KEY,version INTEGER); INSERT INTO reader_editor_draft_schema_v1 VALUES(1,2)",
    ] {
        let data = tempfile::tempdir().unwrap();
        let mut store = ReaderStore::open(data.path()).unwrap();
        let saved = checkpoint(&mut store,key(),0,"preserved");
        let before = store.with_library_connection(|db| {
            db.execute_batch(sql).unwrap();
            db.query_row("SELECT generation,body,metadata FROM main.reader_editor_drafts_v1",[],|r|Ok((r.get::<_,u64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).unwrap()
        });
        assert!(store.editor_drafts(DraftRequest::Discard { draft_id:saved.draft_id, expected_generation:saved.generation }).is_err(),"{sql}");
        let after = store.with_library_connection(|db| db.query_row("SELECT generation,body,metadata FROM main.reader_editor_drafts_v1",[],|r|Ok((r.get::<_,u64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).unwrap());
        assert_eq!(before,after,"{sql}");
    }
}
#[test]
fn draft_capacity_and_body_integrity_fail_closed_without_implicit_eviction() {
    let data = tempfile::tempdir().unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let first = checkpoint(&mut store, key(), 0, "draft");
    for n in 1..MAX_ACTIVE_DRAFTS {
        checkpoint(
            &mut store,
            DraftKey::Device {
                document_id: format!("public-{n}"),
            },
            0,
            "draft",
        );
    }
    assert!(store
        .editor_drafts(DraftRequest::Checkpoint {
            key: DraftKey::Device {
                document_id: "overflow".into()
            },
            expected_generation: 0,
            draft: input("overflow")
        })
        .is_err());
    let DraftReply::Listed { drafts } = store.editor_drafts(DraftRequest::List).unwrap() else {
        panic!("list")
    };
    assert_eq!(drafts.len(), MAX_ACTIVE_DRAFTS as usize);
    store.with_library_connection(|db| {
        db.execute(
            "UPDATE main.reader_editor_drafts_v1 SET body='changed' WHERE draft_id=?1",
            [&first.draft_id],
        )
        .unwrap();
    });
    assert!(matches!(
        store.editor_drafts(DraftRequest::Read {
            draft_id: first.draft_id,
            generation: first.generation
        }),
        Err(ReaderError::Conflict)
    ));
    assert!(store
        .editor_drafts(DraftRequest::Checkpoint {
            key: key(),
            expected_generation: 1,
            draft: input(&"x".repeat(MAX_DRAFT_BYTES + 1))
        })
        .is_err());
}
