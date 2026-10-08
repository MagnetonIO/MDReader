//! These private-seam tests prove owner orchestration/storage, NOT OIDC identity.
use super::*;
use mdreader_core::device;
use mdreader_sync::{
    blob_digest, codec, encrypt,
    incoming::{self, EncryptedResource},
    CursorState, EncryptionContext, LibraryKey, PreparedRevision, RemoteChange, RemoteRevision,
    SyncScope, SyncStore,
};
use serde::Deserialize;
use serde_json::json;
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Vector {
    context: EncryptionContext,
    plaintext_utf8: String,
    envelope_utf8: String,
    blob_sha256: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Vectors {
    source_utf8: String,
    vectors: Vec<Vector>,
}

/// Public crypto vectors deliberately omit the Markdown image target and carry
/// a structural-only PNG. Derive valid owner input without changing those vectors.
fn publication_vectors(vectors: &mut Vectors) {
    vectors.source_utf8.push_str("\r\n![Pixel](pixel.png)\r\n");
    let key = LibraryKey::from_bytes(
        vectors.vectors[2].context.key_id.clone(),
        Zeroizing::new(vec![0; 32]),
    )
    .unwrap();
    let png = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4,
        0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 252, 255, 31, 0, 3, 3,
        2, 0, 239, 162, 167, 91, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    for (index,data) in [(0,"77u_IyBQdWJsaWMgc3ludGhldGljIGZpeHR1cmUNCg0KRXhhY3QgTWFya2Rvd246IM-ALg0KDQohW1BpeGVsXShwaXhlbC5wbmcpDQo"),(1,"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8_x8AAwMCAO-ip1sAAAAASUVORK5CYII")] {
        let vector=&mut vectors.vectors[index];
        let mut chunk=codec::decode_chunk(vector.plaintext_utf8.as_bytes()).unwrap();
        chunk.data=data.into();
        rewrite(vector,&key,&codec::encode_chunk(&chunk).unwrap());
    }
    let mut content = codec::decode_content(vectors.vectors[2].plaintext_utf8.as_bytes()).unwrap();
    content.source.bytes = vectors.source_utf8.len();
    content.source.sha256 = blob_digest(vectors.source_utf8.as_bytes());
    content.source.chunks[0].bytes = content.source.bytes;
    content.source.chunks[0].sha256 = content.source.sha256.clone();
    content.source.chunks[0].blob = vectors.vectors[0].blob_sha256.clone();
    content.assets[0].sha256 = blob_digest(&png);
    content.assets[0].chunks[0].sha256 = content.assets[0].sha256.clone();
    content.assets[0].chunks[0].blob = vectors.vectors[1].blob_sha256.clone();
    rewrite(
        &mut vectors.vectors[2],
        &key,
        &codec::encode_content(&content).unwrap(),
    );
}
fn rewrite(vector: &mut Vector, key: &LibraryKey, plaintext: &[u8]) {
    let encrypted = encrypt(key, &vector.context, plaintext).unwrap();
    vector.plaintext_utf8 = std::str::from_utf8(plaintext).unwrap().into();
    vector.envelope_utf8 = std::str::from_utf8(encrypted.envelope_utf8())
        .unwrap()
        .into();
    vector.blob_sha256 = encrypted.blob_id().into();
}
struct Fixture {
    temp: tempfile::TempDir,
    owner: ReaderStore,
    runtime: AssetRuntime,
    scope: SyncScope,
    vectors: Vectors,
    origin: publication::IncomingOrigin,
}
impl Fixture {
    fn new() -> Self {
        let mut f = Self::unstaged();
        f.stage(2, 1);
        f
    }
    fn unstaged() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut owner = ReaderStore::open(&temp.path().join("data")).unwrap();
        crate::sync::initialize(&mut owner).unwrap();
        let mut vectors: Vectors = serde_json::from_str(include_str!(
            "../../../../mdreader-sync/fixtures/payload-v1.json"
        ))
        .unwrap();
        publication_vectors(&mut vectors);
        let scope = SyncScope::new(
            "a".repeat(64),
            vectors.vectors[2].context.library_id.clone(),
            "A".repeat(43),
        )
        .unwrap();
        let mut f = Self {
            temp,
            owner,
            runtime: AssetRuntime::default(),
            scope,
            vectors,
            origin: publication::IncomingOrigin {
                scope_digest: [0; 32],
                item_id: Uuid::new_v4(),
                wire_revision: Uuid::new_v4(),
                session_epoch: Uuid::new_v4(),
                selection_epoch: Uuid::new_v4(),
            },
        };
        f.owner.with_library_connection(|db| {
            let tx = db.transaction().unwrap();
            schema::bind(&tx, Uuid::parse_str(f.scope.library_id()).unwrap()).unwrap();
            tx.commit().unwrap();
        });
        let prepared = f.prepared(2, 1);
        f.origin = identity::origin(
            &f.scope,
            &prepared,
            f.origin.session_epoch,
            f.origin.selection_epoch,
        )
        .unwrap();
        f
    }
    fn key(&self) -> LibraryKey {
        LibraryKey::from_bytes(
            self.vectors.vectors[2].context.key_id.clone(),
            Zeroizing::new(vec![0; 32]),
        )
        .unwrap()
    }
    fn root(&self, index: usize, sequence: u64) -> RemoteRevision {
        let v = &self.vectors.vectors[index];
        RemoteRevision {
            change: RemoteChange {
                sequence,
                revision_id: v.context.revision_id.clone(),
                item_id: v.context.item_id.clone(),
                blob: Some(v.blob_sha256.clone()),
                deleted: false,
                parents: vec![],
                device: self.scope.device_id().into(),
            },
            envelope_utf8: Some(v.envelope_utf8.as_bytes().into()),
        }
    }
    fn prepared(&self, index: usize, sequence: u64) -> PreparedIncoming {
        let resources: Vec<_> = if index == 2 {
            self.vectors.vectors[..2]
                .iter()
                .map(|v| EncryptedResource {
                    blob_id: &v.blob_sha256,
                    envelope_utf8: v.envelope_utf8.as_bytes(),
                })
                .collect()
        } else {
            vec![]
        };
        incoming::prepare(
            &self.scope,
            &self.root(index, sequence),
            &[&self.key()],
            &resources,
        )
        .unwrap()
    }
    fn stage(&mut self, index: usize, sequence: u64) {
        let v = &self.vectors.vectors[index];
        let body = PreparedRevision::from_wire(
            &self.key(),
            &v.context,
            &v.blob_sha256,
            v.envelope_utf8.as_bytes(),
        )
        .unwrap();
        let change = self.root(index, sequence).change;
        let page=serde_json::to_vec(&json!({"changes":[change],"cursor":sequence,"highWater":sequence,"hasMore":false,"checkpoint":0})).unwrap();
        self.owner.with_library_connection(|db| {
            SyncStore::attach(db, self.scope.clone())
                .unwrap()
                .stage_changes(&self.scope, 200, sequence - 1, &page, &[body])
                .unwrap()
        });
    }
    fn start(&mut self, input: &PreparedIncoming) -> storage::Start {
        storage::start(
            &mut self.owner,
            &self.scope,
            &identity::Identity::from_prepared(input).unwrap(),
            &self.origin,
            &identity::document(input).unwrap(),
            storage::Materialization {
                documents: &self.temp.path().join("documents"),
                assets: &self.temp.path().join("assets"),
                runtime: &self.runtime,
            },
        )
        .unwrap()
    }
    fn ready(&mut self) -> publication::ReadyIncoming {
        let input = self.prepared(2, 1);
        let storage::Start::Plan(plan) = self.start(&input) else {
            panic!("plan required")
        };
        plan.write(identity::document(&input).unwrap())
            .unwrap()
            .validate(self.runtime.validation_permit().unwrap())
            .unwrap()
    }
    fn commit(&mut self, ready: &publication::ReadyIncoming) -> Result<Receipt> {
        let input = self.prepared(2, 1);
        storage::commit(
            &mut self.owner,
            &self.scope,
            &identity::Identity::from_prepared(&input).unwrap(),
            &self.origin,
            ready,
            |_| Ok(()),
        )
    }
    fn cursor(&mut self) -> CursorState {
        self.owner.with_library_connection(|db| {
            SyncStore::attach(db, self.scope.clone())
                .unwrap()
                .cursors()
                .unwrap()
        })
    }
    fn count(&mut self, table: &str) -> u64 {
        self.owner.with_library_connection(|db| {
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        })
    }
    fn reopen(&mut self) {
        self.owner = ReaderStore::open(&self.temp.path().join("data")).unwrap();
        self.runtime = AssetRuntime::default();
    }
}

