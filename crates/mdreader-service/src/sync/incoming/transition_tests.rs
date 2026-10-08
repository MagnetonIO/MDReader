//! Private orchestration/storage seam only; not an identity-verification fixture.
use super::*;
use crate::sync::incoming::{heads, transition_commit, transition_start};
use mdreader_sync::codec::{ReadingPosition, UserStatePayload};

fn stage_root(f: &mut Fixture, root: &RemoteRevision) {
    let bodies = if let Some(envelope) = &root.envelope_utf8 {
        vec![PreparedRevision::from_wire(
            &f.key(),
            &EncryptionContext {
                library_id: f.scope.library_id().into(),
                item_id: root.change.item_id.clone(),
                revision_id: root.change.revision_id.clone(),
                key_id: f.key().key_id().into(),
            },
            root.change.blob.as_ref().unwrap(),
            envelope,
        )
        .unwrap()]
    } else {
        vec![]
    };
    let page=serde_json::to_vec(&json!({"changes":[root.change],"cursor":root.change.sequence,"highWater":root.change.sequence,"hasMore":false,"checkpoint":0})).unwrap();
    f.owner.with_library_connection(|db| {
        SyncStore::attach(db, f.scope.clone())
            .unwrap()
            .stage_changes(&f.scope, 200, root.change.sequence - 1, &page, &bodies)
            .unwrap()
    });
}
fn state_input(
    f: &mut Fixture,
    item: &str,
    sequence: u64,
    target: &str,
    parents: Vec<String>,
    position: Option<ReadingPosition>,
) -> PreparedIncoming {
    let state = UserStatePayload {
        v: 1,
        kind: "mdreader-user-state".into(),
        document_id: target.into(),
        pinned: true,
        position,
    };
    let revision = Uuid::new_v4().to_string();
    let context = EncryptionContext {
        library_id: f.scope.library_id().into(),
        item_id: item.into(),
        revision_id: revision.clone(),
        key_id: f.key().key_id().into(),
    };
    let encrypted = encrypt(
        &f.key(),
        &context,
        &codec::encode_user_state(&state).unwrap(),
    )
    .unwrap();
    let root = RemoteRevision {
        change: RemoteChange {
            sequence,
            item_id: item.into(),
            revision_id: revision,
            blob: Some(encrypted.blob_id().into()),
            deleted: false,
            parents,
            device: f.scope.device_id().into(),
        },
        envelope_utf8: Some(encrypted.envelope_utf8().into()),
    };
    stage_root(f, &root);
    incoming::prepare(&f.scope, &root, &[&f.key()], &[]).unwrap()
}
fn tombstone(f: &mut Fixture, item: &str, sequence: u64, parent: String) -> PreparedIncoming {
    let root = RemoteRevision {
        change: RemoteChange {
            sequence,
            item_id: item.into(),
            revision_id: Uuid::new_v4().to_string(),
            blob: None,
            deleted: true,
            parents: vec![parent],
            device: f.scope.device_id().into(),
        },
        envelope_utf8: None,
    };
    stage_root(f, &root);
    incoming::prepare(&f.scope, &root, &[], &[]).unwrap()
}
fn replacement(f: &mut Fixture, sequence: u64, parent: String) -> PreparedIncoming {
    let revision = Uuid::new_v4().to_string();
    let key = f.key();
    for index in 0..2 {
        let vector = &mut f.vectors.vectors[index];
        let mut chunk = codec::decode_chunk(vector.plaintext_utf8.as_bytes()).unwrap();
        chunk.revision_id = revision.clone();
        vector.context.revision_id = Uuid::new_v4().to_string();
        rewrite(vector, &key, &codec::encode_chunk(&chunk).unwrap());
    }
    let mut content =
        codec::decode_content(f.vectors.vectors[2].plaintext_utf8.as_bytes()).unwrap();
    content.revision_id = revision.clone();
    for (index, reference) in [
        (0, &mut content.source.chunks[0]),
        (1, &mut content.assets[0].chunks[0]),
    ] {
        reference.revision_id = f.vectors.vectors[index].context.revision_id.clone();
        reference.blob = f.vectors.vectors[index].blob_sha256.clone();
    }
    f.vectors.vectors[2].context.revision_id = revision;
    rewrite(
        &mut f.vectors.vectors[2],
        &key,
        &codec::encode_content(&content).unwrap(),
    );
    let mut root = f.root(2, sequence);
    root.change.parents = vec![parent];
    stage_root(f, &root);
    let resources = f.vectors.vectors[..2]
        .iter()
        .map(|v| EncryptedResource {
            blob_id: &v.blob_sha256,
            envelope_utf8: v.envelope_utf8.as_bytes(),
        })
        .collect::<Vec<_>>();
    incoming::prepare(&f.scope, &root, &[&key], &resources).unwrap()
}
fn apply(f: &mut Fixture, input: &PreparedIncoming) -> Result<Outcome> {
    let identity = identity::Identity::from_prepared(input)?;
    f.origin = identity::origin(
        &f.scope,
        input,
        f.origin.session_epoch,
        f.origin.selection_epoch,
    )?;
    let materialization = storage::Materialization {
        documents: &f.temp.path().join("documents"),
        assets: &f.temp.path().join("assets"),
        runtime: &f.runtime,
    };
    if matches!(input.payload(), incoming::IncomingPayload::Content(_)) {
        let document = identity::document(input)?;
        match storage::start(
            &mut f.owner,
            &f.scope,
            &identity,
            &f.origin,
            &document,
            materialization,
        )? {
            storage::Start::Applied(receipt) => Ok(Outcome::Applied {
                receipt,
                replayed: true,
                index_queued: false,
            }),
            storage::Start::Pending(reason) => Ok(Outcome::Pending(reason)),
            storage::Start::Plan(plan) => {
                let ready = plan
                    .write(document)?
                    .validate(f.runtime.validation_permit().unwrap())?;
                Ok(Outcome::Applied {
                    receipt: Box::new(storage::commit(
                        &mut f.owner,
                        &f.scope,
                        &identity,
                        &f.origin,
                        &ready,
                        |_| Ok(()),
                    )?),
                    replayed: false,
                    index_queued: false,
                })
            }
            storage::Start::Replacement {
                plan,
                current,
                head,
            } => {
                let current = current.verify()?;
                let ready = plan
                    .write(document)?
                    .validate(f.runtime.validation_permit().unwrap())?;
                Ok(Outcome::Applied {
                    receipt: Box::new(storage::commit_replacement(
                        &mut f.owner,
                        &f.scope,
                        &identity,
                        &f.origin,
                        storage::Replacement {
                            ready: &ready,
                            current: &current,
                            head: &head,
                        },
                        |_| Ok(()),
                    )?),
                    replayed: false,
                    index_queued: false,
                })
            }
        }
    } else {
        match transition_start::start(
            &mut f.owner,
            &f.scope,
            &identity,
            &f.origin,
            materialization,
        )? {
            transition_start::Start::Applied(receipt) => Ok(Outcome::Applied {
                receipt,
                replayed: true,
                index_queued: false,
            }),
            transition_start::Start::Pending(reason) => Ok(Outcome::Pending(reason)),
            transition_start::Start::Plan(plan) => {
                let plan = plan.verify()?;
                if let Some(reason) = transition_commit::pending(&plan) {
                    return Ok(Outcome::Pending(reason));
                }
                Ok(Outcome::Applied {
                    receipt: Box::new(transition_commit::commit(
                        &mut f.owner,
                        &f.scope,
                        &identity,
                        &f.origin,
                        &plan,
                        |_| Ok(()),
                    )?),
                    replayed: false,
                    index_queued: false,
                })
            }
        }
    }
}
fn receipt(outcome: Outcome) -> Receipt {
    let Outcome::Applied { receipt, .. } = outcome else {
        panic!("applied expected")
    };
    *receipt
}
fn head(f: &mut Fixture, item: &str) -> heads::Head {
    f.owner.with_library_connection(|db| {
        let tx = db.transaction().unwrap();
        heads::get(&tx, &identity::scope_key(&f.scope).unwrap(), item)
            .unwrap()
            .unwrap()
    })
}
#[test]
fn replacement_history_head_and_native_retirement_commit_together_or_rollback() {
    let mut f = Fixture::new();
    let first = f.prepared(2, 1);
    let old = receipt(apply(&mut f, &first).unwrap());
    let state_item = Uuid::new_v4().to_string();
    let state = state_input(
        &mut f,
        &state_item,
        2,
        &old.wire_item_id,
        vec![],
        Some(ReadingPosition::Line {
            revision_id: old.wire_revision.clone(),
            line: 3,
        }),
    );
    receipt(apply(&mut f, &state).unwrap());
    let next = replacement(&mut f, 3, old.wire_revision.clone());
    f.owner.with_library_connection(|db|db.execute_batch("CREATE TRIGGER mutate_after_reservation AFTER INSERT ON native_incoming_items_v1 WHEN NEW.sequence=3 BEGIN UPDATE native_incoming_heads_v2 SET tombstone_revision=NEW.wire_revision WHERE kind='content'; END;").unwrap());
    assert!(
        apply(&mut f, &next).is_err(),
        "a later head must not replace the checked admission snapshot"
    );
    assert_eq!(f.cursor().applied, 2);
    assert_eq!(f.count("device_documents_v1"), 1);
    f.owner.with_library_connection(|db|db.execute_batch("DROP TRIGGER mutate_after_reservation;UPDATE native_incoming_heads_v2 SET tombstone_revision=NULL;").unwrap());
    f.owner.with_library_connection(|db|db.execute_batch("CREATE TRIGGER mutate_after_cursor AFTER UPDATE OF applied ON mdreader_sync_cursors WHEN NEW.applied=3 BEGIN UPDATE native_incoming_heads_v2 SET baseline_generation=0; END;").unwrap());
    assert!(apply(&mut f, &next).is_err());
    assert_eq!(f.cursor().applied, 2);
    assert_eq!(f.count("device_documents_v1"), 1);
    assert!(device::read(&mut f.owner, &old.document.id, None).is_ok());
    f.owner.with_library_connection(|db| {
        db.execute_batch("DROP TRIGGER mutate_after_cursor")
            .unwrap()
    });
    let new = receipt(apply(&mut f, &next).unwrap());
    assert_ne!(new.document.id, old.document.id);
    assert_ne!(new.document.source_id, old.document.source_id);
    assert_eq!(new.document.revision, old.document.revision);
    assert!(new.document.pinned);
    assert_eq!(new.document.line, 1);
    assert_eq!(f.count("native_incoming_items_v1"), 3);
    assert_eq!(f.count("device_documents_v1"), 2);
    assert_eq!(head(&mut f, &old.wire_item_id).revision, new.wire_revision);
    assert!(f
        .owner
        .set_source_enabled(&old.document.source_id, true)
        .is_err());
    f.reopen();
    assert_eq!(f.cursor().applied, 3);
    let replay = receipt(apply(&mut f, &first).unwrap());
    assert_eq!(replay.document.id, old.document.id);
    assert_eq!(f.count("device_documents_v1"), 2);
    let wrong = replacement(&mut f, 4, old.wire_revision.clone());
    assert!(matches!(
        apply(&mut f, &wrong).unwrap(),
        Outcome::Pending(PendingReason::HeadConflict)
    ));
    assert_eq!(f.cursor().applied, 3);
}
#[test]
fn exact_state_revision_generation_aba_and_state_item_deletion_do_not_resurrect() {
    let mut f = Fixture::new();
    let input = f.prepared(2, 1);
    let content = receipt(apply(&mut f, &input).unwrap());
    let item = Uuid::new_v4().to_string();
    let state = state_input(
        &mut f,
        &item,
        2,
        &content.wire_item_id,
        vec![],
        Some(ReadingPosition::Line {
            revision_id: content.wire_revision.clone(),
            line: 3,
        }),
    );
    let applied = receipt(apply(&mut f, &state).unwrap());
    assert!(applied.operation == Operation::UserState);
    assert_eq!(applied.generation, Some(1));
    assert_eq!(applied.document.line, 3);
    // The codec requires position; explicit JSON null resets to start rather
    // than being confused with an omitted field or implicit pin-only patch.
    let clear = state_input(
        &mut f,
        &item,
        3,
        &content.wire_item_id,
        vec![applied.wire_revision.clone()],
        None,
    );
    let cleared = receipt(apply(&mut f, &clear).unwrap());
    assert_eq!(cleared.document.line, 1);
    assert_eq!(cleared.generation, Some(2));
    let deleted = tombstone(&mut f, &item, 4, cleared.wire_revision.clone());
    let removed = receipt(apply(&mut f, &deleted).unwrap());
    assert!(removed.operation == Operation::UserStateDeleted);
    let current = device::state::current(&mut f.owner, &content.document.id).unwrap();
    assert!(current.document.pinned);
    assert_eq!(current.document.line, 1);
    assert_eq!(current.generation, 2);
    let resurrect = state_input(
        &mut f,
        &item,
        5,
        &content.wire_item_id,
        vec![removed.wire_revision.clone()],
        None,
    );
    assert!(matches!(
        apply(&mut f, &resurrect).unwrap(),
        Outcome::Pending(PendingReason::Tombstoned)
    ));
    assert_eq!(f.cursor().applied, 4);
    let mut dirty = Fixture::new();
    let input = dirty.prepared(2, 1);
    let content = receipt(apply(&mut dirty, &input).unwrap());
    device::pin(&mut dirty.owner, &content.document.id, true).unwrap();
    device::pin(&mut dirty.owner, &content.document.id, false).unwrap();
    let state = state_input(
        &mut dirty,
        &Uuid::new_v4().to_string(),
        2,
        &content.wire_item_id,
        vec![],
        None,
    );
    assert!(matches!(
        apply(&mut dirty, &state).unwrap(),
        Outcome::Pending(PendingReason::LocalStateChanged)
    ));
    assert_eq!(dirty.cursor().applied, 1);
}
#[test]
fn observed_content_tombstones_fence_dirty_state_and_clean_deletion_retains_originals() {
    let mut f = Fixture::new();
    let input = f.prepared(2, 1);
    let content = receipt(apply(&mut f, &input).unwrap());
    let deletion = tombstone(
        &mut f,
        &content.wire_item_id,
        2,
        content.wire_revision.clone(),
    );
    let deleted = receipt(apply(&mut f, &deletion).unwrap());
    assert!(deleted.operation == Operation::DocumentDeleted);
    assert!(deleted.retirement.is_some());
    assert!(device::read(&mut f.owner, &content.document.id, None).is_err());
    assert!(f
        .owner
        .set_source_enabled(&content.document.source_id, true)
        .is_err());
    assert_eq!(f.cursor().applied, 2);
    let assets = f
        .runtime
        .assets(&mut f.owner, &f.temp.path().join("assets"))
        .unwrap();
    let archived =
        publication::prepare_archive(&mut f.owner, &content.document.id, &assets, &f.runtime)
            .unwrap()
            .read()
            .unwrap();
    assert_eq!(archived.markdown(), f.vectors.source_utf8.as_bytes());
    drop(archived);
    f.reopen();
    assert!(receipt(apply(&mut f, &deletion).unwrap()).operation == Operation::DocumentDeleted);
    let state = state_input(
        &mut f,
        &Uuid::new_v4().to_string(),
        3,
        &content.wire_item_id,
        vec![],
        None,
    );
    assert!(matches!(
        apply(&mut f, &state).unwrap(),
        Outcome::Pending(PendingReason::Tombstoned)
    ));
    assert_eq!(f.cursor().applied, 2);
    let mut dirty = Fixture::new();
    let input = dirty.prepared(2, 1);
    let content = receipt(apply(&mut dirty, &input).unwrap());
    device::pin(&mut dirty.owner, &content.document.id, true).unwrap();
    let deletion = tombstone(
        &mut dirty,
        &content.wire_item_id,
        2,
        content.wire_revision.clone(),
    );
    assert!(matches!(
        apply(&mut dirty, &deletion).unwrap(),
        Outcome::Pending(PendingReason::LocalStateChanged)
    ));
    assert_eq!(dirty.cursor().applied, 1);
    assert_eq!(
        head(&mut dirty, &content.wire_item_id).tombstone.as_deref(),
        Some(deletion.change().revision_id.as_str())
    );
    dirty.reopen();
    assert!(head(&mut dirty, &content.wire_item_id).tombstone.is_some());
    assert!(
        device::read(&mut dirty.owner, &content.document.id, None)
            .unwrap()
            .document
            .pinned
    );
}
#[test]
fn v1_upgrade_preserves_raw_history_unknown_baseline_and_future_marker() {
    let mut f = Fixture::new();
    let input = f.prepared(2, 1);
    let first = receipt(apply(&mut f, &input).unwrap());
    let before=f.owner.with_library_connection(|db| {
        let (identity,origin,receipt):(String,String,String)=db.query_row("SELECT identity,origin,receipt FROM native_incoming_items_v1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        let mut legacy:serde_json::Value=serde_json::from_str(&receipt).unwrap();legacy.as_object_mut().unwrap().remove("generation");let legacy=serde_json::to_string(&legacy).unwrap();
        db.execute_batch("ALTER TABLE native_incoming_items_v1 RENAME TO upgrade_test_items;DROP TABLE native_incoming_schema_v1;DROP TABLE native_incoming_heads_v2;").unwrap();
        db.execute_batch(include_str!("migrations/001-incoming.sql")).unwrap();db.execute("INSERT INTO native_incoming_schema_v1 VALUES(1,1,?1)",[f.scope.library_id()]).unwrap();
        db.execute("INSERT INTO native_incoming_items_v1 SELECT scope,wire_revision,wire_item,sequence,identity,origin,document_id,state,?1 FROM upgrade_test_items",[&legacy]).unwrap();db.execute_batch("DROP TABLE upgrade_test_items").unwrap();(identity,origin,legacy)
    });
    let rolled_back: Result<()> = f.owner.with_library_connection(|db| {
        let tx = db.transaction()?;
        schema::initialize(&tx)?;
        Err(Error::Inactive)
    });
    assert!(rolled_back.is_err());
    let version: u64 = f
        .owner
        .with_library_connection(|db| {
            db.query_row("SELECT version FROM native_incoming_schema_v1", [], |r| {
                r.get(0)
            })
        })
        .unwrap();
    assert_eq!(version, 1);
    f.reopen();
    f.owner.with_library_connection(|db| {
        let tx = db.transaction().unwrap();
        schema::bind(&tx, Uuid::parse_str(f.scope.library_id()).unwrap()).unwrap();
        tx.commit().unwrap();
    });
    let after = f
        .owner
        .with_library_connection(|db| {
            db.query_row(
                "SELECT identity,origin,receipt FROM native_incoming_items_v1",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
        })
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(head(&mut f, &first.wire_item_id).generation, None);
    let next = replacement(&mut f, 2, first.wire_revision);
    assert!(matches!(
        apply(&mut f, &next).unwrap(),
        Outcome::Pending(PendingReason::BaselineUnknown)
    ));
    assert_eq!(f.cursor().applied, 1);
    assert_eq!(f.count("device_documents_v1"), 1);
    f.owner.with_library_connection(|db|db.execute_batch("PRAGMA ignore_check_constraints=ON;UPDATE native_incoming_schema_v1 SET version=3;PRAGMA ignore_check_constraints=OFF;").unwrap());
    let refused: Result<()> = f.owner.with_library_connection(|db| {
        let tx = db.transaction()?;
        schema::initialize(&tx)?;
        tx.commit()?;
        Ok(())
    });
    assert!(matches!(refused, Err(Error::Schema)));
    assert_eq!(f.count("native_incoming_items_v1"), 1);
}
