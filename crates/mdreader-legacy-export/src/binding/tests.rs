use super::*;
use crate::{export, hex, reviews::*, Input, Limits, Role};
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::{cell::Cell, fs, os::unix::fs::PermissionsExt, path::Path};
use uuid::Uuid;

const TEXT: &str = "# Selected\r\n\r\nCited text\r\nElsewhere\r\n";
fn cited_hash(text: &str) -> String {
    format!("blake3:{}", blake3::hash(text.as_bytes()).to_hex())
}
fn fixture(root: &Path) -> (ReviewArchive, ArchiveIdentity, Vec<ArchivedReview>) {
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    let sqlite = root.join("proposals.db");
    let c = rusqlite::Connection::open(&sqlite).unwrap();
    c.execute_batch("CREATE TABLE proposal(fact_id TEXT PRIMARY KEY,kind TEXT,schema_name TEXT,extractor TEXT,rule TEXT,qualified TEXT,source_path TEXT,start_line INTEGER,end_line INTEGER,content_hash TEXT,endpoint_src TEXT,endpoint_dst TEXT,state TEXT,reviewer TEXT,reviewed_hash TEXT,payload TEXT); PRAGMA user_version=2;").unwrap();
    let recorded = cited_hash("# Selected\n\nCited text");
    let old = cited_hash("previously reviewed");
    for (index, reviewed) in [
        Some(recorded.as_str()),
        Some(old.as_str()),
        None,
        None,
        None,
        Some("sha256:unknown"),
    ]
    .into_iter()
    .enumerate()
    {
        let id = Uuid::new_v4().to_string();
        let hash = if index == 4 {
            "sha256:unknown"
        } else {
            &recorded
        };
        let payload = serde_json::json!({"id":id,"entity":"decision_overlay.Decision","value":{"decision_title":"Archive only"},"provenance":{"source":"../old/private.md","range":[1,3],"content_hash":hash,"schema_version":"1","extractor":"mdreader.decision@1","confidence":0.95,"approved_by":null}}).to_string();
        c.execute("INSERT INTO proposal VALUES(?1,'object','decision_overlay','mdreader.decision@1',?2,'decision_overlay.Decision','../old/private.md',1,3,?3,NULL,NULL,?4,'Reviewer',?5,?6)",params![id,if index == 3 {"FUTURE"} else {"D1"},hash,if index==1 {"rejected"} else {"approved"},reviewed,payload]).unwrap();
    }
    drop(c);
    let archive = root.join("export");
    let manifest = export(
        &[Input {
            role: Role::Proposals,
            path: sqlite,
        }],
        &archive,
        &Limits::default(),
    )
    .unwrap();
    let identity = ArchiveIdentity {
        export_id: manifest.export_id,
        manifest_sha256: hex(&Sha256::digest(
            fs::read(archive.join("manifest.json")).unwrap(),
        )),
    };
    let stage = root.join("reviews");
    stage_reviews(&archive, &identity, &stage, &Limits::default()).unwrap();
    let archive = ReviewArchive::open(&stage, &identity, &Limits::default()).unwrap();
    let records = archive.list(0, 100).unwrap().records;
    (archive, identity, records)
}