#[test]
fn staged_graph_requires_verified_bodies_before_atomic_owner_projection() {
    let mut f = Fixture::new();
    f.reopen();
    let staged = f.owner.with_library_connection(|db| {
        SyncStore::attach(db, f.scope.clone())
            .unwrap()
            .staged(0, 1)
            .unwrap()
            .remove(0)
    });
    let key = f.key();
    let blobs = incoming::dependencies(&f.scope, &staged, &[&key]).unwrap();
    assert_eq!(blobs.len(), 2);
    // Simulate arbitrary download order, resolving only authenticated hashes.
    let bodies = blobs
        .iter()
        .rev()
        .map(|blob| {
            let vector = f
                .vectors
                .vectors
                .iter()
                .find(|v| &v.blob_sha256 == blob)
                .unwrap();
            EncryptedResource {
                blob_id: blob,
                envelope_utf8: vector.envelope_utf8.as_bytes(),
            }
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        incoming::prepare(&f.scope, &staged, &[&key], &bodies[..1]),
        Err(mdreader_sync::SyncError::NotReady)
    ));
    let mut corrupt = bodies[0].envelope_utf8.to_vec();
    corrupt[0] ^= 1;
    let tampered = [
        EncryptedResource {
            blob_id: bodies[0].blob_id,
            envelope_utf8: &corrupt,
        },
        EncryptedResource {
            blob_id: bodies[1].blob_id,
            envelope_utf8: bodies[1].envelope_utf8,
        },
    ];
    assert!(matches!(
        incoming::prepare(&f.scope, &staged, &[&key], &tampered),
        Err(mdreader_sync::SyncError::Authentication)
    ));
    let prepared = incoming::prepare(&f.scope, &staged, &[&key], &bodies).unwrap();
    assert_eq!(
        f.cursor(),
        CursorState {
            received: 1,
            applied: 0,
            server_acknowledged: 0
        }
    );
    let storage::Start::Plan(plan) = f.start(&prepared) else {
        panic!("expected selected publication plan")
    };
    let ready = plan
        .write(identity::document(&prepared).unwrap())
        .unwrap()
        .validate(f.runtime.validation_permit().unwrap())
        .unwrap();
    let receipt = f.commit(&ready).unwrap();
    assert_eq!(
        f.cursor(),
        CursorState {
            received: 1,
            applied: 1,
            server_acknowledged: 0
        }
    );
    f.reopen();
    assert_eq!(
        device::read(
            &mut f.owner,
            &receipt.document.id,
            Some(&receipt.document.revision)
        )
        .unwrap()
        .markdown,
        f.vectors.source_utf8
    );
    assert_eq!(f.cursor().applied, 1);
    assert_eq!(
        f.cursor().server_acknowledged,
        0,
        "local projection is not remote acknowledgment"
    );
}

