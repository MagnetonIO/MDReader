#![cfg(unix)]
use mdreader_core::ReaderStore;
use mdreader_provider::*;
use mdreader_service::{
    catalog::*,
    provider::{self, Owner},
};
use std::sync::{Arc, Mutex};
#[test]
fn provider_catalog_is_selected_metadata_only_cache_fenced_and_exact_readable_without_a_factory() {
    exercise_provider_catalog(false);
}
#[test]
fn cold_provider_catalog_admits_exact_body_before_listing_finishes() {
    exercise_provider_catalog(true);
}
fn exercise_provider_catalog(partial: bool) {
    let t = tempfile::tempdir().unwrap();
    let owner = Arc::new(Mutex::new(ReaderStore::open(t.path()).unwrap()));
    let library = owner.with_connection(provider::library_id).unwrap();
    let keys = vec![
        RecordKey::new("private-key-a").unwrap(),
        RecordKey::new("private-key-b").unwrap(),
    ];
    let scope = SelectedScope::exact(keys.clone()).unwrap();
    let descriptor = SourceDescriptor::new(
        SourceBinding::new(
            library.clone(),
            SourceId::new("selected").unwrap(),
            AccountBinding::new("private-account").unwrap(),
            uuid::Uuid::new_v4(),
            scope.digest(),
        )
        .unwrap(),
        ProviderKind::GoogleDrive,
        DisplayText::new("Selected").unwrap(),
        scope,
        ProviderCapabilities {
            read: true,
            list: true,
            changes: false,
            assets: false,
            guarded_write: false,
        },
        SourceHealth::Ready,
        Coverage::NeverIndexed,
    )
    .unwrap();
    let projection = provider::CoreProjection::open(&t.path().join("cache"), library).unwrap();
    let mut metadata = vec![];
    let mut bodies = vec![];
    for (i, key) in keys.iter().enumerate() {
        let body = format!("# Plan {i}\nPrivate body {i}");
        let version = DocumentVersion {
            provider_revision: OpaqueValue::new(format!("private-version-{i}")).unwrap(),
            content_hash: Some(
                ContentHash::new(HashAlgorithm::Sha256, body_hash(body.as_bytes()).to_vec())
                    .unwrap(),
            ),
            conditional_write_token: None,
        };
        metadata.push(Metadata {
            membership_ancestors: None,
            key: key.clone(),
            name: DisplayText::new(format!("Plan{i}.md")).unwrap(),
            display_path: DisplayText::new(format!("Plan{i}.md")).unwrap(),
            media_type: None,
            modified_at_ms: Some(1),
            size_bytes: Some(body.len() as u64),
            version: version.clone(),
        });
        bodies.push(DocumentBody {
            key: key.clone(),
            version,
            body: MarkdownBytes::new(body.into_bytes()).unwrap(),
            observed_at_ms: 1,
        });
    }
    let staged = owner
        .with_connection(|c| {
            provider::admit(
                c,
                &descriptor,
                &OpaqueValue::new("private-vault-reference").unwrap(),
                [7; 32],
            )?;
            let snapshot = provider::begin_snapshot(c, descriptor.binding(), None)?;
            provider::stage_page(
                c,
                &snapshot,
                None,
                &MetadataPage::new(
                    metadata.clone(),
                    None,
                    1,
                    Coverage::MetadataCompleteSelected { observed_at_ms: 1 },
                )?,
            )?;
            if !partial {
                provider::publish_snapshot(c, &snapshot, &projection)?;
            }
            Ok(snapshot)
        })
        .unwrap();
    let catalog = Coordinator::new(
        owner.clone(),
        Arc::new(Mutex::new(ReaderStore::open_readonly(t.path()).unwrap())),
    );
    let get = |cursor| {
        catalog.execute(CatalogRequest::Page {
            catalog_id: "remote:selected".into(),
            cursor,
            limit: 1,
        })
    };
    let CatalogReply::Page {
        documents,
        next_cursor,
        ..
    } = get(None).unwrap()
    else {
        panic!()
    };
    assert!(
        matches!(&documents[0].open,CatalogOpen::SelectedUncached{source_id,reader_source_id,..}if source_id=="selected"&&reader_source_id.starts_with("provider:"))
    );
    let serialized = serde_json::to_string(&documents).unwrap();
    for private in [
        "private-key",
        "private-account",
        "private-version",
        "private-vault",
        "Private body",
    ] {
        assert!(!serialized.contains(private));
    }
    let receipt = owner
        .with_connection(|c| provider::admit_body(c, descriptor.binding(), &bodies[0], &projection))
        .unwrap();
    assert!(matches!(get(next_cursor), Err(CatalogError::Stale)));
    let CatalogReply::Page {
        documents,
        next_cursor,
        ..
    } = get(None).unwrap()
    else {
        panic!()
    };
    assert!(
        matches!(&documents[0].open,CatalogOpen::Exact{source_id,path,revision}if source_id==receipt.reader_source_id.as_str()&&path==receipt.path.as_str()&&revision==receipt.revision.as_str())
    );
    let body = owner
        .lock()
        .unwrap()
        .read_document(
            receipt.reader_source_id.as_str(),
            receipt.path.as_str(),
            Some(receipt.revision.as_str()),
        )
        .unwrap();
    assert_eq!(body.markdown.as_bytes(), bodies[0].body.as_bytes());
    if partial {
        // Publishing the finished scan reconciles all admitted cold bodies.
        owner
            .with_connection(|c| provider::publish_snapshot(c, &staged, &projection))
            .unwrap();
        assert!(get(None).is_ok());
    }
    owner
        .with_connection(|c| provider::disable(c, descriptor.binding(), &projection))
        .unwrap();
    assert!(get(next_cursor).is_err());
    assert!(owner
        .lock()
        .unwrap()
        .read_document(
            receipt.reader_source_id.as_str(),
            receipt.path.as_str(),
            Some(receipt.revision.as_str())
        )
        .is_err());
}
