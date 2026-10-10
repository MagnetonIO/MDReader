use super::*;
use serde_json::{Value, json};

struct Model {
    files: Mutex<BTreeMap<String, Value>>,
    feed: Mutex<Value>,
    requests: Mutex<Vec<Request>>,
}
fn file(id: &str, name: &str, parent: &str, version: &str) -> Value {
    json!({"id":id,"name":name,"mimeType":"text/plain","version":version,"parents":[parent],"modifiedTime":"2026-10-10T12:00:00Z"})
}
fn dir(id: &str, parent: Option<&str>) -> Value {
    json!({"id":id,"name":id,"mimeType":"application/vnd.google-apps.folder","parents":parent.into_iter().collect::<Vec<_>>()})
}
#[async_trait]
impl Transport for Model {
    async fn send(&self, request: Request) -> Result<Response> {
        assert_eq!(request.method(), Method::Get);
        let url = url::Url::parse(request.url()).unwrap();
        assert!(!url.query_pairs().any(|(key, _)| key == "alt"));
        self.requests.lock().unwrap().push(request);
        let value = if url.path().ends_with("/startPageToken") {
            json!({"startPageToken":"start"})
        } else if url.path().ends_with("/changes") {
            self.feed.lock().unwrap().clone()
        } else if url.path().ends_with("/files") {
            let q = url
                .query_pairs()
                .find(|(key, _)| key == "q")
                .unwrap()
                .1
                .into_owned();
            let parent = q.split('\'').nth(1).unwrap();
            let rows: Vec<_> = self
                .files
                .lock()
                .unwrap()
                .values()
                .filter(|file| {
                    file["parents"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|p| p == parent)
                        && file["trashed"] != true
                })
                .cloned()
                .collect();
            json!({"files":rows})
        } else {
            let key = url.path_segments().unwrap().next_back().unwrap();
            match self.files.lock().unwrap().get(key) {
                Some(file) => file.clone(),
                None => return Response::new(404, BTreeMap::new(), vec![]),
            }
        };
        Response::new(200, BTreeMap::new(), serde_json::to_vec(&value).unwrap())
    }
}
fn model() -> Arc<Model> {
    Arc::new(Model {
        files: Mutex::new(BTreeMap::from([
            ("notes".into(), dir("notes", None)),
            ("sub".into(), dir("sub", Some("notes"))),
            ("outside".into(), dir("outside", None)),
            ("a".into(), file("a", "A.md", "notes", "1")),
            ("b".into(), file("b", "B.markdown", "sub", "1")),
            (
                "unrelated".into(),
                file("unrelated", "Private.md", "outside", "1"),
            ),
        ])),
        feed: Mutex::new(json!({"changes":[],"newStartPageToken":"end"})),
        requests: Mutex::new(vec![]),
    })
}
fn opened(descriptor: SourceDescriptor, model: Arc<Model>, known: Vec<Metadata>) -> Client {
    Client::new_folder(
        descriptor,
        RecordKey::new("notes").unwrap(),
        true,
        Arc::new(Resolver(AtomicUsize::new(0))),
        Arc::new(Authority(AtomicBool::new(true))),
        model,
        [7; 32],
    )
    .unwrap()
    .with_known_metadata(known)
    .unwrap()
}
fn fresh(model: Arc<Model>) -> Client {
    let (template, _, _, _) = fixture(vec![]);
    let scope = SelectedScope::DriveFolder {
        root: RecordKey::new("notes").unwrap(),
        recursive: true,
    };
    let old = template.descriptor();
    let binding = SourceBinding::new(
        old.binding().library_id().clone(),
        old.binding().source_id().clone(),
        old.binding().account().clone(),
        uuid::Uuid::new_v4(),
        scope.digest(),
    )
    .unwrap();
    let descriptor = SourceDescriptor::new(
        binding,
        ProviderKind::GoogleDrive,
        DisplayText::new("Public notes").unwrap(),
        scope,
        ProviderCapabilities {
            read: true,
            list: true,
            changes: true,
            assets: false,
            guarded_write: false,
        },
        SourceHealth::Ready,
        Coverage::NeverIndexed,
    )
    .unwrap();
    opened(descriptor, model, vec![])
}
async fn census(client: &Client) -> Vec<Metadata> {
    let mut rows = Vec::new();
    let mut cursor = None;
    loop {
        let page = client.list(cursor.as_ref(), &context()).await.unwrap();
        rows.extend_from_slice(page.rows());
        cursor = page.next_cursor().cloned();
        if cursor.is_none() {
            return rows;
        }
    }
}
fn changed(file: &Value) -> Value {
    json!({"fileId":file["id"],"file":file})
}
fn keys(events: &[Change]) -> Vec<(String, bool)> {
    events
        .iter()
        .map(|event| match event {
            Change::Upsert { key } => (key.as_str().to_owned(), true),
            Change::Deleted { key, .. } => (key.as_str().to_owned(), false),
        })
        .collect()
}

