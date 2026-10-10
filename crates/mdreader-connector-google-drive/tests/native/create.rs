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
fn empty_created(etag: Option<&str>, checksum: bool) -> Step {
    let mut value = serde_json::json!({"id":"created-empty","name":"plan.md","mimeType":"text/markdown","version":"1","size":"0","parents":["actual-root"],"trashed":false});
    if checksum {
        value["md5Checksum"] = "d41d8cd98f00b204e9800998ecf8427e".into();
    }
    Step::Reply(
        Response::new(
            200,
            etag.map(|value| BTreeMap::from([("etag".into(), value.into())]))
                .unwrap_or_default(),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap(),
    )
}
#[tokio::test]
async fn zero_byte_multipart_creation_confirms_without_inventing_a_write_validator() {
    let body = MarkdownBytes::new(vec![]).unwrap();
    let (client, script) = client(
        "root",
        true,
        true,
        vec![
            folder("actual-root", vec![]),
            empty_created(None, true),
            empty_created(Some("\"empty\""), true),
            folder("actual-root", vec![]),
        ],
    );
    let row = client
        .guarded_create(permit(&client, &body), &body, &context())
        .await
        .unwrap();
    assert_eq!(row.size_bytes, Some(0));
    assert_eq!(
        row.version.conditional_write_token,
        Some(OpaqueValue::new("\"empty\"").unwrap())
    );
    let requests = script.requests.lock().unwrap();
    let upload = &requests[1];
    assert!(
        upload
            .body()
            .windows(b"\r\n\r\n\r\n--".len())
            .any(|v| v == b"\r\n\r\n\r\n--")
    );
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.method() == Method::Post)
            .count(),
        1
    );
}
#[tokio::test]
async fn zero_byte_post_without_validator_confirms_creation_without_inventing_edit_authority() {
    let body = MarkdownBytes::new(vec![]).unwrap();
    let (client, script) = client(
        "root",
        true,
        true,
        vec![
            folder("actual-root", vec![]),
            empty_created(None, true),
            empty_created(None, true),
            folder("actual-root", vec![]),
        ],
    );
    let created = client
        .guarded_create(permit(&client, &body), &body, &context())
        .await
        .unwrap();
    assert_eq!(created.size_bytes, Some(0));
    assert_eq!(created.version.conditional_write_token, None);
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
fn operation_match(id: uuid::Uuid, changed: bool) -> Step {
    let Step::Reply(response) = empty_created(None, true) else {
        unreachable!()
    };
    let mut file: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
    file["modifiedTime"] = serde_json::json!("2000-01-01T00:00:00Z");
    file["appProperties"] = serde_json::json!({"mdreaderCreateOperation": if changed {uuid::Uuid::new_v4().to_string()} else {id.to_string()}});
    Step::Reply(
        Response::new(
            200,
            BTreeMap::new(),
            serde_json::to_vec(&serde_json::json!({"files":[file]})).unwrap(),
        )
        .unwrap(),
    )
}
fn recovery_steps(id: uuid::Uuid, changed: bool) -> Vec<Step> {
    vec![
        folder("actual-root", vec![]),
        operation_match(id, false),
        empty_created(None, true),
        folder("actual-root", vec![]),
        Step::Reply(Response::new(200, BTreeMap::new(), vec![]).unwrap()),
        empty_created(None, true),
        folder("actual-root", vec![]),
        operation_match(id, changed),
        folder("actual-root", vec![]),
    ]
}
#[tokio::test]
async fn read_only_original_empty_create_recovery_checks_operation_twice_and_never_posts() {
    let id = uuid::Uuid::new_v4();
    let body = MarkdownBytes::new(vec![]).unwrap();
    let (client, script) = client("root", true, true, recovery_steps(id, false));
    let creation::RecoveryObservation::Confirmed(row) = client
        .recover_create(id, "plan.md", &body, &context())
        .await
        .unwrap()
    else {
        panic!("original operation must be confirmed")
    };
    assert_eq!(row.key.as_str(), "created-empty");
    assert_eq!(row.size_bytes, Some(0));
    assert_eq!(row.modified_at_ms, Some(946684800000));
    assert!(row.version.conditional_write_token.is_none());
    let requests = script.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .all(|request| request.method() == Method::Get)
    );
    let queries: Vec<_> = requests
        .iter()
        .filter(|request| request.url().contains("appProperties"))
        .collect();
    assert_eq!(queries.len(), 2);
    for request in queries {
        let url = url::Url::parse(request.url()).unwrap();
        let q = url
            .query_pairs()
            .find(|(k, _)| k == "q")
            .unwrap()
            .1
            .into_owned();
        assert!(q.contains(&id.to_string()));
        assert!(q.contains("'actual-root' in parents"));
        assert!(
            url.query_pairs()
                .find(|(k, _)| k == "fields")
                .unwrap()
                .1
                .contains("modifiedTime")
        );
    }
}
#[tokio::test]
async fn recovery_refuses_changed_operation_ambiguity_and_remote_refusal_without_post() {
    let id = uuid::Uuid::new_v4();
    let body = MarkdownBytes::new(vec![]).unwrap();
    let (client, script) = client("root", true, true, recovery_steps(id, true));
    assert!(matches!(
        client
            .recover_create(id, "plan.md", &body, &context())
            .await
            .unwrap(),
        creation::RecoveryObservation::Unresolved(creation::RecoveryDiagnostic::MetadataMismatch)
    ));
    assert!(
        script
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.method() == Method::Get)
    );
    for (response, phase) in [
        (
            serde_json::json!({"files":[]}),
            creation::RecoveryDiagnostic::NoMatchingOperation,
        ),
        (
            serde_json::json!({"files":[],"incompleteSearch":true}),
            creation::RecoveryDiagnostic::AmbiguousOperation,
        ),
    ] {
        let (other, script) = super::folder_options::client(
            "root",
            true,
            true,
            vec![
                folder("actual-root", vec![]),
                Step::Reply(
                    Response::new(200, BTreeMap::new(), serde_json::to_vec(&response).unwrap())
                        .unwrap(),
                ),
            ],
        );
        let creation::RecoveryObservation::Unresolved(actual) = other
            .recover_create(id, "plan.md", &body, &context())
            .await
            .unwrap()
        else {
            panic!("must remain uncertain")
        };
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(phase).unwrap()
        );
        assert!(
            script
                .requests
                .lock()
                .unwrap()
                .iter()
                .all(|r| r.method() == Method::Get)
        );
    }
    let (other, script) = super::folder_options::client(
        "root",
        true,
        true,
        vec![
            folder("actual-root", vec![]),
            Step::Reply(Response::new(403, BTreeMap::new(), vec![]).unwrap()),
        ],
    );
    assert!(
        other
            .recover_create(id, "plan.md", &body, &context())
            .await
            .is_err()
    );
    assert!(
        script
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.method() == Method::Get)
    );
}

