use super::*;
fn permit(client: &Client, body: &MarkdownBytes) -> creation::CreatePermit {
    let mut c = rusqlite::Connection::open_in_memory().unwrap();
    let id = uuid::Uuid::new_v4();
    creation::prepare(&mut c, client.descriptor().binding(), id, "plan.md", body).unwrap();
    creation::dispatch(&mut c, client.descriptor().binding(), id).unwrap()
}
#[tokio::test]
async fn create_only_add_is_scoped_exact_and_conflict_never_overwrites() {
    let body = MarkdownBytes::new(b"old".to_vec()).unwrap();
    let conflict = Response::new(
        409,
        BTreeMap::new(),
        br#"{"error":{".tag":"path","path":{".tag":"conflict"}}}"#.to_vec(),
    )
    .unwrap();
    let (client, script, _) = fixture(
        true,
        vec![
            Step::Reply(json(file("abcdef001", b"old"))),
            Step::Reply(conflict),
        ],
    );
    let metadata = client
        .guarded_create(permit(&client, &body), &body, &context())
        .await
        .unwrap();
    assert_eq!(metadata.display_path.as_str(), "/plans/plan.md");
    assert!(matches!(
        client
            .guarded_create(permit(&client, &body), &body, &context())
            .await,
        Err(ProviderError::Conflict)
    ));
    for request in script.requests.lock().unwrap().iter() {
        let args: serde_json::Value =
            serde_json::from_str(&request.headers()["dropbox-api-arg"]).unwrap();
        assert_eq!(args["mode"], "add");
        assert_eq!(args["autorename"], false);
        assert_eq!(args["strict_conflict"], true);
        assert_eq!(args["path"], "/plans/plan.md");
    }
    let (exact, script, _) = fixture(false, vec![]);
    assert!(matches!(
        exact
            .guarded_create(permit(&exact, &body), &body, &context())
            .await,
        Err(ProviderError::Unsupported)
    ));
    assert!(script.requests.lock().unwrap().is_empty());
}
#[tokio::test]
async fn lost_create_reply_is_unknown_and_revoked_grant_sends_nothing() {
    let body = MarkdownBytes::new(b"old".to_vec()).unwrap();
    let (client, script, authority) = fixture(true, vec![Step::AcceptedThenLost]);
    assert!(matches!(
        client
            .guarded_create(permit(&client, &body), &body, &context())
            .await,
        Err(ProviderError::UnknownWrite { .. })
    ));
    assert_eq!(script.requests.lock().unwrap().len(), 1);
    authority.0.store(false, Ordering::SeqCst);
    assert!(matches!(
        client
            .guarded_create(permit(&client, &body), &body, &context())
            .await,
        Err(ProviderError::Unauthorized)
    ));
    assert_eq!(script.requests.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn unicode_create_filename_has_ascii_header_json_with_surrogate_pairs() {
    let filename = "Résumé😀.md";
    let body = MarkdownBytes::new(b"old".to_vec()).unwrap();
    let mut response = file("abcdef001", b"old");
    response["name"] = filename.into();
    response["path_lower"] = "/plans/résumé😀.md".into();
    let (client, script, _) = fixture(true, vec![Step::Reply(json(response))]);
    let mut c = rusqlite::Connection::open_in_memory().unwrap();
    let operation = uuid::Uuid::new_v4();
    creation::prepare(
        &mut c,
        client.descriptor().binding(),
        operation,
        filename,
        &body,
    )
    .unwrap();
    let permit = creation::dispatch(&mut c, client.descriptor().binding(), operation).unwrap();
    client
        .guarded_create(permit, &body, &context())
        .await
        .unwrap();
    let requests = script.requests.lock().unwrap();
    let header = &requests[0].headers()["dropbox-api-arg"];
    assert!(header.is_ascii());
    assert!(header.contains("\\ud83d\\ude00"));
    let args: serde_json::Value = serde_json::from_str(header).unwrap();
    assert_eq!(args["path"], "/plans/Résumé😀.md");
}