#[tokio::test]
async fn folder_delta_admits_add_revision_rename_move_and_delete_only_within_existing_grant() {
    let model = model();
    let initial = fresh(model.clone());
    let known = census(&initial).await;
    assert_eq!(known.len(), 2);
    assert!(
        known
            .iter()
            .all(|row| row.membership_ancestors.as_ref().unwrap()[0].as_str() == "notes")
    );
    let client = opened(initial.descriptor().clone(), model.clone(), known);
    let start = client.start_changes(&context()).await.unwrap();
    let a = file("a", "Renamed.MD", "notes", "2");
    let b = file("b", "Moved.markdown", "outside", "2");
    let c = file("c", "New.md", "notes", "1");
    let outside = file("unrelated", "Unrelated.md", "outside", "2");
    {
        let mut files = model.files.lock().unwrap();
        for row in [&a, &b, &c, &outside] {
            files.insert(row["id"].as_str().unwrap().into(), row.clone());
        }
    }
    *model.feed.lock().unwrap() = json!({"changes":[changed(&a),changed(&b),changed(&c),changed(&outside),{"fileId":"removed","removed":true}],"newStartPageToken":"end"});
    let ChangeOutcome::Page {
        events,
        checkpoint,
        next_cursor,
    } = client.changes(&start, &context()).await.unwrap()
    else {
        panic!("unexpected rebuild")
    };
    assert_eq!(
        keys(&events),
        vec![
            ("a".into(), true),
            ("b".into(), false),
            ("c".into(), true),
            ("unrelated".into(), false),
            ("removed".into(), false)
        ]
    );
    assert!(checkpoint.is_some() && next_cursor.is_none());
    let current = client
        .metadata(&RecordKey::new("a").unwrap(), &context())
        .await
        .unwrap();
    assert_eq!(current.name.as_str(), "Renamed.MD");
    assert_eq!(current.version.provider_revision.as_str(), "2");
    *model.feed.lock().unwrap() =
        json!({"changes":[{"fileId":"a","removed":true}],"newStartPageToken":"later"});
    let ChangeOutcome::Page { events, .. } = client
        .changes(checkpoint.as_ref().unwrap(), &context())
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(keys(&events), vec![("a".into(), false)]);
    assert!(!client.descriptor().capabilities().guarded_write);
    assert!(
        model
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.method() == Method::Get && !r.url().contains("alt=media"))
    );
}

#[tokio::test]
async fn folder_structure_reconciles_only_affected_subtrees_and_signed_work_survives_sqlite_reopen()
{
    let model = model();
    let initial = fresh(model.clone());
    let known = census(&initial).await;
    let descriptor = initial.descriptor().clone();
    let mut client = opened(descriptor.clone(), model.clone(), known.clone());
    let mut cursor = client.start_changes(&context()).await.unwrap();
    let moved = dir("sub", Some("outside"));
    let incoming = dir("incoming", Some("notes"));
    {
        let mut files = model.files.lock().unwrap();
        files.insert("sub".into(), moved.clone());
        files.insert("incoming".into(), incoming.clone());
        files.insert(
            "new-leaf".into(),
            file("new-leaf", "Fresh.md", "incoming", "1"),
        );
    }
    *model.feed.lock().unwrap() =
        json!({"changes":[changed(&moved),changed(&incoming)],"newStartPageToken":"after-folders"});
    model.requests.lock().unwrap().clear();
    let mut result = Vec::new();
    let mut completed = false;
    for turn in 0..12 {
        let ChangeOutcome::Page {
            events,
            next_cursor,
            checkpoint,
        } = client.changes(&cursor, &context()).await.unwrap()
        else {
            panic!("structural change must stay incremental")
        };
        result.extend(keys(&events));
        if checkpoint.is_some() {
            completed = true;
            break;
        }
        cursor = next_cursor.unwrap();
        assert!(cursor.as_bytes().len() <= 32 * 1024);
        if turn == 0 {
            let path = std::env::temp_dir().join(format!(
                "mdreader-drive-delta-{}.sqlite",
                uuid::Uuid::new_v4()
            ));
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute_batch("CREATE TABLE retained(value BLOB NOT NULL)")
                .unwrap();
            db.execute("INSERT INTO retained VALUES(?1)", [cursor.as_bytes()])
                .unwrap();
            drop(db);
            let db = rusqlite::Connection::open(&path).unwrap();
            let value: Vec<u8> = db
                .query_row("SELECT value FROM retained", [], |r| r.get(0))
                .unwrap();
            cursor = ProviderCursor::new(value).unwrap();
            client = opened(descriptor.clone(), model.clone(), known.clone());
            drop(db);
            std::fs::remove_file(path).unwrap();
        }
    }
    assert!(completed);
    assert_eq!(result, vec![("b".into(), false), ("new-leaf".into(), true)]);
    let requests = model.requests.lock().unwrap();
    let enumerations: Vec<_> = requests
        .iter()
        .filter(|r| url::Url::parse(r.url()).unwrap().path().ends_with("/files"))
        .collect();
    assert_eq!(enumerations.len(), 1);
    let url = url::Url::parse(enumerations[0].url()).unwrap();
    assert!(
        url.query_pairs()
            .any(|(key, value)| key == "q" && value == "trashed = false and 'incoming' in parents")
    );
    assert!(!requests.iter().any(|r| r.url().contains("/files/a?")));
    assert!(
        requests
            .iter()
            .all(|r| r.method() == Method::Get && !r.url().contains("alt=media"))
    );
}