#[tokio::test]
async fn confirmation_retries_creation_metadata_without_repeating_post() {
    let mut stale = metadata("0", Some("\"stale\""), b"old");
    let mut value: serde_json::Value = serde_json::from_slice(stale.body()).unwrap();
    value["parents"] = serde_json::json!(["actual-root"]);
    stale = Response::new(
        200,
        stale.headers().clone(),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    let (client, script) = client(
        "root",
        true,
        true,
        vec![
            folder("actual-root", vec![]),
            created(),
            Step::Reply(stale),
            folder("actual-root", vec![]),
            created(),
            folder("actual-root", vec![]),
        ],
    );
    let body = MarkdownBytes::new(b"old".to_vec()).unwrap();
    let row = client
        .guarded_create(permit(&client, &body), &body, &context())
        .await
        .unwrap();
    assert_eq!(row.version.provider_revision.as_str(), "1");
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
    assert!(script.steps.lock().unwrap().is_empty());
}

fn current_created(step: Step, revision: &str, key: &str) -> Step {
    let Step::Reply(response) = step else {
        unreachable!()
    };
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(response.body()) else {
        return Step::Reply(response);
    };
    let file = if value.get("files").is_some() {
        &mut value["files"][0]
    } else {
        &mut value
    };
    if file["mimeType"] == "text/markdown" {
        file["version"] = revision.into();
        file["id"] = key.into();
    }
    Step::Reply(
        Response::new(
            response.status(),
            response.headers().clone(),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap(),
    )
}
#[tokio::test]
async fn create_advanced_head_is_proven_by_original_operation_and_literal_body_without_post_replay()
{
    for (wrong_key, wrong_operation) in [(false, false), (true, false), (false, true)] {
        let id = uuid::Uuid::new_v4();
        let body = MarkdownBytes::new(vec![]).unwrap();
        let mut steps = vec![folder("actual-root", vec![]), empty_created(None, true)];
        // Every successful metadata GET has a stable head newer than the POST;
        // delaying/repeating that comparison can never resolve this difference.
        for _ in 0..3 {
            steps.push(current_created(
                empty_created(None, true),
                "2",
                "created-empty",
            ));
            steps.push(folder("actual-root", vec![]));
        }
        let proof = recovery_steps(id, false);
        for (n, step) in proof.into_iter().enumerate() {
            let step = if n == 1 && wrong_operation {
                operation_match(id, true)
            } else {
                step
            };
            steps.push(current_created(
                step,
                "2",
                if wrong_key {
                    "different-file"
                } else {
                    "created-empty"
                },
            ));
            if (wrong_key || wrong_operation) && n == 1 {
                break;
            }
        }
        let (client, script) = client("root", true, true, steps);
        let mut c = rusqlite::Connection::open_in_memory().unwrap();
        creation::prepare(&mut c, client.descriptor().binding(), id, "plan.md", &body).unwrap();
        let p = creation::dispatch(&mut c, client.descriptor().binding(), id).unwrap();
        let result = client.guarded_create(p, &body, &context()).await;
        if wrong_key || wrong_operation {
            assert!(
                matches!(result, Err(ProviderError::UnknownWrite { operation_id }) if operation_id == id)
            );
        } else {
            let row = result.unwrap();
            assert_eq!(row.key.as_str(), "created-empty");
            assert_eq!(row.version.provider_revision.as_str(), "2");
            assert_eq!(row.size_bytes, Some(0));
            assert!(row.version.conditional_write_token.is_none());
        }
        let requests = script.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.method() == Method::Post)
                .count(),
            1
        );
        assert!(
            requests
                .iter()
                .all(|r| matches!(r.method(), Method::Get | Method::Post))
        );
        assert!(script.steps.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn advanced_create_head_with_different_literal_body_remains_unknown_without_replay() {
    let id = uuid::Uuid::new_v4();
    let body = MarkdownBytes::new(b"old".to_vec()).unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(metadata("2", None, b"new").body()).unwrap();
    value["parents"] = serde_json::json!(["actual-root"]);
    let reply = || {
        Step::Reply(
            Response::new(200, BTreeMap::new(), serde_json::to_vec(&value).unwrap()).unwrap(),
        )
    };
    let mut matched = value.clone();
    matched["appProperties"] = serde_json::json!({"mdreaderCreateOperation":id.to_string()});
    let matched = Step::Reply(
        Response::new(
            200,
            BTreeMap::new(),
            serde_json::to_vec(&serde_json::json!({"files":[matched]})).unwrap(),
        )
        .unwrap(),
    );
    let mut steps = vec![folder("actual-root", vec![]), created()];
    for _ in 0..3 {
        steps.extend([reply(), folder("actual-root", vec![])]);
    }
    steps.extend([
        folder("actual-root", vec![]),
        matched,
        reply(),
        folder("actual-root", vec![]),
        Step::Reply(Response::new(200, BTreeMap::new(), b"new".to_vec()).unwrap()),
        reply(),
        folder("actual-root", vec![]),
    ]);
    let (client, script) = client("root", true, true, steps);
    let mut c = rusqlite::Connection::open_in_memory().unwrap();
    creation::prepare(&mut c, client.descriptor().binding(), id, "plan.md", &body).unwrap();
    let p = creation::dispatch(&mut c, client.descriptor().binding(), id).unwrap();
    assert!(
        matches!(client.guarded_create(p, &body, &context()).await, Err(ProviderError::UnknownWrite { operation_id }) if operation_id == id)
    );
    let requests = script.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.method() == Method::Post)
            .count(),
        1
    );
    assert_eq!(script.mutations.load(Ordering::SeqCst), 0);
    assert!(script.steps.lock().unwrap().is_empty());
}