// Only the trusted owner boundary is injected. Archives/staging use real SQLite.
struct SelectedOwner {
    scope: Uuid,
    returned: CurrentDocumentReference,
    text: String,
    revalidation: Cell<Option<CurrentReadError>>,
    reads: Cell<usize>,
    final_reads: Cell<usize>,
    tamper_on_final: Option<std::path::PathBuf>,
}
impl SelectedOwner {
    fn new() -> Self {
        let scope = Uuid::new_v4();
        let returned = CurrentDocumentReference::new(
            scope,
            "explicit-local-source".into(),
            "renamed/selected.md".into(),
            blake3::hash(TEXT.as_bytes()).to_hex().to_string(),
        )
        .unwrap();
        Self {
            scope,
            returned,
            text: TEXT.into(),
            revalidation: Cell::new(None),
            reads: Cell::new(0),
            final_reads: Cell::new(0),
            tamper_on_final: None,
        }
    }
}
impl CurrentDocumentReader for SelectedOwner {
    fn owner_scope(&self) -> Uuid {
        self.scope
    }
    fn read_exact(
        &self,
        _: &CurrentDocumentReference,
    ) -> Result<CurrentDocument, CurrentReadError> {
        self.reads.set(self.reads.get() + 1);
        CurrentDocument::new(
            self.returned.clone(),
            self.text.clone(),
            if self.returned.source_id().starts_with("provider:") {
                CurrentObservation::ProviderCachedRevision
            } else {
                CurrentObservation::LocalCurrentRead
            },
        )
    }
    fn revalidate_exact(&self, _: &CurrentDocumentReference) -> Result<(), CurrentReadError> {
        self.final_reads.set(self.final_reads.get() + 1);
        if let Some(path) = &self.tamper_on_final {
            fs::write(path, b"synthetic archive mutation").unwrap();
        }
        self.revalidation.get().map_or(Ok(()), Err)
    }
}
fn selection(
    identity: &ArchiveIdentity,
    record: &ArchivedReview,
    owner: &SelectedOwner,
) -> SelectedBinding {
    SelectedBinding::new(
        identity.clone(),
        record.record_id.clone(),
        owner.returned.clone(),
    )
    .unwrap()
}

#[test]
fn normalized_span_comparisons_preserve_review_history_and_owner_observation() {
    let temp = tempfile::tempdir().unwrap();
    let (archive, identity, records) = fixture(temp.path());
    let mut owner = SelectedOwner::new();
    let before = fs::read(temp.path().join("reviews/reviews.sqlite")).unwrap();
    for (index, reviewed) in [
        CurrentHashComparison::Matches,
        CurrentHashComparison::Differs,
        CurrentHashComparison::Unknown,
    ]
    .into_iter()
    .enumerate()
    {
        let result = verify_binding(
            &archive,
            &selection(&identity, &records[index], &owner),
            &owner,
        )
        .unwrap();
        assert_eq!(result.recorded_current, CurrentHashComparison::Matches);
        assert_eq!(result.reviewed_current, reviewed);
        assert_eq!(result.archival_review.state, records[index].state);
        assert_eq!(
            result.archival_review.current_evidence,
            CurrentEvidence::SourceBindingRequired
        );
        assert_eq!(
            result.archival_review.graph_membership,
            GraphMembership::Unavailable
        );
        assert_eq!(
            result.archival_review.chronology,
            ReviewChronology::Unavailable
        );
        assert!(result.selected_current == owner.returned);
    }
    // CRLF and a terminal newline are normalized by the actual legacy producer.
    owner.text = "# Selected\n\nCited text\nDifferent uncited line".into();
    owner.returned = CurrentDocumentReference::new(
        owner.scope,
        owner.returned.source_id().into(),
        owner.returned.path().into(),
        blake3::hash(owner.text.as_bytes()).to_hex().to_string(),
    )
    .unwrap();
    let result =
        verify_binding(&archive, &selection(&identity, &records[0], &owner), &owner).unwrap();
    assert_eq!(result.recorded_current, CurrentHashComparison::Matches);
    // A provider revision is opaque and cannot be compared to a body BLAKE3.
    owner.returned = CurrentDocumentReference::new(
        owner.scope,
        format!("provider:{}", "b".repeat(64)),
        "records/opaque.md".into(),
        "c".repeat(64),
    )
    .unwrap();
    let result =
        verify_binding(&archive, &selection(&identity, &records[0], &owner), &owner).unwrap();
    assert!(result.observation == CurrentObservation::ProviderCachedRevision);
    assert_eq!(owner.final_reads.get(), 5);
    assert_eq!(
        fs::read(temp.path().join("reviews/reviews.sqlite")).unwrap(),
        before
    );
}

