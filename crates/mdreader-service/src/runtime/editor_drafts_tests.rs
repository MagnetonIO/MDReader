use super::*;
use mdreader_core::editor_drafts::{DraftInput, DraftKey, DraftRequest};
use std::time::Instant;
#[test]
fn drafts_are_local_owner_effects_and_refuse_during_preservation_freeze() {
    let directory = tempfile::tempdir().unwrap();
    let owner = Arc::new(Mutex::new(ReaderStore::open(directory.path()).unwrap()));
    let runtime = Runtime::new(
        owner.clone(),
        directory.path().join("model"),
        crate::ServiceOptions {
            disable_model: true,
        },
    )
    .unwrap();
    let key = DraftKey::Device {
        document_id: "public-recovery-key-without-grant".into(),
    };
    let checkpoint = Request::EditorDrafts {
        request: DraftRequest::Checkpoint {
            key: key.clone(),
            expected_generation: 0,
            draft: DraftInput {
                base_revision: "original".into(),
                base_hash: None,
                title: "Public note.md".into(),
                markdown: "unsaved public text".into(),
            },
        },
    };
    // The uncomposed dispatcher cannot bypass the service's effect gate.
    assert!(crate::execute(&mut owner.lock().unwrap(), checkpoint.clone()).is_err());
    let saved = runtime.execute(checkpoint).unwrap();
    let draft_id = saved["slot"]["draft_id"].as_str().unwrap().to_owned();
    let gen = saved["slot"]["generation"].as_u64().unwrap();
    let admission = runtime.capture_participant().unwrap();
    let operation = uuid::Uuid::new_v4();
    let epoch = uuid::Uuid::new_v4();
    let frozen = admission
        .freeze(operation, epoch, Instant::now() + Duration::from_secs(2))
        .unwrap();
    for request in [
        DraftRequest::List,
        DraftRequest::Inspect { key },
        DraftRequest::Read {
            draft_id: draft_id.clone(),
            generation: gen,
        },
        DraftRequest::Discard {
            draft_id: draft_id.clone(),
            expected_generation: gen,
        },
    ] {
        assert!(runtime.execute(Request::EditorDrafts { request }).is_err());
    }
    frozen
        .revalidate(admission, &owner, operation, epoch)
        .unwrap();
    drop(frozen);
    let restored = runtime
        .execute(Request::EditorDrafts {
            request: DraftRequest::Read {
                draft_id,
                generation: gen,
            },
        })
        .unwrap();
    assert_eq!(restored["markdown"], "unsaved public text");
    assert!(owner.lock().unwrap().list_sources().unwrap().is_empty());
}
