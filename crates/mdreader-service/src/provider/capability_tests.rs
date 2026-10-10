use super::*;

#[test]
fn verified_folder_delta_upgrade_retains_grant_and_discards_untracked_staging_atomically() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("registry.sqlite");
    let mut c = Connection::open(&path).unwrap();
    let scope = SelectedScope::DriveFolder {
        root: RecordKey::new("notes").unwrap(),
        recursive: true,
    };
    let binding = SourceBinding::new(
        library_id(&mut c).unwrap(),
        SourceId::new("drive-synthetic").unwrap(),
        AccountBinding::new("synthetic-account").unwrap(),
        Uuid::new_v4(),
        scope.digest(),
    )
    .unwrap();
    let old = SourceDescriptor::new(
        binding.clone(),
        ProviderKind::GoogleDrive,
        DisplayText::new("Public Drive").unwrap(),
        scope.clone(),
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
        &old,
        &OpaqueValue::new("synthetic-vault-reference").unwrap(),
        [7; 32],
    )
    .unwrap();
    let snap = begin_snapshot(&mut c, &binding, None).unwrap();
    stage_page(
        &mut c,
        &snap,
        None,
        &MetadataPage::new(
            vec![],
            Some(ProviderCursor::new(b"old-list-page".to_vec()).unwrap()),
            10,
            Coverage::Partial {
                reason: DisplayText::new("Loading").unwrap(),
            },
        )
        .unwrap(),
    )
    .unwrap();
    let mut enabled = old.capabilities();
    enabled.changes = true;
    let verified = SourceDescriptor::new(
        binding.clone(),
        old.kind(),
        old.name().clone(),
        scope,
        enabled,
        old.health(),
        old.coverage().clone(),
    )
    .unwrap();
    let before = configuration(&mut c, binding.source_id()).unwrap();
    capabilities::retain_verified_changes(&mut c, &verified).unwrap();
    assert!(resume(&mut c, &binding).unwrap().is_none());
    let after = configuration(&mut c, binding.source_id()).unwrap();
    assert_eq!(after.descriptor.binding(), before.descriptor.binding());
    assert_eq!(after.credential_reference, before.credential_reference);
    assert_eq!(after.configuration_digest, before.configuration_digest);
    assert!(after.descriptor.capabilities().changes);
    assert!(!after.descriptor.capabilities().guarded_write);
    drop(c);
    let mut c = Connection::open(&path).unwrap();
    capabilities::retain_verified_changes(&mut c, &verified).unwrap();
    assert!(
        configuration(&mut c, binding.source_id())
            .unwrap()
            .descriptor
            .capabilities()
            .changes
    );
    let start = ProviderCursor::new(b"fresh-exact-start".to_vec()).unwrap();
    begin_snapshot(&mut c, &binding, Some(&start)).unwrap();
    assert_eq!(
        resume(&mut c, &binding).unwrap().unwrap().change_cursor,
        Some(start)
    );
    disable(&mut c, &binding, &UnsupportedProjection).unwrap();
    assert!(capabilities::retain_verified_changes(&mut c, &verified).is_err());
}
