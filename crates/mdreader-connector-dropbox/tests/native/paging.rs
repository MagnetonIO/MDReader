use super::*;
fn small() -> IoContext {
    IoContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_secs(5),
        2,
        8 * 1024 * 1024,
        Cancellation::default(),
    )
    .unwrap()
}
fn entries() -> Vec<serde_json::Value> {
    let mut a = file("abcdef001", b"old");
    let mut b = a.clone();
    b["id"] = "id:second".into();
    b["name"] = "other.markdown".into();
    b["path_lower"] = "/plans/other.markdown".into();
    a["id"] = "id:first".into();
    vec![
        serde_json::json!({".tag":"folder","id":"id:folder","name":"nested","path_lower":"/plans/nested"}),
        a,
        serde_json::json!({".tag":"file","id":"id:nonmd","name":"plain.txt","path_lower":"/plans/plain.txt","rev":"abcdef001","size":3}),
        b,
        serde_json::json!({".tag":"deleted","name":"gone.md","path_lower":"/plans/gone.md"}),
    ]
}
fn listing() -> Vec<serde_json::Value> {
    let mut rows = entries();
    rows.pop();
    let mut third = file("abcdef002", b"third");
    third["id"] = "id:third".into();
    third["path_lower"] = "/plans/third.md".into();
    third["name"] = "third.md".into();
    rows.push(third);
    rows
}
#[tokio::test]
async fn overflow_listing_reopens_frozen_owner_page_without_another_remote_request() {
    let (client, script, authority) = fixture(
        true,
        vec![Step::Reply(json(
            serde_json::json!({"entries":listing(),"cursor":"remote-after","has_more":false}),
        ))],
    );
    let first = client.list(None, &small()).await.unwrap();
    assert_eq!(first.rows().len(), 2); // Folders/non-Markdown do not consume local output slots.
    let cursor = first.next_cursor().unwrap();
    assert!(cursor.as_bytes().len() < 32 * 1024);
    let reopened = Client::new(
        client.descriptor().clone(),
        Selection::AppFolderRoot {
            path: RecordKey::new("/plans").unwrap(),
            recursive: true,
        },
        Arc::new(Resolver),
        authority,
        script.clone(),
        [9; 32],
    )
    .unwrap();
    let second = reopened.list(Some(cursor), &small()).await.unwrap();
    assert_eq!(second.rows()[0].key.as_str(), "id:third");
    assert!(second.next_cursor().is_none());
    assert!(matches!(
        second.coverage(),
        Coverage::MetadataCompleteSelected { .. }
    ));
    assert_eq!(script.requests.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn folder_heavy_listing_exposes_markdown_in_first_chunk_without_false_early_completion() {
    let mut rows: Vec<_> = (0..600).map(|i| serde_json::json!({".tag":"folder","id":format!("id:folder{i}"),"name":format!("folder{i}"),"path_lower":format!("/plans/folder{i}")})).collect();
    rows.push(file("abcdef001", b"old"));
    let (client, script, _) = fixture(
        true,
        vec![
            Step::Reply(json(
                serde_json::json!({"entries":rows,"cursor":"next","has_more":true}),
            )),
            Step::Reply(json(
                serde_json::json!({"entries":[],"cursor":"done","has_more":false}),
            )),
        ],
    );
    let first = client.list(None, &small()).await.unwrap();
    assert_eq!(first.rows().len(), 1);
    assert!(first.next_cursor().is_some());
    let last = client.list(first.next_cursor(), &small()).await.unwrap();
    assert!(last.next_cursor().is_none());
    assert_eq!(script.requests.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn frozen_change_page_preserves_nonmarkdown_deletes_and_delays_remote_checkpoint() {
    let (client, script, _) = fixture(
        true,
        vec![
            Step::Reply(json(serde_json::json!({"cursor":"remote-before"}))),
            Step::Reply(json(
                serde_json::json!({"entries":entries(),"cursor":"remote-after","has_more":false}),
            )),
        ],
    );
    let mut cursor = client.start_changes(&small()).await.unwrap();
    let mut total = 0;
    for index in 0..3 {
        match client.changes(&cursor, &small()).await.unwrap() {
            ChangeOutcome::Page {
                events,
                next_cursor,
                checkpoint,
            } => {
                assert!(events.len() <= 2);
                total += events.len();
                if index < 2 {
                    assert!(checkpoint.is_none());
                    cursor = next_cursor.unwrap();
                } else {
                    assert!(next_cursor.is_none());
                    assert!(checkpoint.is_some());
                }
            }
            _ => panic!("expected page"),
        }
    }
    assert_eq!(total, 3); // Unknown deleted paths cannot manufacture identities.
    assert_eq!(script.requests.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn exchanged_revoked_or_missing_spool_cursor_never_advances_or_refetches() {
    let (client, script, authority) = fixture(
        true,
        vec![Step::Reply(json(
            serde_json::json!({"entries":listing(),"cursor":"after","has_more":false}),
        ))],
    );
    let first = client.list(None, &small()).await.unwrap();
    assert!(matches!(
        client.changes(first.next_cursor().unwrap(), &small()).await,
        Err(ProviderError::Unauthorized)
    ));
    authority.0.store(false, Ordering::SeqCst);
    assert!(matches!(
        client.list(first.next_cursor(), &small()).await,
        Err(ProviderError::Unauthorized)
    ));
    authority.0.store(true, Ordering::SeqCst);
    authority
        .1
        .lock()
        .unwrap()
        .execute("DELETE FROM pages", [])
        .unwrap();
    assert!(matches!(
        client.list(first.next_cursor(), &small()).await,
        Err(ProviderError::ResetCursor)
    ));
    assert_eq!(script.requests.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn unsupported_spool_owner_refuses_overflow_without_issuing_partial_cursor() {
    struct NoSpool;
    #[async_trait]
    impl ProviderAuthority for NoSpool {
        async fn revalidate(&self, _: &SourceBinding) -> Result<()> {
            Ok(())
        }
    }
    let (original, script, _) = fixture(
        true,
        vec![Step::Reply(json(
            serde_json::json!({"entries":listing(),"cursor":"after","has_more":false}),
        ))],
    );
    let client = Client::new(
        original.descriptor().clone(),
        Selection::AppFolderRoot {
            path: RecordKey::new("/plans").unwrap(),
            recursive: true,
        },
        Arc::new(Resolver),
        Arc::new(NoSpool),
        script.clone(),
        [9; 32],
    )
    .unwrap();
    assert!(matches!(
        client.list(None, &small()).await,
        Err(ProviderError::Unsupported)
    ));
    assert_eq!(script.requests.lock().unwrap().len(), 1);
}
