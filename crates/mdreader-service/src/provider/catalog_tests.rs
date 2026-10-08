use super::*;
#[test]
fn final_fence_rejects_metadata_publication_after_rows_were_observed() {
    let mut c = Connection::open_in_memory().unwrap();
    let library = library_id(&mut c).unwrap();
    let scope = SelectedScope::exact(vec![RecordKey::new("selected").unwrap()]).unwrap();
    let descriptor = SourceDescriptor::new(
        SourceBinding::new(
            library,
            SourceId::new("selected-source").unwrap(),
            AccountBinding::new("private-account").unwrap(),
            Uuid::new_v4(),
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
    admit(
        &mut c,
        &descriptor,
        &OpaqueValue::new("private-reference").unwrap(),
        [1; 32],
    )
    .unwrap();
    let publish = |c: &mut Connection| {
        let snapshot = begin_snapshot(c, descriptor.binding(), None)?;
        stage_page(
            c,
            &snapshot,
            None,
            &MetadataPage::new(
                vec![],
                None,
                1,
                Coverage::MetadataCompleteSelected { observed_at_ms: 1 },
            )?,
        )?;
        publish_snapshot(c, &snapshot, &UnsupportedProjection)
    };
    publish(&mut c).unwrap();
    let result = catalog_page_checked(
        &mut c,
        descriptor.binding().source_id(),
        None,
        None,
        100,
        publish,
    );
    assert!(matches!(result, Err(ProviderError::Conflict)));
    assert!(
        catalog_page(&mut c, descriptor.binding().source_id(), None, None, 100)
            .unwrap()
            .documents
            .is_empty()
    );
}
