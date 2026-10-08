use super::*;
use mdreader_core::SourceRoot;
use std::{fs, path::PathBuf};
use tempfile::TempDir;

const BODY: &str = "# Selected";
#[path = "../../tests/support/review_archive.rs"]
mod fixture;
use fixture::archive;
struct Fixture {
    _temp: TempDir,
    profile: PathBuf,
    notes: PathBuf,
    stage: PathBuf,
    identity: ArchiveIdentity,
    owner: Arc<Mutex<ReaderStore>>,
    source: String,
    revision: String,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("profile");
        let notes = temp.path().join("notes");
        fs::create_dir(&notes).unwrap();
        fs::write(notes.join("current.md"), BODY).unwrap();
        let mut store = ReaderStore::open(&profile).unwrap();
        let source = store
            .add_local_source("Selected", SourceRoot::capture(notes.clone()).unwrap())
            .unwrap()
            .id;
        let revision = store
            .read_document(&source, "current.md", None)
            .unwrap()
            .revision;
        let (stage, identity) = archive(temp.path(), &revision);
        Self {
            _temp: temp,
            profile,
            notes,
            stage,
            identity,
            owner: Arc::new(Mutex::new(store)),
            source,
            revision,
        }
    }
    fn coordinator(&self) -> Coordinator {
        Coordinator::new(self.owner.clone(), &self.profile).unwrap()
    }
    fn select(&self, coordinator: &Coordinator) -> String {
        match coordinator
            .execute(ReviewRequest::Select {
                stage: self.stage.clone(),
                identity: self.identity.clone(),
            })
            .unwrap()
        {
            ReviewReply::Selected { handle, .. } => handle,
            _ => panic!("selection reply"),
        }
    }
    fn compare(&self, handle: &str, record: &str) -> ReviewRequest {
        ReviewRequest::Compare {
            handle: handle.into(),
            record_id: record.into(),
            source_id: self.source.clone(),
            path: "current.md".into(),
            revision: self.revision.clone(),
        }
    }
}
#[test]
fn selected_archive_preserves_unknown_and_rejected_rows_and_compares_exact_current_text() {
    let f = Fixture::new();
    let c = f.coordinator();
    let handle = f.select(&c);
    let before = fs::read(f.stage.join("reviews.sqlite")).unwrap();
    let ReviewReply::Listed { page } = c
        .execute(ReviewRequest::List {
            handle: handle.clone(),
            offset: 0,
            limit: 20,
        })
        .unwrap()
    else {
        panic!("page")
    };
    assert_eq!(page.records.len(), 3);
    assert_eq!(page.records[2].state, "unknown");
    assert!(page.records[2].unavailable_reason.is_some());
    assert_eq!(page.records[1].state, "rejected");
    assert_eq!(page.records[1].reviewer.as_deref(), Some("Reviewer"));
    assert!(!serde_json::to_string(&page).unwrap().contains("NEVER_OPEN"));
    let ReviewReply::Compared { comparison } = c
        .execute(f.compare(&handle, &page.records[1].record_id))
        .unwrap()
    else {
        panic!("comparison")
    };
    assert_eq!(comparison.recorded_current, "matches");
    assert_eq!(comparison.reviewed_current, "differs");
    assert_eq!(comparison.observation, "local_current_read");
    assert_eq!(comparison.current_passage, BODY);
    assert_eq!(comparison.review.graph_membership, "unavailable");
    assert_eq!(fs::read(f.stage.join("reviews.sqlite")).unwrap(), before);
    assert_eq!(
        fs::read(f.notes.join("current.md")).unwrap(),
        BODY.as_bytes()
    );
    assert!(matches!(
        c.execute(f.compare(&handle, &page.records[2].record_id)),
        Err(ReviewError::Unsupported)
    ));
}
#[test]
fn revoked_changed_and_replaced_authority_never_returns_a_current_comparison() {
    let f = Fixture::new();
    let c = f.coordinator();
    let handle = f.select(&c);
    let ReviewReply::Listed { page } = c
        .execute(ReviewRequest::List {
            handle: handle.clone(),
            offset: 0,
            limit: 1,
        })
        .unwrap()
    else {
        panic!("page")
    };
    let record = &page.records[0].record_id;
    let result = c.execute_with_gate(f.compare(&handle, record), || {
        assert!(matches!(
            c.execute(ReviewRequest::List {
                handle: handle.clone(),
                offset: 0,
                limit: 1
            }),
            Err(ReviewError::Busy)
        ));
        f.owner
            .lock()
            .unwrap()
            .set_source_enabled(&f.source, false)
            .unwrap();
    });
    assert!(matches!(result, Err(ReviewError::Unavailable)));
    f.owner
        .lock()
        .unwrap()
        .set_source_enabled(&f.source, true)
        .unwrap();
    assert!(matches!(
        c.execute_with_gate(f.compare(&handle, record), || {
            fs::write(f.notes.join("current.md"), "Changed").unwrap();
        }),
        Err(ReviewError::Conflict)
    ));
    fs::write(f.notes.join("current.md"), BODY).unwrap();
    let moved = f.notes.with_extension("old");
    fs::rename(&f.notes, moved).unwrap();
    fs::create_dir(&f.notes).unwrap();
    fs::write(f.notes.join("current.md"), BODY).unwrap();
    assert!(matches!(
        c.execute(f.compare(&handle, record)),
        Err(ReviewError::Unavailable)
    ));
    let moved = f.profile.with_extension("old");
    fs::rename(&f.profile, moved).unwrap();
    fs::create_dir(&f.profile).unwrap();
    assert!(matches!(
        c.execute(ReviewRequest::List {
            handle,
            offset: 0,
            limit: 1
        }),
        Err(ReviewError::Conflict)
    ));
}
#[test]
fn restart_reselection_and_future_schema_fail_closed_without_opening_legacy_locators() {
    let f = Fixture::new();
    let c = f.coordinator();
    let handle = f.select(&c);
    drop(c);
    let reopened = f.coordinator();
    assert!(matches!(
        reopened.execute(ReviewRequest::List {
            handle,
            offset: 0,
            limit: 1
        }),
        Err(ReviewError::Unavailable)
    ));
    let handle = f.select(&reopened);
    assert!(
        reopened
            .execute(ReviewRequest::List {
                handle,
                offset: 0,
                limit: 1
            })
            .is_ok()
    );
    f.owner
        .lock()
        .unwrap()
        .with_library_connection(|connection| {
            connection.execute("UPDATE mdreader_review_owner SET version=999", [])
        })
        .unwrap();
    assert!(matches!(
        Coordinator::new(f.owner.clone(), &f.profile),
        Err(ReviewError::Unsupported)
    ));
    let scope = Uuid::new_v4().to_string();
    f.owner.lock().unwrap().with_library_connection(|connection| {
        connection.execute_batch("DROP TABLE mdreader_review_owner; CREATE TABLE mdreader_review_owner(id INTEGER PRIMARY KEY,version INTEGER NOT NULL,owner_uuid TEXT NOT NULL);").unwrap();
        connection.execute("INSERT INTO mdreader_review_owner VALUES(1,1,?1)", [&scope]).unwrap();
    });
    assert!(matches!(
        Coordinator::new(f.owner.clone(), &f.profile),
        Err(ReviewError::Unsupported)
    ));
    // Even exact DDL is insufficient when a damaged file violates the singleton.
    f.owner
        .lock()
        .unwrap()
        .with_library_connection(|connection| {
            connection
                .execute_batch("DROP TABLE mdreader_review_owner;")
                .unwrap();
            connection
                .execute_batch(include_str!("migrations/001-owner.sql"))
                .unwrap();
            connection
                .execute_batch("PRAGMA ignore_check_constraints=ON;")
                .unwrap();
            connection
                .execute(
                    "INSERT INTO mdreader_review_owner VALUES(1,1,?1),(2,1,?1)",
                    [&scope],
                )
                .unwrap();
            connection
                .execute_batch("PRAGMA ignore_check_constraints=OFF;")
                .unwrap();
        });
    assert!(matches!(
        Coordinator::new(f.owner.clone(), &f.profile),
        Err(ReviewError::Unsupported)
    ));
    // A view with a convincing singleton row is never accepted as the owner table.
    f.owner.lock().unwrap().with_library_connection(|connection| {
        assert_eq!(connection.query_row("SELECT count(*) FROM mdreader_review_owner", [], |r| r.get::<_,u64>(0)).unwrap(), 2);
        connection.execute_batch("DROP TABLE mdreader_review_owner;").unwrap();
        connection.execute_batch(&format!("CREATE VIEW mdreader_review_owner AS SELECT 1 AS id,1 AS version,'{scope}' AS owner_uuid;")).unwrap();
    });
    assert!(matches!(
        Coordinator::new(f.owner.clone(), &f.profile),
        Err(ReviewError::Unsupported)
    ));
    let kind = f
        .owner
        .lock()
        .unwrap()
        .with_library_connection(|connection| {
            connection
                .query_row(
                    "SELECT type FROM sqlite_schema WHERE name='mdreader_review_owner'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .unwrap()
        });
    assert_eq!(kind, "view");
}

#[test]
fn provider_revision_is_a_cached_observation_and_revoke_between_reads_fences_it() {
    use mdreader_provider::{
        AccountBinding, DisplayText, DocumentBody, DocumentVersion, LibraryId, MarkdownBytes,
        Metadata, OpaqueValue, RecordKey, SourceBinding, SourceId,
    };
    let f = Fixture::new();
    let c = f.coordinator();
    let handle = f.select(&c);
    let library = LibraryId::new("synthetic-library").unwrap();
    let binding = SourceBinding::new(
        library.clone(),
        SourceId::new("selected-provider").unwrap(),
        AccountBinding::new("synthetic-account").unwrap(),
        Uuid::new_v4(),
        "1".repeat(64),
    )
    .unwrap();
    let projection =
        mdreader_core::ProviderProjection::open(&f.profile.join("provider-cache"), library)
            .unwrap();
    let key = RecordKey::new("opaque-selected-key").unwrap();
    let version = DocumentVersion {
        provider_revision: OpaqueValue::new("remote-version-different-from-body-hash").unwrap(),
        content_hash: None,
        conditional_write_token: None,
    };
    let metadata = Metadata {
        key: key.clone(),
        name: DisplayText::new("Selected provider").unwrap(),
        display_path: DisplayText::new("Display only").unwrap(),
        media_type: None,
        modified_at_ms: None,
        size_bytes: Some(BODY.len() as u64),
        version: version.clone(),
    };
    let document = DocumentBody {
        key,
        version,
        body: MarkdownBytes::new(BODY.as_bytes().to_vec()).unwrap(),
        observed_at_ms: 1,
    };
    let receipt = f
        .owner
        .lock()
        .unwrap()
        .with_library_connection(|connection| {
            let tx = connection.transaction().unwrap();
            let receipt = projection
                .admit(&tx, &binding, &metadata, &document)
                .unwrap();
            tx.commit().unwrap();
            receipt
        });
    assert_ne!(receipt.revision, f.revision);
    let ReviewReply::Listed { page } = c
        .execute(ReviewRequest::List {
            handle: handle.clone(),
            offset: 0,
            limit: 1,
        })
        .unwrap()
    else {
        panic!("page")
    };
    let request = || ReviewRequest::Compare {
        handle: handle.clone(),
        record_id: page.records[0].record_id.clone(),
        source_id: receipt.reader_source_id.clone(),
        path: receipt.path.clone(),
        revision: receipt.revision.clone(),
    };
    let ReviewReply::Compared { comparison } = c.execute(request()).unwrap() else {
        panic!("comparison")
    };
    assert_eq!(comparison.observation, "provider_cached_revision");
    assert_eq!(comparison.recorded_current, "matches");
    assert_eq!(comparison.current_passage, BODY);
    assert!(matches!(
        c.execute_with_gate(request(), || {
            f.owner
                .lock()
                .unwrap()
                .with_library_connection(|connection| {
                    let tx = connection.transaction().unwrap();
                    projection.revoke(&tx, &binding).unwrap();
                    tx.commit().unwrap();
                });
        }),
        Err(ReviewError::Unavailable)
    ));
}
