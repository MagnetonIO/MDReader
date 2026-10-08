//! Real owner/graph/receipt storage; these private seams make no OIDC claim.
use super::*;
use crate::sync::{
    incoming::outgoing_selection::{self, Admission},
    outgoing::{Intent, association, scope_digest, snapshot::Snapshot},
};
use mdreader_sync::outgoing::{OutgoingContent, SealedGraph, seal_content};

struct Local {
    f: Fixture,
    current: publication::VerifiedCurrent,
    snapshot: Snapshot,
    graph: SealedGraph,
    intent: Intent,
}
impl Local {
    fn new() -> Self {
        let mut f = Fixture::unstaged();
        let input = identity::document(&f.prepared(2, 1)).unwrap();
        let assets = f
            .runtime
            .assets(&mut f.owner, &f.temp.path().join("assets"))
            .unwrap();
        let ready = publication::reserve_new(
            &mut f.owner,
            &f.temp.path().join("documents"),
            &assets,
            &f.origin,
            &input,
        )
        .unwrap()
        .write(input)
        .unwrap()
        .validate(f.runtime.validation_permit().unwrap())
        .unwrap();
        let native = publication::with_publication(
            &mut f.owner,
            &ready,
            &f.origin,
            |db, guard| -> Result<_> {
                let tx = db.transaction()?;
                let receipt = publication::publish_new(&tx, &guard, &ready)?;
                publication::revalidate_publication(&tx, &guard, &ready)?;
                tx.commit()?;
                Ok(receipt.document)
            },
        )
        .unwrap()
        .unwrap();
        drop(ready);
        device::pin(&mut f.owner, &native.id, true).unwrap();
        device::position(&mut f.owner, &native.id, &native.revision, 3).unwrap();
        let context = EncryptionContext {
            library_id: f.scope.library_id().into(),
            item_id: Uuid::new_v4().to_string(),
            revision_id: Uuid::new_v4().to_string(),
            key_id: f.key().key_id().into(),
        };
        f.origin = publication::IncomingOrigin {
            scope_digest: scope_digest(&f.scope).unwrap(),
            item_id: Uuid::parse_str(&context.item_id).unwrap(),
            wire_revision: Uuid::parse_str(&context.revision_id).unwrap(),
            session_epoch: Uuid::new_v4(),
            selection_epoch: Uuid::new_v4(),
        };
        let current = publication::prepare_current(&mut f.owner, &native.id, &assets, &f.origin)
            .unwrap()
            .verify()
            .unwrap();
        let archive = publication::prepare_archive(&mut f.owner, &native.id, &assets, &f.runtime)
            .unwrap()
            .read()
            .unwrap();
        let snapshot = Snapshot::new(archive, &current).unwrap();
        let graph = seal_content(
            &f.scope,
            &f.key(),
            &context,
            OutgoingContent {
                title: &native.name,
                markdown: snapshot.archive.markdown(),
                assets: &snapshot.asset_inputs().unwrap(),
            },
        )
        .unwrap();
        let intent = Intent {
            operation: mdreader_sync::CommitOperation {
                operation_id: Uuid::new_v4().to_string(),
                item_id: context.item_id,
                revision_id: context.revision_id,
                parents: vec![],
                blob: Some(graph.root().blob_id().into()),
                deleted: false,
            },
            native_document: native.id,
            native_revision: native.revision,
            generation: current.state().generation,
            markdown_sha256: snapshot.markdown_sha256.clone(),
            assets_sha256: snapshot.assets_sha256.clone(),
            origin: f.origin.clone(),
            head: None,
        };
        Self {
            f,
            current,
            snapshot,
            graph,
            intent,
        }
    }
    fn admit(&mut self) -> Result<crate::sync::outgoing::QueuedDocument> {
        outgoing_selection::admit(
            &mut self.f.owner,
            &self.f.scope,
            Admission {
                intent: &self.intent,
                graph: &self.graph,
                current: &self.current,
                archive: &self.snapshot.archive,
            },
            |_| Ok(()),
        )
    }
    fn prepared(&self) -> PreparedIncoming {
        self.prepared_graph(&self.graph)
    }
    fn prepared_graph(&self, graph: &SealedGraph) -> PreparedIncoming {
        let root = graph.root();
        let mut change = self.change();
        change.blob = Some(root.blob_id().into());
        let remote = RemoteRevision {
            change,
            envelope_utf8: Some(root.envelope_utf8().into()),
        };
        let refs = graph
            .resources()
            .iter()
            .map(|r| EncryptedResource {
                blob_id: r.blob_id(),
                envelope_utf8: r.envelope_utf8(),
            })
            .collect::<Vec<_>>();
        incoming::prepare(&self.f.scope, &remote, &[&self.f.key()], &refs).unwrap()
    }
    fn change(&self) -> RemoteChange {
        let op = &self.intent.operation;
        RemoteChange {
            sequence: 1,
            revision_id: op.revision_id.clone(),
            item_id: op.item_id.clone(),
            blob: op.blob.clone(),
            deleted: false,
            parents: op.parents.clone(),
            device: self.f.scope.device_id().into(),
        }
    }
    fn stage(&mut self) {
        let change = self.change();
        let page = serde_json::to_vec(
            &json!({"changes":[change],"cursor":1,"highWater":1,"hasMore":false,"checkpoint":0}),
        )
        .unwrap();
        self.f.owner.with_library_connection(|db| {
            SyncStore::attach(db, self.f.scope.clone())
                .unwrap()
                .stage_changes(&self.f.scope, 200, 0, &page, &[self.graph.root().clone()])
                .unwrap()
        });
        let identity = identity::Identity::from_prepared(&self.prepared()).unwrap();
        self.f.owner.with_library_connection(|db| {
            let tx = db.transaction().unwrap();
            ledger::reserve(
                &tx,
                &identity::scope_key(&self.f.scope).unwrap(),
                &identity,
                &self.intent.origin,
            )
            .unwrap();
            ledger::associate(
                &tx,
                &identity::scope_key(&self.f.scope).unwrap(),
                &identity,
                &self.intent.origin,
                &self.intent.native_document,
            )
            .unwrap();
            tx.commit().unwrap();
        });
    }
    fn confirm(&mut self) {
        let receipt = |body: &PreparedRevision| {
            serde_json::to_vec(&json!({"v":2,"account":self.f.scope.account_digest(),"libraryId":self.f.scope.library_id(),"device":self.f.scope.device_id(),"operationId":self.intent.operation.operation_id,"blob":body.blob_id(),"bytes":body.envelope_utf8().len()})).unwrap()
        };
        let deps = self
            .graph
            .resources()
            .iter()
            .map(|r| (r.blob_id().to_string(), receipt(r)))
            .collect::<Vec<_>>();
        let root = receipt(self.graph.root());
        let commit=serde_json::to_vec(&json!({"operationId":self.intent.operation.operation_id,"revisionId":self.intent.operation.revision_id,"sequence":1,"conflict":false,"heads":[self.intent.operation.revision_id]})).unwrap();
        self.f.owner.with_library_connection(|db| {
            let mut sync = SyncStore::attach(db, self.f.scope.clone()).unwrap();
            for (blob, receipt) in deps {
                sync.mark_dependency_uploaded(
                    &self.f.scope,
                    &self.intent.operation.operation_id,
                    &blob,
                    201,
                    &receipt,
                )
                .unwrap();
            }
            sync.mark_uploaded_scoped(
                &self.f.scope,
                &self.intent.operation.operation_id,
                201,
                &root,
            )
            .unwrap();
            sync.commit_receipt(&self.f.scope, 201, &commit).unwrap();
        });
    }
    fn echo(
        &mut self,
        final_fence: impl Fn(&rusqlite::Transaction<'_>) -> Result<()>,
    ) -> Result<Receipt> {
        let identity = identity::Identity::from_prepared(&self.prepared()).unwrap();
        self_echo::commit(
            &mut self.f.owner,
            &self.f.scope,
            &identity,
            &self.intent.origin,
            self_echo::Echo {
                intent: &self.intent,
                current: &self.current,
                snapshot: &self.snapshot,
            },
            final_fence,
        )
    }
}
#[test]
fn selected_association_and_ciphertext_graph_rollback_and_reopen_as_one_unit() {
    let mut local = Local::new();
    let failed = outgoing_selection::admit(
        &mut local.f.owner,
        &local.f.scope,
        Admission {
            intent: &local.intent,
            graph: &local.graph,
            current: &local.current,
            archive: &local.snapshot.archive,
        },
        |_| Err(Error::Inactive),
    );
    assert!(matches!(failed, Err(Error::Inactive)));
    local.f.owner.with_library_connection(|db| {
        assert!(!crate::sync::outgoing::schema::present(db).unwrap());
        assert_eq!(
            db.query_row("SELECT count(*) FROM mdreader_sync_outbox", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM mdreader_sync_nonces", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
    });
    local.f.owner.with_library_connection(|db| {
        let tx = db.transaction().unwrap();
        tx.execute_batch("CREATE TABLE native_outgoing_future_v9(value TEXT)")
            .unwrap();
        let changes = tx.total_changes();
        assert!(matches!(
            crate::sync::outgoing::schema::initialize(&tx),
            Err(Error::Schema)
        ));
        assert_eq!(
            tx.total_changes(),
            changes,
            "future namespaces are not repaired"
        );
        // Roll back only this synthetic future namespace.
    });
    local.admit().unwrap();
    assert_eq!(local.f.count("device_documents_v1"), 1);
    assert_eq!(local.f.count("native_outgoing_intents_v1"), 1);
    assert_eq!(
        local.f.count("native_incoming_heads_v2"),
        0,
        "queue is not a remote head/apply"
    );
    let Local {
        mut f,
        intent,
        graph,
        current,
        snapshot,
    } = local;
    drop(current);
    drop(snapshot);
    drop(f.owner);
    f.owner = ReaderStore::open(&f.temp.path().join("data")).unwrap();
    f.runtime = AssetRuntime::default();
    f.owner.with_library_connection(|db| {
        let tx = db.transaction().unwrap();
        let stored = association::get(&tx, &f.scope, &intent.operation.operation_id)
            .unwrap()
            .unwrap();
        assert!(stored.intent == intent);
        assert_eq!(stored.applied, None);
        let entry =
            mdreader_sync::graph_entry_in(&tx, &f.scope, &intent.operation.operation_id).unwrap();
        assert_eq!(
            entry.envelope_utf8.as_deref(),
            Some(graph.root().envelope_utf8())
        );
        assert!(
            mdreader_sync::published_graph_in(
                &tx,
                &f.scope,
                &intent.operation.operation_id,
                &RemoteChange {
                    sequence: 1,
                    revision_id: intent.operation.revision_id.clone(),
                    item_id: intent.operation.item_id.clone(),
                    blob: intent.operation.blob.clone(),
                    deleted: false,
                    parents: vec![],
                    device: f.scope.device_id().into()
                }
            )
            .is_err()
        );
    });
    assert_eq!(f.cursor(), CursorState::default());
    let current = device::state::current(&mut f.owner, &intent.native_document).unwrap();
    assert_eq!(current.generation, intent.generation);
    assert!(current.document.pinned);
    assert_eq!(current.document.line, 3);
}
#[test]
fn exact_self_echo_receipts_apply_to_same_document_and_late_fence_rolls_back() {
    let mut local = Local::new();
    assert!(local.snapshot.matches_prepared(&local.prepared()));
    let mut changed_markdown = local.snapshot.archive.markdown().to_vec();
    changed_markdown.push(b' ');
    let changed = seal_content(
        &local.f.scope,
        &local.f.key(),
        &EncryptionContext {
            library_id: local.f.scope.library_id().into(),
            item_id: local.intent.operation.item_id.clone(),
            revision_id: local.intent.operation.revision_id.clone(),
            key_id: local.f.key().key_id().into(),
        },
        OutgoingContent {
            title: &local.current.state().document.name,
            markdown: &changed_markdown,
            assets: &local.snapshot.asset_inputs().unwrap(),
        },
    )
    .unwrap();
    assert!(
        !local
            .snapshot
            .matches_prepared(&local.prepared_graph(&changed))
    );
    local.admit().unwrap();
    local.stage();
    // Uploaded/ack status alone or an absent receipt can never authorize apply.
    assert!(local.echo(|_| Ok(())).is_err());
    assert_eq!(local.f.cursor().applied, 0);
    local.confirm();
    let change = local.change();
    local.f.owner.with_library_connection(|db| {
        let tx = db.transaction().unwrap();
        tx.execute(
            "UPDATE mdreader_sync_outbox SET receipt=NULL WHERE operation_id=?1",
            [&local.intent.operation.operation_id],
        )
        .unwrap();
        assert!(
            self_echo::publication_proof(&tx, &local.f.scope, &local.intent, &change).is_err(),
            "acknowledged status without exact receipt is not proof"
        );
        assert_eq!(
            association::get(&tx, &local.f.scope, &local.intent.operation.operation_id)
                .unwrap()
                .unwrap()
                .applied,
            None
        );
        // Whole-transaction rollback preserves the receipt, association and cursor.
    });
    assert_eq!(local.f.cursor().applied, 0);
    local.f.owner.with_library_connection(|db| {
        let tx = db.transaction().unwrap();
        for mismatch in 0..3 {
            let mut other = change.clone();
            match mismatch {
                0 => other.device = Uuid::new_v4().to_string(),
                1 => other.sequence += 1,
                _ => other.parents = vec![Uuid::new_v4().to_string()],
            }
            assert!(
                self_echo::publication_proof(&tx, &local.f.scope, &local.intent, &other).is_err()
            );
        }
        let mut operation = local.intent.operation.clone();
        operation.operation_id = Uuid::new_v4().to_string();
        tx.execute(
            "UPDATE mdreader_sync_outbox SET operation=?1 WHERE operation_id=?2",
            rusqlite::params![
                serde_json::to_string(&operation).unwrap(),
                local.intent.operation.operation_id
            ],
        )
        .unwrap();
        // Before commit receipts are present, an embedded operation still cannot
        // contradict the caller's exact indexed operation identity.
        tx.execute(
            "UPDATE mdreader_sync_outbox SET state='uploaded',receipt=NULL WHERE operation_id=?1",
            [&local.intent.operation.operation_id],
        )
        .unwrap();
        assert!(
            mdreader_sync::graph_entry_in(
                &tx,
                &local.f.scope,
                &local.intent.operation.operation_id
            )
            .is_err()
        );
        assert!(
            mdreader_sync::outbox_revision_in(
                &tx,
                &local.f.scope,
                &local.intent.operation.revision_id
            )
            .is_err()
        );
    });
    local.f.owner.with_library_connection(|db| {
        let mut sync = SyncStore::attach(db, local.f.scope.clone()).unwrap();
        let incomplete: std::result::Result<(), mdreader_sync::SyncError> = sync.with_applied_transaction(&change, |tx, _advance| {
            tx.execute("UPDATE native_outgoing_intents_v1 SET state='applied',sequence=1 WHERE operation_id=?1", [&local.intent.operation.operation_id])?;
            Ok(())
        });
        assert!(incomplete.is_err(), "publication cannot commit without consuming the exact cursor advance");
    });
    assert_eq!(local.f.cursor().applied, 0);
    local.f.owner.with_library_connection(|db| {
        assert_eq!(association::get(db, &local.f.scope, &local.intent.operation.operation_id).unwrap().unwrap().applied, None);
        for sql in [
            "DELETE FROM mdreader_sync_graph_dependencies WHERE operation_id=?1 AND ordinal=0",
            "UPDATE mdreader_sync_graph_dependencies SET rowid=rowid+100000 WHERE operation_id=?1 AND ordinal=0",
            "INSERT INTO mdreader_sync_graph_dependencies SELECT account,library,device,operation_id,ordinal+2,'ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff',context,envelope,bytes,receipt FROM mdreader_sync_graph_dependencies WHERE operation_id=?1 AND ordinal=0",
            "UPDATE mdreader_sync_graph_roots SET root_receipt=root_receipt||' ' WHERE operation_id=?1",
            "UPDATE mdreader_sync_graph_schema SET version=2 WHERE EXISTS(SELECT 1 FROM mdreader_sync_graph_roots WHERE operation_id=?1)",
        ] {
            let tx = db.transaction().unwrap();
            let proof = mdreader_sync::published_graph_proof_in(
                &tx, &local.f.scope, &local.intent.operation.operation_id, &change,
            ).unwrap();
            proof.revalidate().unwrap();
            tx.execute(sql, [&local.intent.operation.operation_id]).unwrap();
            assert!(proof.revalidate().is_err(), "changed graph membership/identity/receipt/schema is not reusable");
            // Drop proof before rolling back this isolated mutation.
        }
        db.execute_batch(&format!("CREATE TEMP TRIGGER self_echo_graph_mutation AFTER UPDATE OF applied ON mdreader_sync_cursors BEGIN UPDATE mdreader_sync_graph_dependencies SET context=context||' ' WHERE operation_id='{}' AND ordinal=0; END", local.intent.operation.operation_id)).unwrap();
    });
    assert!(
        local.echo(|_| Ok(())).is_err(),
        "post-cursor trigger changed validated raw context bytes"
    );
    assert_eq!(local.f.cursor().applied, 0);
    assert_eq!(local.f.count("native_incoming_heads_v2"), 0);
    local.f.owner.with_library_connection(|db| {
        assert_eq!(
            association::get(db, &local.f.scope, &local.intent.operation.operation_id)
                .unwrap()
                .unwrap()
                .applied,
            None
        );
        db.execute_batch("DROP TRIGGER self_echo_graph_mutation")
            .unwrap();
    });
    let before_assets = local
        .snapshot
        .materialized
        .iter()
        .map(|a| a.asset_id.clone())
        .collect::<Vec<_>>();
    assert!(matches!(
        local.echo(|_| Err(Error::Inactive)),
        Err(Error::Inactive)
    ));
    assert_eq!(local.f.cursor().applied, 0);
    assert_eq!(local.f.count("native_incoming_heads_v2"), 0);
    local.f.owner.with_library_connection(|db| {
        assert_eq!(
            association::get(db, &local.f.scope, &local.intent.operation.operation_id)
                .unwrap()
                .unwrap()
                .applied,
            None
        )
    });
    let receipt = local.echo(|_| Ok(())).unwrap();
    assert_eq!(receipt.document.id, local.intent.native_document);
    assert_eq!(receipt.document.revision, local.intent.native_revision);
    assert!(receipt.document.pinned);
    assert_eq!(receipt.document.line, 3);
    assert_eq!(
        receipt
            .materialized_assets
            .iter()
            .map(|a| a.asset_id.clone())
            .collect::<Vec<_>>(),
        before_assets
    );
    assert_eq!(local.f.count("device_documents_v1"), 1);
    assert_eq!(local.f.count("sources"), 1);
    assert_eq!(local.f.count("device_asset_items_v1"), 1);
    assert_eq!(local.f.cursor().applied, 1);
    assert_eq!(local.f.cursor().server_acknowledged, 0);
    let identity = identity::Identity::from_prepared(&local.prepared()).unwrap();
    assert_eq!(
        storage::reconcile(
            &mut local.f.owner,
            &local.f.scope,
            &identity,
            &local.intent.origin
        )
        .unwrap()
        .unwrap()
        .document
        .id,
        receipt.document.id
    );
    // Pin ABA cannot be erased by the still-exact raw file hash/old receipt.
    let mut dirty = Local::new();
    dirty.admit().unwrap();
    dirty.stage();
    dirty.confirm();
    device::pin(&mut dirty.f.owner, &dirty.intent.native_document, false).unwrap();
    device::pin(&mut dirty.f.owner, &dirty.intent.native_document, true).unwrap();
    assert!(dirty.echo(|_| Ok(())).is_err());
    assert_eq!(dirty.f.cursor().applied, 0);
    assert_eq!(dirty.f.count("device_documents_v1"), 1);
}
