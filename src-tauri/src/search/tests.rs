use super::*;
use std::fs;
use tempfile::TempDir;
fn workspace_dir() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    (dir, root)
}

fn retained(path: &Path) -> SourceRoot {
    SourceRoot::capture(path.to_path_buf()).expect("capture source root")
}

#[test]
fn basename_takes_the_final_component() {
    assert_eq!(basename("notes/deep/page.md"), "page.md");
    assert_eq!(basename("page.md"), "page.md");
}

#[cfg(unix)]
fn provider_search_fixture(
    reader: &crate::reader::ReaderBackend,
    directory: &Path,
) -> mdreader_core::ProviderProjectionReceipt {
    use mdreader_provider::*;
    let binding = SourceBinding::new(
        LibraryId::new("synthetic-search-library").unwrap(),
        SourceId::new("selected-drive").unwrap(),
        AccountBinding::new("private-account").unwrap(),
        uuid::Uuid::new_v4(),
        "1".repeat(64),
    )
    .unwrap();
    let text = "# Provider plan\n\nquartz storage evidence\n";
    let version = DocumentVersion {
        provider_revision: OpaqueValue::new("private-remote-version").unwrap(),
        content_hash: None,
        conditional_write_token: None,
    };
    let metadata = Metadata {
        key: RecordKey::new("private-record").unwrap(),
        name: DisplayText::new("Plan.md").unwrap(),
        display_path: DisplayText::new("Plan.md").unwrap(),
        media_type: None,
        modified_at_ms: Some(1),
        size_bytes: Some(text.len() as u64),
        version: version.clone(),
    };
    let body = DocumentBody {
        key: metadata.key.clone(),
        version,
        body: MarkdownBytes::new(text.as_bytes().to_vec()).unwrap(),
        observed_at_ms: 1,
    };
    let projection = mdreader_core::ProviderProjection::open(
        &directory.join("provider-cache"),
        binding.library_id().clone(),
    )
    .unwrap();
    reader
        .with_mobile_store(|store| {
            let receipt = store.with_library_connection(|c| {
                let tx = c.transaction().unwrap();
                let receipt = projection.admit(&tx, &binding, &metadata, &body).unwrap();
                tx.commit().unwrap();
                receipt
            });
            store
                .index_source(&receipt.reader_source_id)
                .map_err(|e| e.to_string())?;
            Ok(receipt)
        })
        .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn selected_provider_search_without_workspace_keeps_canonical_revision_and_rejects_disabled_scope(
) {
    let directory = TempDir::new().unwrap();
    let reader = crate::reader::ReaderBackend::embedded(directory.path()).unwrap();
    let receipt = provider_search_fixture(&reader, directory.path());
    let workspace = WorkspaceState::default();
    let options:SearchQuery=serde_json::from_value(serde_json::json!({"limit":20,"lexical_only":true,"explain":true,"intent":"exact","provider_sources":[receipt.reader_source_id]})).unwrap();
    let outcome = search_reader(&reader, &workspace, "quartz", &options, |_| {})
        .await
        .expect("provider-only search must not require a local folder");
    assert_eq!(outcome.hits.len(), 1);
    let hit = &outcome.hits[0];
    assert_eq!(
        hit.source_id.as_deref(),
        Some(receipt.reader_source_id.as_str())
    );
    assert_eq!(hit.revision.as_deref(), Some(receipt.revision.as_str()));
    assert_eq!(
        hit.abs_path, receipt.path,
        "provider paths are never joined to local/cache roots"
    );
    assert_eq!(hit.explain.as_ref().unwrap().provenance.source_root, None);
    reader
        .with_mobile_store(|store| {
            store
                .set_source_enabled(&receipt.reader_source_id, false)
                .map_err(|e| e.to_string())
        })
        .unwrap();
    assert!(
        search_reader(&reader, &workspace, "quartz", &options, |_| {})
            .await
            .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn mixed_selected_scope_is_ranked_together_and_unselected_provider_is_excluded() {
    let directory = TempDir::new().unwrap();
    let local = TempDir::new().unwrap();
    fs::write(
        local.path().join("local.md"),
        "# Local plan\n\nquartz storage evidence\n",
    )
    .unwrap();
    let reader = crate::reader::ReaderBackend::embedded(directory.path()).unwrap();
    let receipt = provider_search_fixture(&reader, directory.path());
    let workspace = WorkspaceState::granted(local.path().canonicalize().unwrap());
    let id = reader
        .admit_async(workspace.current().unwrap().source_root())
        .await
        .unwrap();
    reader
        .with_mobile_store(|store| store.index_source(&id).map_err(|e| e.to_string()))
        .unwrap();
    let local_options: SearchQuery = serde_json::from_value(
        serde_json::json!({"limit":20,"lexical_only":true,"explain":false,"intent":"exact"}),
    )
    .unwrap();
    let local_only = search_reader(&reader, &workspace, "quartz", &local_options, |_| {})
        .await
        .unwrap();
    assert_eq!(local_only.hits.len(), 1);
    assert_eq!(local_only.hits[0].source_id.as_deref(), Some(id.as_str()));
    let mixed_options:SearchQuery=serde_json::from_value(serde_json::json!({"limit":20,"lexical_only":true,"explain":false,"intent":"exact","provider_sources":[receipt.reader_source_id]})).unwrap();
    let mixed = search_reader(&reader, &workspace, "quartz", &mixed_options, |_| {})
        .await
        .unwrap();
    assert_eq!(mixed.hits.len(), 2);
    let ranks: Vec<_> = mixed.hits.iter().map(|h| h.lex_rank.unwrap()).collect();
    assert_eq!(
        ranks,
        vec![1, 2],
        "ranks belong to one cross-source query, not merged source-local rank1 lists"
    );
    assert!(mixed.hits.iter().any(|h| h.abs_path
        == workspace
            .current()
            .unwrap()
            .root
            .join("local.md")
            .to_string_lossy()));
    assert!(mixed.hits.iter().any(|h| h.source_id.as_deref()
        == Some(receipt.reader_source_id.as_str())
        && h.abs_path == receipt.path));
}