#[test]
fn exact_content_assets_receipt_cursor_and_replay_survive_reopen() {
    let mut f = Fixture::new();
    let ready = f.ready();
    let receipt = f.commit(&ready).unwrap();
    assert_eq!(receipt.sequence, 1);
    assert_eq!(receipt.document.name, "Public synthetic π.md");
    assert_ne!(receipt.document.id, receipt.wire_item_id);
    assert_ne!(receipt.document.revision, receipt.wire_revision);
    assert_eq!(
        device::read(
            &mut f.owner,
            &receipt.document.id,
            Some(&receipt.document.revision)
        )
        .unwrap()
        .markdown,
        f.vectors.source_utf8
    );
    let assets = f
        .runtime
        .assets(&mut f.owner, &f.temp.path().join("assets"))
        .unwrap();
    let manifest = assets
        .manifest(
            &mut f.owner,
            &receipt.document.id,
            &receipt.document.revision,
        )
        .unwrap();
    assert_eq!(manifest.assets.len(), 1);
    assert_eq!(manifest.assets[0].content_hash, receipt.assets[0].sha256);
    assert!(
        f.owner
            .index_job(&receipt.document.source_id)
            .unwrap()
            .is_none(),
        "publication must not index before commit"
    );
    drop(ready);
    f.reopen();
    assert_eq!(f.cursor().applied, 1);
    let prepared = f.prepared(2, 1);
    f.origin.session_epoch = Uuid::new_v4();
    let storage::Start::Applied(replayed) = f.start(&prepared) else {
        panic!("durable receipt required")
    };
    assert_eq!(replayed.document.id, receipt.document.id);
    assert!(storage::queue_index(&f.owner, &replayed));
    assert_eq!(f.count("device_documents_v1"), 1);
    assert_eq!(f.count("native_incoming_items_v1"), 1);
    f.owner
        .set_source_enabled(&receipt.document.source_id, false)
        .unwrap();
    let identity = identity::Identity::from_prepared(&prepared).unwrap();
    assert!(
        storage::reconcile(&mut f.owner, &f.scope, &identity, &f.origin)
            .unwrap()
            .is_some()
    );
    assert!(
        device::read(&mut f.owner, &receipt.document.id, None).is_err(),
        "reconciliation cannot regrant"
    );
}

