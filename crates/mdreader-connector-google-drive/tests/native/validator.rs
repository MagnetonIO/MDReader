use super::folder_options::{client, folder};
use super::*;
fn v3(revision: &str, body: &[u8]) -> Step {
    let response = metadata(revision, None, body);
    let mut value: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
    value["parents"] = serde_json::json!(["actual-root"]);
    Step::Reply(Response::new(200, BTreeMap::new(), serde_json::to_vec(&value).unwrap()).unwrap())
}
fn v2(revision: &str, etag: &str, body: &[u8]) -> Step {
    let response = metadata(revision, None, body);
    let v: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
    Step::Reply(Response::new(200,BTreeMap::new(),serde_json::to_vec(&serde_json::json!({"id":"selected","title":"plan.md","mimeType":"text/markdown","version":revision,"md5Checksum":v["md5Checksum"],"fileSize":body.len().to_string(),"parents":[{"id":"actual-root"}],"labels":{"trashed":false},"etag":etag})).unwrap()).unwrap())
}
fn observed(revision: &str, etag: &str, body: &[u8]) -> Vec<Step> {
    vec![
        v3(revision, body),
        folder("actual-root", vec![]),
        v2(revision, etag, body),
    ]
}
fn permit(client: &Client, expected: &DocumentVersion) -> journal::DispatchPermit {
    let mut c = rusqlite::Connection::open_in_memory().unwrap();
    let body = MarkdownBytes::new(b"new".to_vec()).unwrap();
    let operation = journal::prepare(
        &mut c,
        client.descriptor().binding(),
        &RecordKey::new("selected").unwrap(),
        expected,
        &body,
    )
    .unwrap();
    journal::begin_dispatch(&mut c, client.descriptor().binding(), operation).unwrap()
}
#[tokio::test]
async fn empty_file_validator_binds_exact_v3_read_and_conditional_v2_update() {
    let mut steps = observed("1", "\"actual-file-v1\"", b"");
    steps.push(Step::Reply(
        Response::new(200, BTreeMap::new(), b"".to_vec()).unwrap(),
    ));
    steps.extend(observed("1", "\"actual-file-v1\"", b""));
    steps.extend(observed("1", "\"actual-file-v1\"", b""));
    steps.push(v2("2", "\"actual-file-v2\"", b"new"));
    steps.extend(observed("2", "\"actual-file-v2\"", b"new"));
    let (client, script) = client("root", true, true, steps);
    let read = client
        .read(&RecordKey::new("selected").unwrap(), &context())
        .await
        .unwrap();
    assert_eq!(read.body.as_bytes(), b"");
    assert_eq!(
        read.version
            .conditional_write_token
            .as_ref()
            .unwrap()
            .as_str(),
        "drive-v2-file-etag:\"actual-file-v1\""
    );
    let receipt = client
        .guarded_write(
            permit(&client, &read.version),
            &MarkdownBytes::new(b"new".to_vec()).unwrap(),
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(receipt.version.provider_revision.as_str(), "2");
    assert_eq!(
        receipt.version.conditional_write_token.unwrap().as_str(),
        "drive-v2-file-etag:\"actual-file-v2\""
    );
    let requests = script.requests.lock().unwrap();
    let uploads: Vec<_> = requests
        .iter()
        .filter(|r| matches!(r.method(), Method::Put | Method::Patch | Method::Post))
        .collect();
    assert_eq!(uploads.len(), 1);
    assert_eq!(uploads[0].method(), Method::Put);
    assert!(uploads[0]
        .url()
        .contains("/upload/drive/v2/files/selected?uploadType=multipart"));
    assert_eq!(uploads[0].headers()["if-match"], "\"actual-file-v1\"");
    assert!(std::str::from_utf8(uploads[0].body())
        .unwrap()
        .contains("\r\nnew\r\n"));
    let download = requests
        .iter()
        .find(|r| r.url().contains("alt=media"))
        .unwrap();
    assert!(
        !download.headers().contains_key("if-match"),
        "v2 metadata ETag must not be used as v3 media validator"
    );
}
#[tokio::test]
async fn v2_revision_mismatch_and_weak_validator_never_grant_editing() {
    for (revision, etag, expected_error) in [("2", "\"v2\"", true), ("1", "W/\"weak\"", false)] {
        let (client, script) = client(
            "root",
            true,
            true,
            vec![
                v3("1", b"old"),
                folder("actual-root", vec![]),
                v2(revision, etag, b"old"),
            ],
        );
        let result = client
            .metadata(&RecordKey::new("selected").unwrap(), &context())
            .await;
        if expected_error {
            assert!(matches!(result, Err(ProviderError::Conflict)));
        } else {
            assert!(result.unwrap().version.conditional_write_token.is_none());
        }
        assert_eq!(script.mutations.load(Ordering::SeqCst), 0);
    }
}
#[tokio::test]
async fn v2_conditional_precondition_failure_is_conflict_and_never_retried() {
    let mut steps = observed("1", "\"actual-v1\"", b"old");
    steps.extend(observed("1", "\"actual-v1\"", b"old"));
    steps.push(Step::Reply(
        Response::new(412, BTreeMap::new(), vec![]).unwrap(),
    ));
    let (client, script) = client("root", true, true, steps);
    let metadata = client
        .metadata(&RecordKey::new("selected").unwrap(), &context())
        .await
        .unwrap();
    assert!(matches!(
        client
            .guarded_write(
                permit(&client, &metadata.version),
                &MarkdownBytes::new(b"new".to_vec()).unwrap(),
                &context()
            )
            .await,
        Err(ProviderError::Conflict)
    ));
    assert_eq!(script.mutations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn v2_lost_update_reply_remains_unknown_without_retry() {
    let mut steps = observed("1", "\"actual-v1\"", b"old");
    steps.extend(observed("1", "\"actual-v1\"", b"old"));
    steps.push(Step::AcceptedThenLost);
    let (client, script) = client("root", true, true, steps);
    let metadata = client
        .metadata(&RecordKey::new("selected").unwrap(), &context())
        .await
        .unwrap();
    let permit = permit(&client, &metadata.version);
    let operation_id = permit.operation_id();
    assert!(
        matches!(client.guarded_write(permit, &MarkdownBytes::new(b"new".to_vec()).unwrap(), &context()).await, Err(ProviderError::UnknownWrite { operation_id: actual }) if actual == operation_id)
    );
    assert_eq!(script.mutations.load(Ordering::SeqCst), 1);
}

fn chosen_permit(
    client: &Client,
    expected: &DocumentVersion,
    body: &MarkdownBytes,
    id: uuid::Uuid,
) -> journal::DispatchPermit {
    let mut c = rusqlite::Connection::open_in_memory().unwrap();
    journal::prepare_with_id(
        &mut c,
        client.descriptor().binding(),
        &RecordKey::new("selected").unwrap(),
        expected,
        body,
        id,
    )
    .unwrap();
    journal::begin_dispatch(&mut c, client.descriptor().binding(), id).unwrap()
}
#[tokio::test]
async fn chosen_edit_delimiter_and_framing_overflow_refuse_before_put() {
    let id = uuid::Uuid::parse_str("40000000-0000-4000-8000-000000000001").unwrap();
    for (draft, too_large) in [
        (
            format!("literal\r\n--mdreader-{id}--\r\nend").into_bytes(),
            false,
        ),
        (vec![b'a'; MAX_BODY_BYTES], true),
    ] {
        let mut steps = observed("1", "\"actual-v1\"", b"old");
        steps.extend(observed("1", "\"actual-v1\"", b"old"));
        let (client, script) = client("root", true, true, steps);
        let metadata = client
            .metadata(&RecordKey::new("selected").unwrap(), &context())
            .await
            .unwrap();
        let body = MarkdownBytes::new(draft.clone()).unwrap();
        let result = client
            .guarded_write(
                chosen_permit(&client, &metadata.version, &body, id),
                &body,
                &context(),
            )
            .await;
        if too_large {
            assert!(matches!(result, Err(ProviderError::TooLarge)));
        } else {
            assert!(matches!(result, Err(ProviderError::Invalid)));
        }
        assert_eq!(body.as_bytes(), draft);
        assert_eq!(script.mutations.load(Ordering::SeqCst), 0);
        assert!(script
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.method() == Method::Get));
    }
}
fn without_checksum(step: Step) -> Step {
    let Step::Reply(response) = step else {
        panic!("fixture response");
    };
    let mut v: serde_json::Value = serde_json::from_slice(response.body()).unwrap();
    v.as_object_mut().unwrap().remove("md5Checksum");
    Step::Reply(Response::new(200, BTreeMap::new(), serde_json::to_vec(&v).unwrap()).unwrap())
}
#[tokio::test]
async fn v2_missing_or_mismatched_post_checksum_never_certifies_literal_bytes() {
    for case in 0..3 {
        let mut steps = observed("1", "\"actual-v1\"", b"old");
        steps.extend(observed("1", "\"actual-v1\"", b"old"));
        let post = v2(
            "2",
            "\"actual-v2\"",
            if case == 1 { b"old" } else { b"new" },
        );
        steps.push(if case == 0 {
            without_checksum(post)
        } else {
            post
        });
        if case == 2 {
            steps.extend([
                without_checksum(v3("2", b"new")),
                folder("actual-root", vec![]),
                v2("2", "\"actual-v2\"", b"new"),
            ]);
        }
        let (client, script) = client("root", true, true, steps);
        let metadata = client
            .metadata(&RecordKey::new("selected").unwrap(), &context())
            .await
            .unwrap();
        let permit = permit(&client, &metadata.version);
        let id = permit.operation_id();
        assert!(
            matches!(client.guarded_write(permit, &MarkdownBytes::new(b"new".to_vec()).unwrap(), &context()).await, Err(ProviderError::UnknownWrite { operation_id }) if operation_id == id)
        );
        assert_eq!(script.mutations.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn v2_http_refusal_keeps_original_unknown_operation_and_never_retries() {
    for status in [400, 403] {
        let mut steps = observed("1", "\"actual-v1\"", b"old");
        steps.extend(observed("1", "\"actual-v1\"", b"old"));
        steps.push(Step::Reply(
            Response::new(
                status,
                BTreeMap::new(),
                b"untrusted provider response".to_vec(),
            )
            .unwrap(),
        ));
        let (client, script) = client("root", true, true, steps);
        let metadata = client
            .metadata(&RecordKey::new("selected").unwrap(), &context())
            .await
            .unwrap();
        let permit = permit(&client, &metadata.version);
        let id = permit.operation_id();
        assert!(
            matches!(client.guarded_write(permit, &MarkdownBytes::new(b"new".to_vec()).unwrap(), &context()).await, Err(ProviderError::UnknownWrite { operation_id }) if operation_id == id)
        );
        assert_eq!(script.mutations.load(Ordering::SeqCst), 1);
    }
}
