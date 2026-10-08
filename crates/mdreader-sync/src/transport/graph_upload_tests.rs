//! Extends the existing real Worker/TLS fixture; no additional issuer/server run.
use super::tests::*;
use super::*;
use crate::outgoing::{seal_content, OutgoingContent};
use serde_json::json;
pub(super) async fn exercise(
    fixture: &Fixture,
    owner: TestOwner,
    auth: &TestAuthority,
    transport: &Transport,
    temp: &tempfile::TempDir,
) {
    let context = EncryptionContext {
        library_id: auth.scope.library_id().into(),
        item_id: uuid::Uuid::new_v4().to_string(),
        revision_id: uuid::Uuid::new_v4().to_string(),
        key_id: auth.library.key_id().into(),
    };
    let markdown = vec![b'x'; 300_000];
    let graph = seal_content(
        &auth.scope,
        &auth.library,
        &context,
        OutgoingContent {
            title: "Public graph.md",
            markdown: &markdown,
            assets: &[],
        },
    )
    .unwrap();
    let op = uuid::Uuid::new_v4().to_string();
    {
        let mut db = owner.db.lock().unwrap();
        let tx = db.transaction().unwrap();
        enqueue_graph_in(&tx, &auth.scope, &SyncLimits::default(), &op, &[], &graph).unwrap();
        tx.commit().unwrap();
    }
    let before = fixture.json("/__synthetic/control", json!({})).await;
    let initial_uploads = before["uploadHashes"].as_array().unwrap().len();
    let initial_commits = before["commitHashes"].as_array().unwrap().len();
    let partial = transport
        .run(
            &owner,
            auth,
            Budget {
                outgoing: 1,
                pages: 1,
                page_size: 5,
                transfer_bytes: graph.resources()[0].envelope_utf8().len() + 4096,
            },
        )
        .await
        .unwrap();
    assert_eq!(partial.committed, 0);
    assert!(partial.local_pending && partial.remote_pending);
    assert_eq!(
        fixture.json("/__synthetic/control", json!({})).await["commitHashes"]
            .as_array()
            .unwrap()
            .len(),
        initial_commits,
        "no root publication before every dependency receipt"
    );
    assert_eq!(
        owner
            .with_store(&auth.scope, |s| Ok(s
                .dependencies(&op)?
                .iter()
                .filter(|d| d.uploaded)
                .count()))
            .unwrap(),
        1
    );
    drop(owner);
    let owner = super::tests::owner(temp, &auth.scope);
    fixture
        .json("/__synthetic/control", json!({"dropUpload":true}))
        .await;
    assert!(matches!(
        transport.run(&owner, auth, Budget::default()).await,
        Err(TransportError::Retryable)
    ));
    assert_eq!(
        owner
            .with_store(&auth.scope, |s| Ok(s
                .dependencies(&op)?
                .iter()
                .filter(|d| d.uploaded)
                .count()))
            .unwrap(),
        1
    );
    drop(owner);
    let owner = super::tests::owner(temp, &auth.scope);
    let report = transport
        .run(&owner, auth, Budget::default())
        .await
        .unwrap();
    assert_eq!(
        (
            report.committed,
            report.received,
            report.applied,
            report.acknowledged
        ),
        (1, 4, 1, 1)
    );
    let stats = fixture.json("/__synthetic/control", json!({})).await;
    let uploads = &stats["uploadHashes"].as_array().unwrap()[initial_uploads..];
    assert_eq!(uploads.len(), 4);
    assert_eq!(uploads[0], json!(graph.resources()[0].blob_id()));
    assert_eq!(uploads[1], json!(graph.resources()[1].blob_id()));
    assert_eq!(
        uploads[2], uploads[1],
        "lost receipt retries exact immutable second chunk"
    );
    assert_eq!(uploads[3], json!(graph.root().blob_id()));
    assert!(owner
        .with_store(&auth.scope, |s| s.graph_root_uploaded(&op))
        .unwrap());
    let staged = owner
        .with_store(&auth.scope, |s| s.staged(3, 1))
        .unwrap()
        .remove(0);
    let resources = transport
        .download_resources(
            &owner,
            auth,
            &graph
                .resources()
                .iter()
                .map(|r| r.blob_id().to_owned())
                .collect::<Vec<_>>(),
            1024 * 1024,
        )
        .await
        .unwrap();
    let refs = resources
        .iter()
        .map(|r| crate::incoming::EncryptedResource {
            blob_id: &r.blob,
            envelope_utf8: &r.envelope_utf8,
        })
        .collect::<Vec<_>>();
    let prepared = crate::incoming::prepare(&auth.scope, &staged, &[&auth.library], &refs).unwrap();
    assert!(
        matches!(prepared.payload(), crate::incoming::IncomingPayload::Content(content) if content.markdown()==markdown)
    );
    assert_eq!(
        owner
            .with_store(&auth.scope, |s| s.cursors())
            .unwrap()
            .applied,
        1,
        "outgoing root receipt/preparation never advances applied"
    );
}
