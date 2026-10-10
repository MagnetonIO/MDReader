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
#[tokio::test]
async fn oversized_listing_reopens_signed_offset_without_losing_nonmarkdown_entries() {
    let mut rows = entries();
    rows[4] = serde_json::json!({".tag":"folder","name":"last","id":"id:last","path_lower":"/plans/last"});
    let page = serde_json::json!({"entries":rows,"cursor":"remote-after","has_more":false});
    let (client, script, authority) = fixture(
        true,
        vec![
            Step::Reply(json(page.clone())),
            Step::Reply(json(page.clone())),
            Step::Reply(json(page)),
        ],
    );
    let first = client.list(None, &small()).await.unwrap();
    assert_eq!(first.rows().len(), 1);
    let cursor = first.next_cursor().unwrap();
    assert!(cursor.as_bytes().len() < 32 * 1024);
    let c = rusqlite::Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE continuation (bytes BLOB NOT NULL)")
        .unwrap();
    c.execute("INSERT INTO continuation VALUES(?1)", [cursor.as_bytes()])
        .unwrap();
    let retained: Vec<u8> = c
        .query_row("SELECT bytes FROM continuation", [], |r| r.get(0))
        .unwrap();
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
    let second = reopened
        .list(Some(&ProviderCursor::new(retained).unwrap()), &small())
        .await
        .unwrap();
    assert_eq!(second.rows()[0].key.as_str(), "id:second");
    let last = reopened.list(second.next_cursor(), &small()).await.unwrap();
    assert!(last.rows().is_empty());
    assert!(last.next_cursor().is_none());
    assert!(matches!(
        last.coverage(),
        Coverage::MetadataCompleteSelected { .. }
    ));
    for request in script.requests.lock().unwrap().iter() {
        assert!(request.url().ends_with("/list_folder"));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(request.body()).unwrap()["limit"],
            2
        );
    }
}
#[tokio::test]
async fn oversized_legacy_change_page_delays_checkpoint_and_replays_exact_origin() {
    let page = serde_json::json!({"entries":entries(),"cursor":"remote-after","has_more":false});
    let (client, script, _) = fixture(
        true,
        vec![
            Step::Reply(json(serde_json::json!({"cursor":"remote-before"}))),
            Step::Reply(json(page.clone())),
            Step::Reply(json(page.clone())),
            Step::Reply(json(page)),
        ],
    );
    let first = client.start_changes(&small()).await.unwrap();
    let mut next = first;
    let mut totals = 0;
    for index in 0..3 {
        match client.changes(&next, &small()).await.unwrap() {
            ChangeOutcome::Page {
                events,
                next_cursor,
                checkpoint,
            } => {
                assert!(events.len() <= 2);
                totals += events.len();
                if index < 2 {
                    assert!(checkpoint.is_none());
                    next = next_cursor.unwrap();
                } else {
                    assert!(next_cursor.is_none());
                    assert!(checkpoint.is_some());
                }
            }
            _ => panic!("expected bounded change page"),
        }
    }
    assert_eq!(totals, 3); // Unobserved deleted paths cannot manufacture file IDs.
    let requests = script.requests.lock().unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(requests[0].body()).unwrap()["limit"],
        2
    );
    for request in &requests[1..] {
        assert!(request.url().ends_with("/list_folder/continue"));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(request.body()).unwrap()["cursor"],
            "remote-before"
        );
    }
}
#[tokio::test]
async fn replay_changed_page_and_exchanged_cursor_fail_without_skipping_tail() {
    let page = serde_json::json!({"entries":entries(),"cursor":"after","has_more":false});
    let mut changed = page.clone();
    changed["entries"][3]["rev"] = "different".into();
    let (client, script, _) = fixture(
        true,
        vec![Step::Reply(json(page)), Step::Reply(json(changed))],
    );
    let first = client.list(None, &small()).await.unwrap();
    assert!(matches!(
        client.changes(first.next_cursor().unwrap(), &small()).await,
        Err(ProviderError::Unauthorized)
    ));
    assert_eq!(script.requests.lock().unwrap().len(), 1);
    assert!(matches!(
        client.list(first.next_cursor(), &small()).await,
        Err(ProviderError::ResetCursor)
    ));
}