#[tokio::test]
async fn legacy_unknown_ancestry_rebuilds_only_when_structural_membership_proof_is_missing() {
    let model = model();
    let initial = fresh(model.clone());
    let mut known = census(&initial).await;
    for row in &mut known {
        row.membership_ancestors = None;
    }
    let client = opened(initial.descriptor().clone(), model.clone(), known);
    let start = client.start_changes(&context()).await.unwrap();
    let a = file("a", "A.md", "notes", "2");
    model.files.lock().unwrap().insert("a".into(), a.clone());
    *model.feed.lock().unwrap() = json!({"changes":[changed(&a)],"newStartPageToken":"ordinary"});
    assert!(matches!(
        client.changes(&start, &context()).await.unwrap(),
        ChangeOutcome::Page { .. }
    ));
    *model.feed.lock().unwrap() =
        json!({"changes":[changed(&dir("sub",Some("notes")))],"newStartPageToken":"structural"});
    assert!(matches!(
        client.changes(&start, &context()).await.unwrap(),
        ChangeOutcome::RebuildRequired
    ));
}

#[tokio::test]
async fn cold_committed_observations_reconcile_structural_removals_before_first_publication() {
    let model = model();
    let client = fresh(model.clone());
    let mut cursor = None;
    loop {
        let page = client.list(cursor.as_ref(), &context()).await.unwrap();
        client
            .retain_metadata_observation(page.rows(), cursor.is_none())
            .unwrap();
        cursor = page.next_cursor().cloned();
        if cursor.is_none() {
            break;
        }
    }
    let start = client.start_changes(&context()).await.unwrap();
    let moved = dir("sub", Some("outside"));
    model
        .files
        .lock()
        .unwrap()
        .insert("sub".into(), moved.clone());
    *model.feed.lock().unwrap() = json!({"changes":[changed(&moved),{"fileId":"a","removed":true}],"newStartPageToken":"cold-end"});
    let mut cursor = start;
    let mut events = Vec::new();
    let mut completed = false;
    for _ in 0..6 {
        let ChangeOutcome::Page {
            events: page,
            next_cursor,
            checkpoint,
        } = client.changes(&cursor, &context()).await.unwrap()
        else {
            panic!("cold membership was retained")
        };
        events.extend(keys(&page));
        if checkpoint.is_some() {
            completed = true;
            break;
        }
        cursor = next_cursor.unwrap();
    }
    assert!(completed);
    assert_eq!(events, vec![("b".into(), false), ("a".into(), false)]);
}

#[tokio::test]
async fn committed_delta_ancestry_is_retained_for_the_next_folder_event_in_same_client() {
    let model = model();
    let client = fresh(model.clone());
    let start = client.start_changes(&context()).await.unwrap();
    let leaf = file("new-leaf", "Fresh.md", "sub", "1");
    model
        .files
        .lock()
        .unwrap()
        .insert("new-leaf".into(), leaf.clone());
    *model.feed.lock().unwrap() = json!({"changes":[changed(&leaf)],"newStartPageToken":"first"});
    let ChangeOutcome::Page {
        events,
        checkpoint: Some(next),
        ..
    } = client.changes(&start, &context()).await.unwrap()
    else {
        panic!("expected file delta")
    };
    assert_eq!(keys(&events), vec![("new-leaf".into(), true)]);
    let metadata = client
        .metadata(&RecordKey::new("new-leaf").unwrap(), &context())
        .await
        .unwrap();
    client.retain_metadata_changes(&[metadata], &[]).unwrap();
    let moved = dir("sub", Some("outside"));
    model
        .files
        .lock()
        .unwrap()
        .insert("sub".into(), moved.clone());
    *model.feed.lock().unwrap() = json!({"changes":[changed(&moved)],"newStartPageToken":"second"});
    let ChangeOutcome::Page {
        next_cursor: Some(cursor),
        ..
    } = client.changes(&next, &context()).await.unwrap()
    else {
        panic!("expected structural continuation")
    };
    let ChangeOutcome::Page { events, .. } = client.changes(&cursor, &context()).await.unwrap()
    else {
        panic!("expected exact old descendant reconciliation")
    };
    assert_eq!(keys(&events), vec![("new-leaf".into(), false)]);
    client
        .retain_metadata_changes(&[], &[RecordKey::new("new-leaf").unwrap()])
        .unwrap();
}
