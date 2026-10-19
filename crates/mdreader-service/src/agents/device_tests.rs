use super::*;
use crate::{
    accounts::{AccountError, CacheRoots},
    runtime::Runtime,
    NativeAdapters, Request, ServiceOptions,
};
use std::sync::{Arc, Mutex};
fn make_runtime(store: ReaderStore, temp: &tempfile::TempDir) -> Runtime {
    Runtime::new_with_adapters(
        Arc::new(Mutex::new(store)),
        temp.path().join("reader/models"),
        ServiceOptions {
            disable_model: true,
        },
        NativeAdapters {
            factory: None,
            accounts: Some((
                Err(AccountError::Invalid),
                CacheRoots {
                    documents: temp.path().join("documents"),
                    assets: temp.path().join("assets"),
                },
            )),
        },
    )
    .unwrap()
}
fn call(
    runtime: &Runtime,
    grant: &str,
    request: AgentRequest,
) -> mdreader_core::Result<serde_json::Value> {
    runtime.execute(Request::AgentCall {
        grant_id: grant.into(),
        request,
    })
}
#[test]
fn device_update_replays_after_reopen_transfers_only_issuing_grant_and_fences_revoke() {
    let (temp, mut store, _, id) = fixture();
    let document = mdreader_core::device::import(
        &mut store,
        &temp.path().join("documents"),
        "Public.md".into(),
        "# Original".into(),
    )
    .unwrap();
    permissions::set(
        &mut store,
        &id,
        true,
        &[],
        std::slice::from_ref(&document.source_id),
        std::slice::from_ref(&document.source_id),
    )
    .unwrap();
    let mut other = grants::record(&mut store, &id).unwrap().unwrap();
    other.selection.agent = Agent::Claude;
    let other_id = other.selection.id().unwrap();
    grants::save_install(&mut store, &other_id, &other).unwrap();
    permissions::set(
        &mut store,
        &other_id,
        true,
        &[],
        std::slice::from_ref(&document.source_id),
        std::slice::from_ref(&document.source_id),
    )
    .unwrap();
    let runtime = make_runtime(store, &temp);
    let operation_id = uuid::Uuid::new_v4().to_string();
    let request = AgentRequest::UpdateDocument {
        source_id: document.source_id.clone(),
        path: "Public.md".into(),
        operation_id: operation_id.clone(),
        expected_revision: document.revision,
        markdown: "# Changed".into(),
    };
    let saved = call(&runtime, &id, request.clone()).unwrap();
    assert_eq!(saved["state"], "saved");
    let new_source = saved["document"]["receipt"]["source_id"]
        .as_str()
        .unwrap()
        .to_string();
    let new_revision = saved["document"]["receipt"]["revision"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(new_source, document.source_id);
    {
        let mut owner = runtime.store().unwrap();
        assert_eq!(sources(&mut owner, &id).unwrap(), vec![new_source.clone()]);
        assert_eq!(
            permissions::get(&mut owner, &id).unwrap().edits,
            vec![new_source.clone()]
        );
        assert_eq!(
            permissions::get(&mut owner, &other_id).unwrap().edits,
            vec![document.source_id.clone()]
        );
        assert!(documents::fence(&mut owner, &other_id, &new_source).is_err());
    }
    drop(runtime);
    let store = ReaderStore::open(&temp.path().join("reader")).unwrap();
    let reopened = make_runtime(store, &temp);
    assert_eq!(call(&reopened, &id, request.clone()).unwrap(), saved);
    assert_eq!(
        call(
            &reopened,
            &id,
            AgentRequest::InspectOperation {
                operation_id: operation_id.clone()
            }
        )
        .unwrap(),
        saved
    );
    let mut changed = request;
    if let AgentRequest::UpdateDocument { markdown, .. } = &mut changed {
        *markdown = "# Mismatched replay".into();
    }
    assert!(matches!(
        call(&reopened, &id, changed),
        Err(ReaderError::Conflict)
    ));
    let next_request = AgentRequest::UpdateDocument {
        source_id: new_source,
        path: "Public.md".into(),
        operation_id: uuid::Uuid::new_v4().to_string(),
        expected_revision: new_revision,
        markdown: "# Second change".into(),
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let next = loop {
        match call(&reopened, &id, next_request.clone()) {
            Ok(value) => break value,
            Err(ReaderError::Unavailable) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            Err(error) => panic!("exact operation retry failed: {error}"),
        }
    };
    assert_eq!(next["state"], "saved");
    permissions::set(&mut reopened.store().unwrap(), &id, false, &[], &[], &[]).unwrap();
    assert!(call(
        &reopened,
        &id,
        AgentRequest::InspectOperation { operation_id }
    )
    .is_err());
}
#[test]
fn create_retains_original_scope_and_grants_only_the_exact_created_document() {
    let (temp, mut store, _, id) = fixture();
    let original = mdreader_core::device::import(
        &mut store,
        &temp.path().join("documents"),
        "Original.md".into(),
        "# Original".into(),
    )
    .unwrap();
    permissions::set(
        &mut store,
        &id,
        true,
        &[],
        &[],
        std::slice::from_ref(&original.source_id),
    )
    .unwrap();
    let runtime = make_runtime(store, &temp);
    let operation_id = uuid::Uuid::new_v4().to_string();
    let request = AgentRequest::CreateDocument {
        source_id: original.source_id.clone(),
        path: "New.md".into(),
        operation_id: operation_id.clone(),
        markdown: "# New".into(),
    };
    let saved = call(&runtime, &id, request.clone()).unwrap();
    assert_eq!(call(&runtime, &id, request).unwrap(), saved);
    let source = saved["document"]["receipt"]["source_id"].as_str().unwrap();
    let mut owner = runtime.store().unwrap();
    assert!(grants::grant(&mut owner, &id).unwrap().1.is_empty());
    let edits = permissions::get(&mut owner, &id).unwrap().edits;
    assert!(edits.contains(&original.source_id) && edits.contains(&source.to_string()));
    assert_eq!(edits.len(), 2);
}
#[test]
fn editable_sources_read_the_device_list_once_however_many_device_sources_exist() {
    let (temp, mut store, project, _) = fixture();
    let mut ids = vec![project.clone()];
    for i in 0..12 {
        ids.push(
            mdreader_core::device::import(
                &mut store,
                &temp.path().join("documents"),
                format!("Public {i}.md"),
                "# Public".into(),
            )
            .unwrap()
            .source_id,
        );
    }
    documents::DEVICE_LISTS.with(|count| count.set(0));
    let editable = documents::editable_ids(&mut store, &ids).unwrap();
    assert_eq!(documents::DEVICE_LISTS.with(|count| count.get()), 1);
    let expected: Vec<String> = ids
        .iter()
        .filter(|id| documents::editable(&mut store, id).unwrap())
        .cloned()
        .collect();
    assert_eq!(editable, expected);
    assert_eq!(editable.len(), 13);
}
