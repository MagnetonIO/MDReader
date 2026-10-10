use super::*;
fn source(c: &mut Connection) -> SourceDescriptor {
    let scope = SelectedScope::ScopedRoot {
        root: RecordKey::new("/plans").unwrap(),
        recursive: true,
    };
    let binding = SourceBinding::new(
        registry::library_id(c).unwrap(),
        SourceId::new("source").unwrap(),
        AccountBinding::new("account").unwrap(),
        Uuid::new_v4(),
        scope.digest(),
    )
    .unwrap();
    let descriptor = SourceDescriptor::new(
        binding,
        ProviderKind::Dropbox,
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
    registry::admit(
        c,
        &descriptor,
        &OpaqueValue::new("synthetic-reference").unwrap(),
        [7; 32],
    )
    .unwrap();
    descriptor
}
#[test]
fn exact_page_survives_owner_reopen_and_slot_replacement_never_returns_other_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("library.sqlite");
    let mut c = Connection::open(&path).unwrap();
    let descriptor = source(&mut c);
    let slot = Uuid::new_v4();
    let bytes = b"public frozen page";
    let digest = body_hash(bytes);
    store(
        &mut c,
        descriptor.binding(),
        CursorKind::List,
        slot,
        digest,
        bytes,
    )
    .unwrap();
    drop(c);
    let mut c = Connection::open(path).unwrap();
    assert_eq!(
        load(&mut c, descriptor.binding(), CursorKind::List, slot, digest).unwrap(),
        bytes
    );
    assert_eq!(
        store(
            &mut c,
            descriptor.binding(),
            CursorKind::List,
            slot,
            body_hash(b"changed bytes"),
            b"changed bytes"
        )
        .unwrap_err(),
        ProviderError::Conflict
    );
    assert_eq!(
        load(&mut c, descriptor.binding(), CursorKind::List, slot, digest).unwrap(),
        bytes
    );
    let newer = Uuid::new_v4();
    let next = b"next remote page";
    store(
        &mut c,
        descriptor.binding(),
        CursorKind::List,
        newer,
        body_hash(next),
        next,
    )
    .unwrap();
    assert_eq!(
        load(&mut c, descriptor.binding(), CursorKind::List, slot, digest).unwrap_err(),
        ProviderError::ResetCursor
    );
    assert_eq!(
        load(
            &mut c,
            descriptor.binding(),
            CursorKind::List,
            newer,
            body_hash(next)
        )
        .unwrap(),
        next
    );
}
#[test]
fn kind_slot_digest_and_saved_binding_are_independent_exact_fences() {
    let mut c = Connection::open_in_memory().unwrap();
    let descriptor = source(&mut c);
    let slot = Uuid::new_v4();
    let bytes = b"public page";
    let digest = body_hash(bytes);
    store(
        &mut c,
        descriptor.binding(),
        CursorKind::List,
        slot,
        digest,
        bytes,
    )
    .unwrap();
    assert_eq!(
        load(
            &mut c,
            descriptor.binding(),
            CursorKind::Changes,
            slot,
            digest
        )
        .unwrap_err(),
        ProviderError::ResetCursor
    );
    assert_eq!(
        load(
            &mut c,
            descriptor.binding(),
            CursorKind::List,
            Uuid::new_v4(),
            digest
        )
        .unwrap_err(),
        ProviderError::ResetCursor
    );
    assert_eq!(
        load(
            &mut c,
            descriptor.binding(),
            CursorKind::List,
            slot,
            [0; 32]
        )
        .unwrap_err(),
        ProviderError::ResetCursor
    );
    let changed = SourceBinding::new(
        descriptor.binding().library_id().clone(),
        descriptor.binding().source_id().clone(),
        descriptor.binding().account().clone(),
        Uuid::new_v4(),
        descriptor.scope().digest(),
    )
    .unwrap();
    assert_eq!(
        load(&mut c, &changed, CursorKind::List, slot, digest).unwrap_err(),
        ProviderError::Unauthorized
    );
    registry::db(c.execute(
        "UPDATE mdreader_provider_registry_sources SET enabled=0",
        [],
    ))
    .unwrap();
    assert_eq!(
        load(&mut c, descriptor.binding(), CursorKind::List, slot, digest).unwrap_err(),
        ProviderError::Unauthorized
    );
}
#[test]
fn oversized_corrupt_and_foreign_schema_pages_fail_without_mutating_the_valid_slot() {
    let mut c = Connection::open_in_memory().unwrap();
    let descriptor = source(&mut c);
    let slot = Uuid::new_v4();
    let bytes = b"public page";
    let digest = body_hash(bytes);
    store(
        &mut c,
        descriptor.binding(),
        CursorKind::List,
        slot,
        digest,
        bytes,
    )
    .unwrap();
    assert_eq!(
        store(
            &mut c,
            descriptor.binding(),
            CursorKind::List,
            Uuid::new_v4(),
            [0; 32],
            &vec![0; MAX_PAGE_SPOOL_BYTES + 1]
        )
        .unwrap_err(),
        ProviderError::TooLarge
    );
    assert_eq!(
        load(&mut c, descriptor.binding(), CursorKind::List, slot, digest).unwrap(),
        bytes
    );
    c.execute(
        "UPDATE main.mdreader_provider_page_spool_v1 SET bytes=x'01'",
        [],
    )
    .unwrap();
    assert_eq!(
        load(&mut c, descriptor.binding(), CursorKind::List, slot, digest).unwrap_err(),
        ProviderError::Persistence
    );
    c.execute_batch("CREATE TRIGGER foreign_page_write AFTER INSERT ON mdreader_provider_page_spool_v1 BEGIN SELECT 1; END;").unwrap();
    assert_eq!(
        store(
            &mut c,
            descriptor.binding(),
            CursorKind::List,
            slot,
            digest,
            bytes
        )
        .unwrap_err(),
        ProviderError::Persistence
    );
}
#[test]
fn temporary_table_and_trigger_cannot_shadow_or_mutate_the_owned_page() {
    let mut c = Connection::open_in_memory().unwrap();
    let descriptor = source(&mut c);
    let slot = Uuid::new_v4();
    let bytes = b"main public page";
    let digest = body_hash(bytes);
    store(
        &mut c,
        descriptor.binding(),
        CursorKind::List,
        slot,
        digest,
        bytes,
    )
    .unwrap();
    c.execute_batch("CREATE TEMP TABLE mdreader_provider_page_spool_v1(bytes BLOB); INSERT INTO temp.mdreader_provider_page_spool_v1 VALUES(x'70616765'); CREATE TEMP TRIGGER temp_shadow_write AFTER INSERT ON temp.mdreader_provider_page_spool_v1 BEGIN UPDATE mdreader_provider_page_spool_v1 SET bytes=x'00'; END;").unwrap();
    let before_main: Vec<u8> = c
        .query_row(
            "SELECT bytes FROM main.mdreader_provider_page_spool_v1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let before_temp: Vec<u8> = c
        .query_row(
            "SELECT bytes FROM temp.mdreader_provider_page_spool_v1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        store(
            &mut c,
            descriptor.binding(),
            CursorKind::List,
            Uuid::new_v4(),
            body_hash(b"new public page"),
            b"new public page"
        )
        .unwrap_err(),
        ProviderError::Persistence
    );
    assert_eq!(
        load(&mut c, descriptor.binding(), CursorKind::List, slot, digest).unwrap_err(),
        ProviderError::Persistence
    );
    let after_main: Vec<u8> = c
        .query_row(
            "SELECT bytes FROM main.mdreader_provider_page_spool_v1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let after_temp: Vec<u8> = c
        .query_row(
            "SELECT bytes FROM temp.mdreader_provider_page_spool_v1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(after_main, before_main);
    assert_eq!(after_temp, before_temp);
    c.execute_batch("DROP TABLE temp.mdreader_provider_page_spool_v1; CREATE TEMP TRIGGER temp_main_write AFTER INSERT ON main.mdreader_provider_page_spool_v1 BEGIN UPDATE mdreader_provider_page_spool_v1 SET bytes=x'00'; END;").unwrap();
    assert_eq!(
        store(
            &mut c,
            descriptor.binding(),
            CursorKind::List,
            Uuid::new_v4(),
            body_hash(b"new public page"),
            b"new public page"
        )
        .unwrap_err(),
        ProviderError::Persistence
    );
    let unchanged: Vec<u8> = c
        .query_row(
            "SELECT bytes FROM main.mdreader_provider_page_spool_v1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unchanged, before_main);
}