#[test]
fn final_fence_and_post_cursor_source_revocation_roll_back_entire_publication() {
    let mut f = Fixture::new();
    let ready = f.ready();
    let identity = identity::Identity::from_prepared(&f.prepared(2, 1)).unwrap();
    assert!(matches!(
        storage::commit(&mut f.owner, &f.scope, &identity, &f.origin, &ready, |_| {
            Err(Error::Inactive)
        }),
        Err(Error::Inactive)
    ));
    assert_eq!(f.cursor().applied, 0);
    assert_eq!(f.count("device_documents_v1"), 0);
    f.owner.with_library_connection(|db|db.execute_batch("CREATE TRIGGER revoke_after_cursor AFTER UPDATE OF applied ON mdreader_sync_cursors BEGIN UPDATE sources SET enabled=0; END;").unwrap());
    assert!(
        f.commit(&ready).is_err(),
        "final core check must see SQL following publication"
    );
    assert_eq!(f.cursor().applied, 0);
    assert_eq!(f.count("device_documents_v1"), 0);
    f.owner.with_library_connection(|db| {
        db.execute_batch("DROP TRIGGER revoke_after_cursor;")
            .unwrap()
    });
    f.commit(&ready).unwrap();
    assert_eq!(f.cursor().applied, 1);
}

#[test]
fn old_session_or_selection_reservation_cannot_rebind_or_recover_as_local() {
    let mut f = Fixture::new();
    let input = f.prepared(2, 1);
    let storage::Start::Plan(plan) = f.start(&input) else {
        panic!("plan required")
    };
    let id = plan.document_id().to_owned();
    drop(plan);
    f.reopen();
    assert!(
        device::recover_imports(&mut f.owner, &f.temp.path().join("documents"))
            .unwrap()
            .documents
            .is_empty()
    );
    for session in [true, false] {
        let old = f.origin.clone();
        if session {
            f.origin.session_epoch = Uuid::new_v4();
        } else {
            f.origin.selection_epoch = Uuid::new_v4();
        }
        assert!(matches!(
            f.start(&input),
            storage::Start::Pending(PendingReason::ReauthorizationRequired)
        ));
        assert_eq!(f.count("device_incoming_intents_v1"), 1);
        f.origin = old;
    }
    let storage::Start::Plan(plan) = f.start(&input) else {
        panic!("same explicit origin retry required")
    };
    assert_eq!(plan.document_id(), id);
    assert_eq!(f.cursor().applied, 0);
    assert_eq!(f.count("device_documents_v1"), 0);
}

