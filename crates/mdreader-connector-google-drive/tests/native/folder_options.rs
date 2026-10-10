use super::*;
fn reply(value: serde_json::Value) -> Step {
    Step::Reply(Response::new(200, BTreeMap::new(), serde_json::to_vec(&value).unwrap()).unwrap())
}
fn folder(id: &str, parents: Vec<&str>) -> Step {
    reply(
        serde_json::json!({"id":id,"mimeType":"application/vnd.google-apps.folder","version":"1","parents":parents}),
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
    let root_page = serde_json::json!({"files":[
        {"id":"direct","name":"Direct.MD","mimeType":"application/x-markdown","version":"1","parents":["actual-root"]},
        {"id":"ignored","name":"prefix.md.txt","mimeType":"text/plain","version":"1","parents":["actual-root"]},
        {"id":"native-doc","name":"Google.md","mimeType":"application/vnd.google-apps.document","version":"1","parents":["actual-root"]},
        {"id":"nested","name":"Nested","mimeType":"application/vnd.google-apps.folder","parents":["actual-root"]}
    ]});
    let nested_docs = serde_json::json!({"files":[{"id":"leaf","name":"Leaf.markdown","mimeType":"text/plain","version":"1","parents":["nested"]}]});
    let (recursive, script) = client(
        "root",
        true,
        false,
        vec![
            folder("actual-root", vec![]),
            reply(root_page),
            folder("actual-root", vec![]),
            folder("actual-root", vec![]),
            folder("nested", vec!["actual-root"]),
            reply(nested_docs),
            folder("actual-root", vec![]),
            folder("nested", vec!["actual-root"]),
        ],
    );
    let page = recursive.list(None, &context()).await.unwrap();
    assert_eq!(page.rows().len(), 1);
    assert_eq!(page.rows()[0].name.as_str(), "Direct.MD");
    let cursor = page.next_cursor().unwrap();
    let (other, other_script) = client("root", false, false, vec![]);
    assert!(other.list(Some(cursor), &context()).await.is_err());
    assert!(other_script.requests.lock().unwrap().is_empty());
    let nested = recursive.list(Some(cursor), &context()).await.unwrap();
    assert_eq!(nested.rows()[0].name.as_str(), "Leaf.markdown");
    assert!(nested.next_cursor().is_none());
    {
        let requests = script.requests.lock().unwrap();
        assert_eq!(requests.len(), 8);
        assert!(requests
            .iter()
            .all(|r| r.method() == Method::Get && !r.url().contains("alt=media")));
        let query: BTreeMap<_, _> = url::Url::parse(requests[1].url())
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        assert_eq!(query["q"], "trashed = false and 'actual-root' in parents");
        assert_eq!(query["pageSize"], "16");
    }
    let (direct, _) = client(
        "notes",
        false,
        false,
        vec![
            folder("notes", vec![]),
            reply(
                serde_json::json!({"files":[{"id":"direct","name":"Only.md","mimeType":"text/markdown","version":"1","parents":["notes"]}]}),
            ),
            folder("notes", vec![]),
        ],
    );
    assert!(direct
        .list(None, &context())
        .await
        .unwrap()
        .next_cursor()
        .is_none());
}

#[tokio::test]
async fn wide_drive_streams_more_than_64_folders_without_growing_cursor_or_downloading_bodies() {
    let mut steps = vec![];
    for batch in (0..100).step_by(16) {
        let end = (batch + 16).min(100);
        let children: Vec<_> = (batch..end).map(|i| serde_json::json!({"id":format!("folder_{i}"),"name":"Folder","mimeType":"application/vnd.google-apps.folder","parents":["actual-root"]})).collect();
        let mut page = serde_json::json!({"files":children});
        if end < 100 {
            page["nextPageToken"] = serde_json::json!(format!("page_{end}"));
        }
        steps.extend([
            folder("actual-root", vec![]),
            reply(page),
            folder("actual-root", vec![]),
        ]);
        for i in batch..end {
            let child = format!("folder_{i}");
            steps.extend([
                folder("actual-root", vec![]), folder(&child, vec!["actual-root"]),
                reply(serde_json::json!({"files":[{"id":format!("leaf_{i}"),"name":format!("Note-{i}.md"),"mimeType":"application/markdown","version":"1","parents":[child]}]})),
                folder("actual-root", vec![]), folder(&child, vec!["actual-root"]),
            ]);
        }
    }
    let (provider, script) = client("root", true, false, steps);
    let mut cursor = None;
    let mut documents = std::collections::BTreeSet::new();
    let mut pages = 0;
    loop {
        let page = provider.list(cursor.as_ref(), &context()).await.unwrap();
        for row in page.rows() {
            assert!(documents.insert(row.key.clone()));
        }
        pages += 1;
        cursor = page.next_cursor().cloned();
        if let Some(cursor) = &cursor {
            assert!(cursor.as_bytes().len() < 2048);
        } else {
            break;
        }
        assert!(pages < 400);
    }
    assert_eq!(documents.len(), 100);
    assert_eq!(pages, 107);
    assert_eq!(script.requests.lock().unwrap().len(), 521);
    assert!(script
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|r| r.method() == Method::Get && !r.url().contains("alt=media")));
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

#[tokio::test]
async fn folder_movement_during_listing_or_write_never_publishes_current_scope_success() {
    // Both list paths see a valid descendant before the remote request, then
    // discover the same folder moved outside the selected root afterwards.
    for browse in [false, true] {
        let page = if browse {
            serde_json::json!({"files":[{"id":"leaf-folder","name":"Leaf","mimeType":"application/vnd.google-apps.folder","version":"1","parents":["child"]}]})
        } else {
            serde_json::json!({"files":[{"id":"leaf","name":"Leaf.md","mimeType":"text/markdown","version":"1","parents":["child"]}]})
        };
        let steps = if browse {
            vec![
                folder("notes", vec![]),
                folder("child", vec!["notes"]),
                reply(page),
                folder("notes", vec![]),
                folder("child", vec!["outside"]),
                folder("outside", vec![]),
            ]
        } else {
            vec![
                folder("notes", vec![]),
                reply(
                    serde_json::json!({"files":[{"id":"child","name":"Child","mimeType":"application/vnd.google-apps.folder","parents":["notes"]}]}),
                ),
                folder("notes", vec![]),
                folder("notes", vec![]),
                folder("child", vec!["notes"]),
                reply(page),
                folder("notes", vec![]),
                folder("child", vec!["outside"]),
                folder("outside", vec![]),
            ]
        };
        let (reader, script) = client("notes", true, false, steps);
        let result = if browse {
            reader
                .folders(&RecordKey::new("child").unwrap(), None, &context())
                .await
                .map(|_| ())
        } else {
            let first = reader.list(None, &context()).await.unwrap();
            reader
                .list(first.next_cursor(), &context())
                .await
                .map(|_| ())
        };
        assert_eq!(result, Err(ProviderError::Unauthorized));
        assert_eq!(script.mutations.load(Ordering::SeqCst), 0);
    }
    let accepted = Step::Reply(metadata("2", Some("\"http-2\""), b"new"));
    let mut moved: serde_json::Value =
        serde_json::from_slice(metadata("2", Some("\"http-2\""), b"new").body()).unwrap();
    moved["parents"] = serde_json::json!(["child"]);
    let (writer, script) = client(
        "notes",
        true,
        true,
        vec![
            member_metadata("child", Some("\"http-1\"")),
            folder("notes", vec![]),
            folder("child", vec!["notes"]),
            accepted,
            reply(moved),
            folder("notes", vec![]),
            folder("child", vec!["outside"]),
            folder("outside", vec![]),
        ],
    );
    let body = MarkdownBytes::new(b"new".to_vec()).unwrap();
    let expected = DocumentVersion {
        provider_revision: OpaqueValue::new("1").unwrap(),
        content_hash: Some(
            ContentHash::from_hex(HashAlgorithm::Md5, "149603e6c03516362a8da23f624db945").unwrap(),
        ),
        conditional_write_token: Some(OpaqueValue::new("\"http-1\"").unwrap()),
    };
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    let binding = writer.descriptor().binding();
    let key = RecordKey::new("selected").unwrap();
    let operation = journal::prepare(&mut db, binding, &key, &expected, &body).unwrap();
    let permit = journal::begin_dispatch(&mut db, binding, operation).unwrap();
    assert!(
        matches!(writer.guarded_write(permit,&body,&context()).await,Err(ProviderError::UnknownWrite {operation_id}) if operation_id==operation)
    );
    journal::mark_unknown(&mut db, binding, operation).unwrap();
    assert!(journal::begin_dispatch(&mut db, binding, operation).is_err());
    assert_eq!(script.mutations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn old_signed_traversal_restarts_only_staging_before_any_network_request() {
    let (provider, script) = client("notes", true, false, vec![]);
    let cursor = CursorCodec::new([7; 32])
        .encode_traversal(
            ProviderKind::GoogleDrive,
            provider.descriptor().binding(),
            CursorKind::List,
            serde_json::json!({"v":2,"root":"notes","stack":[{"folder":"notes","phase":"documents","token":"next"}]}).to_string(),
        )
        .unwrap();
    assert_eq!(
        provider.list(Some(&cursor), &context()).await.unwrap_err(),
        ProviderError::ResetCursor
    );
    assert!(script.requests.lock().unwrap().is_empty());
}
