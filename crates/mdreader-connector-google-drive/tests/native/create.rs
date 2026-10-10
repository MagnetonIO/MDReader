use super::folder_options::{client, folder};
use super::*;
fn permit(client: &Client, body: &MarkdownBytes) -> creation::CreatePermit {
    let mut c = rusqlite::Connection::open_in_memory().unwrap();
    let id = uuid::Uuid::new_v4();
    creation::prepare(&mut c, client.descriptor().binding(), id, "plan.md", body).unwrap();
    creation::dispatch(&mut c, client.descriptor().binding(), id).unwrap()
}
fn created() -> Step {
    let response = metadata("1", Some("\"created\""), b"old");
    let mut value: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
    value["parents"] = serde_json::json!(["actual-root"]);
    Step::Reply(
        Response::new(
            200,
            response.headers().clone(),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap(),
    )
}
#[tokio::test]
async fn multipart_create_uses_saved_parent_and_confirms_exact_new_receipt() {
    let (client, script) = client(
        "root",
        true,
        true,
        vec![
            folder("actual-root", vec![]),
            created(),
            created(),
            folder("actual-root", vec![]),
        ],
    );
    let body = MarkdownBytes::new(b"old".to_vec()).unwrap();
    let p = permit(&client, &body);
    let operation = p.operation_id();
    let row = client.guarded_create(p, &body, &context()).await.unwrap();
    assert_eq!(row.display_path.as_str(), "/root/selected");
    assert!(row.version.conditional_write_token.is_some());
    let requests = script.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.method() == Method::Post)
            .count(),
        1
    );
    let post = &requests[1];
    assert!(post.url().contains("uploadType=multipart"));
    let payload = std::str::from_utf8(post.body()).unwrap();
    assert!(payload.contains("\"parents\":[\"actual-root\"]"));
    assert!(payload.contains(&operation.to_string()));
    assert!(payload.contains("\r\nold\r\n"));
    assert!(!post.url().contains("/files/selected"));
}
#[tokio::test]
async fn create_does_not_widen_exact_or_readonly_selection_and_lost_reply_is_unknown() {
    let body = MarkdownBytes::new(b"old".to_vec()).unwrap();
    let (exact, script, _, _) = fixture(vec![]);
    assert!(matches!(
        exact
            .guarded_create(permit(&exact, &body), &body, &context())
            .await,
        Err(ProviderError::Unsupported)
    ));
    assert!(script.requests.lock().unwrap().is_empty());
    let (readonly, script) = client("root", true, false, vec![]);
    assert!(matches!(
        readonly
            .guarded_create(permit(&readonly, &body), &body, &context())
            .await,
        Err(ProviderError::Unsupported)
    ));
    assert!(script.requests.lock().unwrap().is_empty());
    let (client, script) = client(
        "root",
        true,
        true,
        vec![folder("actual-root", vec![]), Step::AcceptedThenLost],
    );
    assert!(matches!(
        client
            .guarded_create(permit(&client, &body), &body, &context())
            .await,
        Err(ProviderError::UnknownWrite { .. })
    ));
    assert_eq!(
        script
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method() == Method::Post)
            .count(),
        1
    );
}
