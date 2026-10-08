//! Real SQLite graphs share the caller's owner transaction; public synthetic bytes.
use super::*;
use crate::outgoing::{seal_content, OutgoingContent, SealedGraph};
use serde_json::json;
use zeroize::Zeroizing;
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
struct Fixture {
    _temp: tempfile::TempDir,
    path: std::path::PathBuf,
    db: Connection,
    scope: SyncScope,
    key: LibraryKey,
    context: EncryptionContext,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("owner.sqlite");
        let mut db = Connection::open(&path).unwrap();
        migrate(&mut db).unwrap();
        db.execute_batch("CREATE TABLE selected_owner_association(operation TEXT PRIMARY KEY,revision TEXT NOT NULL)").unwrap();
        let scope = SyncScope::new(
            "a".repeat(64),
            id(),
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
        )
        .unwrap();
        let key = LibraryKey::from_bytes(id(), Zeroizing::new(vec![0; 32])).unwrap();
        let context = EncryptionContext {
            library_id: scope.library_id.clone(),
            item_id: id(),
            revision_id: id(),
            key_id: key.key_id().into(),
        };
        Self {
            _temp: temp,
            path,
            db,
            scope,
            key,
            context,
        }
    }
    fn graph(&self) -> SealedGraph {
        seal_content(
            &self.scope,
            &self.key,
            &self.context,
            OutgoingContent {
                title: "Public selected.md",
                markdown: &vec![b'x'; 300_000],
                assets: &[],
            },
        )
        .unwrap()
    }
    fn admit(&mut self, op: &str, graph: &SealedGraph) {
        let tx = self.db.transaction().unwrap();
        tx.execute(
            "INSERT OR IGNORE INTO selected_owner_association VALUES(?1,?2)",
            params![op, self.context.revision_id],
        )
        .unwrap();
        enqueue_graph_in(&tx, &self.scope, &SyncLimits::default(), op, &[], graph).unwrap();
        tx.commit().unwrap();
    }
    fn reopen(&mut self) {
        self.db = Connection::open(&self.path).unwrap();
    }
    fn receipt(&self, op: &str, body: &PreparedRevision) -> Vec<u8> {
        serde_json::to_vec(&json!({"v":2,"account":self.scope.account_digest,"libraryId":self.scope.library_id,"device":self.scope.device_id,"operationId":op,"blob":body.blob_id(),"bytes":body.envelope_utf8().len()})).unwrap()
    }
}
#[test]
fn caller_rollback_and_reopen_preserve_exact_graph_and_reject_changed_dependencies() {
    let mut f = Fixture::new();
    let graph = f.graph();
    let op = id();
    {
        let tx = f.db.transaction().unwrap();
        tx.execute(
            "INSERT INTO selected_owner_association VALUES(?1,?2)",
            params![op, f.context.revision_id],
        )
        .unwrap();
        enqueue_graph_in(&tx, &f.scope, &SyncLimits::default(), &op, &[], &graph).unwrap();
        // Simulated final selected-owner fence failure: whole transaction drops.
    }
    f.reopen();
    assert!(!graph_schema::present(&f.db).unwrap());
    assert_eq!(
        f.db.query_row("SELECT count(*) FROM selected_owner_association", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        f.db.query_row("SELECT count(*) FROM mdreader_sync_nonces", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
    assert!(SyncStore::attach(&mut f.db, f.scope.clone())
        .unwrap()
        .entry(&op)
        .unwrap()
        .is_none());
    f.admit(&op, &graph);
    f.reopen();
    let deps = SyncStore::attach(&mut f.db, f.scope.clone())
        .unwrap()
        .dependencies(&op)
        .unwrap();
    assert_eq!(deps.len(), 2);
    for (a, b) in deps.iter().zip(graph.resources()) {
        assert_eq!(a.envelope_utf8, b.envelope_utf8());
        assert_eq!(a.blob, b.blob_id());
    }
    f.admit(&op, &graph); // Exact retry does not reserve fresh nonces.
    assert_eq!(
        f.db.query_row("SELECT count(*) FROM mdreader_sync_nonces", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        3
    );
    // Even unchanged ciphertext cannot be replayed under changed stored AAD.
    {
        let tx = f.db.transaction().unwrap();
        let mut context = graph.resources()[0].context.clone();
        context.item_id = id();
        tx.execute("UPDATE mdreader_sync_graph_dependencies SET context=?1 WHERE operation_id=?2 AND ordinal=0", params![serde_json::to_string(&context).unwrap(),op]).unwrap();
        assert!(matches!(
            enqueue_graph_in(&tx, &f.scope, &SyncLimits::default(), &op, &[], &graph),
            Err(SyncError::Reused)
        ));
        // Dropping the transaction restores the exact original context.
    }
    let mut changed = f.graph();
    {
        let tx = f.db.transaction().unwrap();
        assert!(matches!(
            enqueue_graph_in(&tx, &f.scope, &SyncLimits::default(), &op, &[], &changed),
            Err(SyncError::Reused)
        ));
    }
    changed.root = graph.root.clone();
    changed.resources = graph.resources.clone();
    changed.resources.pop();
    {
        let tx = f.db.transaction().unwrap();
        assert!(matches!(
            enqueue_graph_in(&tx, &f.scope, &SyncLimits::default(), &op, &[], &changed),
            Err(SyncError::Reused)
        ));
    }
    assert_eq!(
        SyncStore::attach(&mut f.db, f.scope.clone())
            .unwrap()
            .dependencies(&op)
            .unwrap()
            .len(),
        2
    );
    let context = EncryptionContext {
        item_id: id(),
        revision_id: id(),
        ..f.context.clone()
    };
    let mut collision = seal_content(
        &f.scope,
        &f.key,
        &context,
        OutgoingContent {
            title: "Public collision.md",
            markdown: &vec![b'x'; 300_000],
            assets: &[],
        },
    )
    .unwrap();
    // Reusing any previously reserved key/nonce, even in another operation,
    // rejects the whole new graph. No unreserved ciphertext becomes visible.
    collision.resources[0] = graph.resources[0].clone();
    {
        let tx = f.db.transaction().unwrap();
        assert!(matches!(
            enqueue_graph_in(
                &tx,
                &f.scope,
                &SyncLimits::default(),
                &id(),
                &[],
                &collision
            ),
            Err(SyncError::Reused)
        ));
    }
    assert_eq!(
        f.db.query_row("SELECT count(*) FROM mdreader_sync_outbox", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        f.db.query_row("SELECT count(*) FROM mdreader_sync_nonces", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        3
    );
}
#[test]
fn only_exact_scoped_dependency_and_root_receipts_allow_publication_and_survive_reopen() {
    let mut f = Fixture::new();
    let graph = f.graph();
    let op = id();
    f.admit(&op, &graph);
    let root_receipt = f.receipt(&op, graph.root());
    {
        let mut s = SyncStore::attach(&mut f.db, f.scope.clone()).unwrap();
        assert!(matches!(
            s.mark_uploaded_scoped(&f.scope, &op, 201, &root_receipt),
            Err(SyncError::NotReady)
        ));
        assert!(matches!(s.mark_uploaded(&f.scope,&op,201,&serde_json::to_vec(&json!({"blob":graph.root().blob_id(),"bytes":graph.root().envelope_utf8().len()})).unwrap()),Err(SyncError::Scope)));
    }
    // A receipt for one exact dependency does not read/hash sibling bodies.
    // A malformed sibling still prevents the full root publication proof.
    f.db.execute("UPDATE mdreader_sync_graph_dependencies SET context=?1 WHERE operation_id=?2 AND ordinal=1",params!["é".repeat(700),op]).unwrap();
    {
        let resource = &graph.resources()[0];
        let receipt = f.receipt(&op, resource);
        let mut s = SyncStore::attach(&mut f.db, f.scope.clone()).unwrap();
        s.mark_dependency_uploaded(&f.scope, &op, resource.blob_id(), 201, &receipt)
            .unwrap();
        assert!(matches!(
            s.mark_uploaded_scoped(&f.scope, &op, 201, &root_receipt),
            Err(SyncError::Schema)
        ));
    }
    f.db.execute("UPDATE mdreader_sync_graph_dependencies SET context=?1 WHERE operation_id=?2 AND ordinal=1",params![serde_json::to_string(&graph.resources()[1].context).unwrap(),op]).unwrap();
    for resource in graph.resources() {
        let receipt = f.receipt(&op, resource);
        let mut wrong: serde_json::Value = serde_json::from_slice(&receipt).unwrap();
        wrong["operationId"] = json!(id());
        let mut s = SyncStore::attach(&mut f.db, f.scope.clone()).unwrap();
        assert!(s
            .mark_dependency_uploaded(
                &f.scope,
                &op,
                resource.blob_id(),
                201,
                &serde_json::to_vec(&wrong).unwrap()
            )
            .is_err());
        s.mark_dependency_uploaded(&f.scope, &op, resource.blob_id(), 201, &receipt)
            .unwrap();
        s.mark_dependency_uploaded(&f.scope, &op, resource.blob_id(), 201, &receipt)
            .unwrap();
        // Semantically equivalent JSON is still a changed durable receipt.
        let mut changed_receipt = vec![b' '];
        changed_receipt.extend_from_slice(&receipt);
        assert!(matches!(
            s.mark_dependency_uploaded(&f.scope, &op, resource.blob_id(), 201, &changed_receipt),
            Err(SyncError::Reused)
        ));
    }
    let revision = graph.root().context.revision_id.clone();
    let commit = serde_json::to_vec(&json!({"operationId":op,"revisionId":revision,"sequence":1,"conflict":false,"heads":[revision]})).unwrap();
    {
        let mut s = SyncStore::attach(&mut f.db, f.scope.clone()).unwrap();
        assert!(s.commit_receipt(&f.scope, 201, &commit).is_err());
        s.mark_uploaded_scoped(&f.scope, &op, 201, &root_receipt)
            .unwrap();
        assert!(s.graph_root_uploaded(&op).unwrap());
        let mut changed_receipt = vec![b' '];
        changed_receipt.extend_from_slice(&root_receipt);
        assert!(matches!(
            s.mark_uploaded_scoped(&f.scope, &op, 201, &changed_receipt),
            Err(SyncError::Reused)
        ));
        s.commit_receipt(&f.scope, 201, &commit).unwrap();
    }
    f.reopen();
    let s = SyncStore::attach(&mut f.db, f.scope.clone()).unwrap();
    assert!(s.graph_root_uploaded(&op).unwrap());
    assert!(s.dependencies(&op).unwrap().iter().all(|d| d.uploaded));
    assert_eq!(
        s.entry(&op).unwrap().unwrap().state,
        OutboxState::Acknowledged
    );
    assert_eq!(
        s.cursors().unwrap(),
        CursorState::default(),
        "outgoing receipt is never an applied incoming cursor"
    );
}
#[test]
fn aggregate_quota_and_future_graph_schema_fail_without_partial_admission_or_reset() {
    let mut f = Fixture::new();
    let graph = f.graph();
    let op = id();
    let limits = SyncLimits {
        max_outbox_bytes: (graph.root().envelope_utf8().len() + 8192) as u64,
        ..SyncLimits::default()
    };
    {
        let tx = f.db.transaction().unwrap();
        tx.execute(
            "INSERT INTO selected_owner_association VALUES(?1,?2)",
            params![op, f.context.revision_id],
        )
        .unwrap();
        assert!(matches!(
            enqueue_graph_in(&tx, &f.scope, &limits, &op, &[], &graph),
            Err(SyncError::Bounds)
        ));
    }
    f.reopen();
    assert!(!graph_schema::present(&f.db).unwrap());
    assert_eq!(
        f.db.query_row("SELECT count(*) FROM mdreader_sync_outbox", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
    // Unknown future objects cannot masquerade as an empty lazy namespace.
    {
        let tx = f.db.transaction().unwrap();
        tx.execute_batch("CREATE TABLE mdreader_sync_graph_future(value TEXT)")
            .unwrap();
        assert!(matches!(
            enqueue_graph_in(&tx, &f.scope, &SyncLimits::default(), &op, &[], &graph),
            Err(SyncError::Schema)
        ));
    }
    f.admit(&op, &graph);
    // Missing roots with retained chunks must fail closed, never fall back to
    // a legacy root-only upload/commit channel. Caller rollback preserves them.
    {
        let tx = f.db.transaction().unwrap();
        tx.execute(
            "DELETE FROM mdreader_sync_graph_roots WHERE operation_id=?1",
            [&op],
        )
        .unwrap();
        assert!(matches!(
            graph::has_root(&tx, &f.scope, &op),
            Err(SyncError::Schema)
        ));
        assert!(matches!(
            graph_read::dependencies_on(&tx, &f.scope, &op),
            Err(SyncError::Schema)
        ));
    }
    {
        let tx = f.db.transaction().unwrap();
        tx.execute(
            "UPDATE mdreader_sync_outbox SET envelope=zeroblob(2097153) WHERE operation_id=?1",
            [&op],
        )
        .unwrap();
        assert!(matches!(
            entry_on(&tx, &f.scope, &op),
            Err(SyncError::Bounds)
        ));
    }
    f.db.execute("UPDATE mdreader_sync_graph_schema SET version=99", [])
        .unwrap();
    let another = id();
    {
        let tx = f.db.transaction().unwrap();
        assert!(matches!(
            enqueue_graph_in(&tx, &f.scope, &SyncLimits::default(), &another, &[], &graph),
            Err(SyncError::Schema)
        ));
    }
    f.reopen();
    assert_eq!(
        f.db.query_row("SELECT version FROM mdreader_sync_graph_schema", [], |r| {
            r.get::<_, u64>(0)
        })
        .unwrap(),
        99
    );
    assert_eq!(
        f.db.query_row(
            "SELECT count(*) FROM mdreader_sync_graph_dependencies",
            [],
            |r| r.get::<_, u64>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(
        f.db.query_row("SELECT count(*) FROM mdreader_sync_outbox", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        1
    );
}