#[test]
fn explicit_scope_reference_span_and_final_authority_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let (archive, identity, records) = fixture(temp.path());
    let mut owner = SelectedOwner::new();
    let chosen = selection(&identity, &records[0], &owner);
    owner.scope = Uuid::new_v4();
    assert!(matches!(
        verify_binding(&archive, &chosen, &owner),
        Err(BindingError::OwnerMismatch)
    ));
    assert_eq!(owner.reads.get(), 0);
    owner.scope = owner.returned.owner_scope();
    owner.returned = CurrentDocumentReference::new(
        owner.scope,
        "another-source".into(),
        "other.md".into(),
        chosen.current().revision().into(),
    )
    .unwrap();
    assert!(matches!(
        verify_binding(&archive, &chosen, &owner),
        Err(BindingError::ReferenceMismatch)
    ));
    owner.returned = chosen.current().clone();
    for text in ["only one line\n", ""] {
        owner.text = text.into();
        owner.returned = CurrentDocumentReference::new(
            owner.scope,
            owner.returned.source_id().into(),
            owner.returned.path().into(),
            blake3::hash(owner.text.as_bytes()).to_hex().to_string(),
        )
        .unwrap();
        assert!(matches!(
            verify_binding(&archive, &selection(&identity, &records[0], &owner), &owner),
            Err(BindingError::SpanUnavailable)
        ));
    }
    assert_eq!(owner.final_reads.get(), 0);
    owner.text = TEXT.into();
    owner.returned = chosen.current().clone();
    for error in [CurrentReadError::Conflict, CurrentReadError::Unavailable] {
        owner.revalidation.set(Some(error));
        assert!(
            matches!(verify_binding(&archive, &chosen, &owner), Err(BindingError::Current(e)) if e==error)
        );
    }
    owner.revalidation.set(None);
    owner.text = "# Changed\r\n\r\nCited text\r\n".into();
    owner.returned = CurrentDocumentReference::new(
        owner.scope,
        owner.returned.source_id().into(),
        owner.returned.path().into(),
        blake3::hash(owner.text.as_bytes()).to_hex().to_string(),
    )
    .unwrap();
    let result =
        verify_binding(&archive, &selection(&identity, &records[0], &owner), &owner).unwrap();
    assert_eq!(result.recorded_current, CurrentHashComparison::Differs);
    for path in [
        "/absolute.md",
        "../old.md",
        "nested//bad.md",
        "C:\\private.md",
    ] {
        assert!(CurrentDocumentReference::new(
            owner.scope,
            "source".into(),
            path.into(),
            "a".repeat(64)
        )
        .is_err());
    }
    assert!(matches!(
        CurrentDocument::new(
            owner.returned.clone(),
            "x".repeat(MAX_CURRENT_DOCUMENT_BYTES + 1),
            CurrentObservation::LocalCurrentRead
        ),
        Err(CurrentReadError::Bounds)
    ));
}

#[test]
fn unknown_hashes_rules_and_archive_identity_cannot_become_current_matches() {
    let temp = tempfile::tempdir().unwrap();
    let (archive, identity, records) = fixture(temp.path());
    let mut owner = SelectedOwner::new();
    for record in &records[3..] {
        assert!(matches!(
            verify_binding(&archive, &selection(&identity, record, &owner), &owner),
            Err(BindingError::UnsupportedAssertion | BindingError::UnsupportedHash)
        ));
    }
    assert_eq!(owner.reads.get(), 0);
    let mut wrong = identity.clone();
    wrong.export_id = Uuid::new_v4();
    let wrong =
        SelectedBinding::new(wrong, records[0].record_id.clone(), owner.returned.clone()).unwrap();
    assert!(matches!(
        verify_binding(&archive, &wrong, &owner),
        Err(BindingError::ArchiveMismatch)
    ));
    struct Unintegrated(Uuid);
    impl CurrentDocumentReader for Unintegrated {
        fn owner_scope(&self) -> Uuid {
            self.0
        }
    }
    assert!(matches!(
        verify_binding(
            &archive,
            &selection(&identity, &records[0], &owner),
            &Unintegrated(owner.scope)
        ),
        Err(BindingError::Current(CurrentReadError::Unsupported))
    ));
    owner.tamper_on_final = Some(temp.path().join("reviews/reviews.sqlite"));
    assert!(matches!(
        verify_binding(&archive, &selection(&identity, &records[0], &owner), &owner),
        Err(BindingError::ArchiveUnavailable)
    ));
}