#[test]
fn exact_staged_identity_contiguous_sequence_and_payload_gates_preserve_cursor() {
    let mut f = Fixture::new();
    let input = f.prepared(2, 2);
    assert_eq!(
        storage::classify(&mut f.owner, &f.scope, input.change()).unwrap(),
        Some(PendingReason::Unstaged)
    );
    f.stage(3, 2);
    let state = f.prepared(3, 2);
    assert_eq!(
        identity::unsupported(&state),
        Some(PendingReason::PositionUnsupported)
    );
    assert_eq!(
        storage::classify(&mut f.owner, &f.scope, state.change()).unwrap(),
        Some(PendingReason::NotContiguous)
    );
    let mut revision = f.root(2, 1);
    revision.change.parents = vec![Uuid::new_v4().to_string()];
    let resources = f.vectors.vectors[..2]
        .iter()
        .map(|v| EncryptedResource {
            blob_id: &v.blob_sha256,
            envelope_utf8: v.envelope_utf8.as_bytes(),
        })
        .collect::<Vec<_>>();
    let parents = incoming::prepare(&f.scope, &revision, &[&f.key()], &resources).unwrap();
    assert_eq!(identity::unsupported(&parents), None);
    assert_eq!(
        storage::classify(&mut f.owner, &f.scope, parents.change()).unwrap(),
        Some(PendingReason::Unstaged)
    );
    revision.change.deleted = true;
    revision.change.blob = None;
    revision.change.parents.clear();
    revision.envelope_utf8 = None;
    let tombstone = incoming::prepare(&f.scope, &revision, &[], &[]).unwrap();
    assert_eq!(identity::unsupported(&tombstone), None);
    assert_eq!(f.cursor().applied, 0);
    // Authenticated bytes alone cannot admit the content into a different scope.
    let other = SyncScope::new(
        "b".repeat(64),
        f.scope.library_id().into(),
        f.scope.device_id().into(),
    )
    .unwrap();
    let prepared = f.prepared(2, 1);
    assert_eq!(
        storage::classify(&mut f.owner, &other, prepared.change()).unwrap(),
        Some(PendingReason::Unstaged)
    );
}

#[test]
fn future_or_malformed_service_namespace_fails_without_resetting_rows() {
    let mut f = Fixture::new();
    let input = f.prepared(2, 1);
    drop(f.start(&input));
    let mismatch = f.owner.with_library_connection(|db| -> Result<()> {
        let tx = db.transaction()?;
        schema::bind(&tx, Uuid::new_v4())?;
        tx.commit()?;
        Ok(())
    });
    assert!(matches!(mismatch, Err(Error::Scope)));
    let snapshot = f.owner.with_library_connection(|db| {
        db.query_row(
            "SELECT identity||origin FROM native_incoming_items_v1",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
    });
    f.owner.with_library_connection(|db|db.execute_batch("PRAGMA ignore_check_constraints=ON;UPDATE native_incoming_schema_v1 SET version=3;PRAGMA ignore_check_constraints=OFF;").unwrap());
    let failed = f.owner.with_library_connection(|db| -> Result<()> {
        let tx = db.transaction()?;
        schema::initialize(&tx)?;
        tx.commit()?;
        Ok(())
    });
    assert!(matches!(failed, Err(Error::Schema)));
    assert_eq!(
        f.owner.with_library_connection(|db| db
            .query_row(
                "SELECT identity||origin FROM native_incoming_items_v1",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap()),
        snapshot
    );
    assert_eq!(f.cursor().applied, 0);
    assert_eq!(f.count("device_documents_v1"), 0);
    f.owner.with_library_connection(|db|db.execute_batch("PRAGMA ignore_check_constraints=ON;UPDATE native_incoming_schema_v1 SET version=2;PRAGMA ignore_check_constraints=OFF;ALTER TABLE native_incoming_items_v1 RENAME COLUMN identity TO unknown_identity;").unwrap());
    let malformed = f.owner.with_library_connection(|db| -> Result<()> {
        let tx = db.transaction()?;
        schema::initialize(&tx)?;
        tx.commit()?;
        Ok(())
    });
    assert!(matches!(malformed, Err(Error::Schema)));
    assert_eq!(
        f.owner.with_library_connection(|db| db
            .query_row(
                "SELECT unknown_identity||origin FROM native_incoming_items_v1",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap()),
        snapshot
    );
}

#[path = "transition_tests.rs"]
mod transitions;

#[path = "../outgoing/tests.rs"]
mod outgoing;
