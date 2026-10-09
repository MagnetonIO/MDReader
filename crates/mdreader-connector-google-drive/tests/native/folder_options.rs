use super::*;
fn reply(value: serde_json::Value) -> Step {
    Step::Reply(Response::new(200, BTreeMap::new(), serde_json::to_vec(&value).unwrap()).unwrap())
}
fn folder(id: &str, parents: Vec<&str>) -> Step {
    reply(
        serde_json::json!({"id":id,"mimeType":"application/vnd.google-apps.folder","parents":parents}),
    )
}
fn member_metadata(parent: &str, etag: Option<&str>) -> Step {
    let mut value: serde_json::Value =
        serde_json::from_slice(metadata("1", etag, b"old").body()).unwrap();
    value["parents"] = serde_json::json!([parent]);
    Step::Reply(
        Response::new(
            200,
            etag.map(|e| BTreeMap::from([("etag".into(), e.into())]))
                .unwrap_or_default(),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap(),
    )
}
fn client(root: &str, recursive: bool, editing: bool, steps: Vec<Step>) -> (Client, Arc<Script>) {
    let (_, script, resolver, authority) = fixture(steps);
    let root = RecordKey::new(root).unwrap();
    let scope = SelectedScope::DriveFolder {
        root: root.clone(),
        recursive,
    };
    let binding = SourceBinding::new(
        LibraryId::new("library").unwrap(),
        SourceId::new("source").unwrap(),
        AccountBinding::new("account").unwrap(),
        uuid::Uuid::new_v4(),
        scope.digest(),
    )
    .unwrap();
    let descriptor = SourceDescriptor::new(
        binding,
        ProviderKind::GoogleDrive,
        DisplayText::new("Synthetic folder").unwrap(),
        scope,
        ProviderCapabilities {
            read: true,
            list: true,
            changes: false,
            assets: false,
            guarded_write: editing,
        },
        SourceHealth::Ready,
        Coverage::NeverIndexed,
    )
    .unwrap();
    (
        Client::new_folder(
            descriptor,
            root,
            recursive,
            resolver,
            authority,
            script.clone(),
            [7; 32],
        )
        .unwrap(),
        script,
    )
}
#[tokio::test]
async fn folder_options_enumerate_direct_or_recursive_scope_without_fetching_bodies_and_bind_all_cursor_state(
) {
    let first = serde_json::json!({"files":[
        {"id":"nested","name":"Nested","mimeType":"application/vnd.google-apps.folder","version":"1","parents":["actual-root"]},
        {"id":"direct","name":"Direct.MD","mimeType":"text/markdown","version":"1","parents":["actual-root"]},
        {"id":"ignored","name":"prefix.md.txt","mimeType":"text/plain","version":"1","parents":["actual-root"]}
    ]});
    let nested = serde_json::json!({"files":[{"id":"leaf","name":"Leaf.markdown","mimeType":"text/plain","version":"1","parents":["nested"]}]});
    let (recursive, script) = client(
        "root",
        true,
        false,
        vec![
            folder("actual-root", vec![]),
            reply(first),
            folder("actual-root", vec![]),
            folder("actual-root", vec![]),
            folder("nested", vec!["actual-root"]),
            reply(nested),
        ],
    );
    let page = recursive.list(None, &context()).await.unwrap();
    assert_eq!(page.rows().len(), 1);
    assert_eq!(page.rows()[0].name.as_str(), "Direct.MD");
    let cursor = page.next_cursor().unwrap();
    let (other, other_script) = client("root", false, false, vec![]);
    assert!(other.list(Some(cursor), &context()).await.is_err());
    assert!(other_script.requests.lock().unwrap().is_empty());
    let next = recursive.list(Some(cursor), &context()).await.unwrap();
    assert_eq!(next.rows()[0].name.as_str(), "Leaf.markdown");
    assert!(next.next_cursor().is_none());
    let requests = script.requests.lock().unwrap();
    assert_eq!(requests.len(), 6);
    assert!(requests
        .iter()
        .all(|r| r.method() == Method::Get && !r.url().contains("alt=media")));
    let query: BTreeMap<_, _> = url::Url::parse(requests[1].url())
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    assert!(query["q"].contains("'actual-root' in parents"));
    assert!(query["q"].contains("google-apps.folder"));
    drop(requests);
    let (direct, script) = client(
        "notes",
        false,
        false,
        vec![
            folder("notes", vec![]),
            reply(
                serde_json::json!({"files":[{"id":"direct","name":"Only.md","mimeType":"text/markdown","version":"1","parents":["notes"]}]}),
            ),
        ],
    );
    assert!(direct
        .list(None, &context())
        .await
        .unwrap()
        .next_cursor()
        .is_none());
    let requests = script.requests.lock().unwrap();
    let query: BTreeMap<_, _> = url::Url::parse(requests[1].url())
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    assert!(!query["q"].contains("google-apps.folder"));
}
#[tokio::test]
async fn editable_folder_keeps_current_ancestry_strong_validator_and_single_dispatch_unknown_write_guards(
) {
    let (editable, script) = client(
        "notes",
        false,
        true,
        vec![
            member_metadata("notes", Some("\"http-1\"")),
            folder("notes", vec![]),
            Step::AcceptedThenLost,
        ],
    );
    let key = RecordKey::new("selected").unwrap();
    let body = MarkdownBytes::new(b"new".to_vec()).unwrap();
    let expected = DocumentVersion {
        provider_revision: OpaqueValue::new("1").unwrap(),
        content_hash: Some(
            ContentHash::from_hex(HashAlgorithm::Md5, "149603e6c03516362a8da23f624db945").unwrap(),
        ),
        conditional_write_token: Some(OpaqueValue::new("\"http-1\"").unwrap()),
    };
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    let binding = editable.descriptor().binding();
    let id = journal::prepare(&mut db, binding, &key, &expected, &body).unwrap();
    let permit = journal::begin_dispatch(&mut db, binding, id).unwrap();
    assert!(
        matches!(editable.guarded_write(permit, &body, &context()).await, Err(ProviderError::UnknownWrite { operation_id }) if operation_id == id)
    );
    journal::mark_unknown(&mut db, binding, id).unwrap();
    assert!(journal::begin_dispatch(&mut db, binding, id).is_err());
    assert_eq!(script.mutations.load(Ordering::SeqCst), 1);
    assert_eq!(
        script.requests.lock().unwrap()[2].headers()["if-match"],
        "\"http-1\""
    );
    let (moved, script) = client(
        "notes",
        false,
        true,
        vec![
            member_metadata("outside", Some("\"http-1\"")),
            folder("notes", vec![]),
        ],
    );
    let binding = moved.descriptor().binding();
    let id = journal::prepare(&mut db, binding, &key, &expected, &body).unwrap();
    let permit = journal::begin_dispatch(&mut db, binding, id).unwrap();
    assert_eq!(
        moved
            .guarded_write(permit, &body, &context())
            .await
            .unwrap_err(),
        ProviderError::Unauthorized
    );
    assert_eq!(script.mutations.load(Ordering::SeqCst), 0);
    let (readonly, script) = client("notes", false, false, vec![]);
    let binding = readonly.descriptor().binding();
    let id = journal::prepare(&mut db, binding, &key, &expected, &body).unwrap();
    let permit = journal::begin_dispatch(&mut db, binding, id).unwrap();
    assert_eq!(
        readonly
            .guarded_write(permit, &body, &context())
            .await
            .unwrap_err(),
        ProviderError::Unsupported
    );
    assert!(script.requests.lock().unwrap().is_empty());
}
