use super::*;

#[test]
fn readonly_pages_keep_first_epoch_and_reads_require_exact_live_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("profile");
    let roots = temp.path().join("documents");
    let mut writer = ReaderStore::open(&profile).unwrap();
    let a = crate::device::import(&mut writer, &roots, "a.md".into(), "# A".into()).unwrap();
    crate::device::import(&mut writer, &roots, "b.md".into(), "# B".into()).unwrap();
    crate::device::import(&mut writer, &roots, "c.md".into(), "# C".into()).unwrap();
    let query = ReaderStore::open_readonly(&profile).unwrap();
    let first = page(&query, None, None, 2).unwrap();
    assert_eq!(first.documents.len(), 2);
    let second = page(
        &query,
        first.next_after.as_deref(),
        first.snapshot.as_ref(),
        2,
    )
    .unwrap();
    assert_eq!(second.documents.len(), 1);
    assert!(second.next_after.is_none());
    assert!(page(&query, None, None, 101).is_err());
    assert_eq!(read(&query, &receipt(&a)).unwrap().markdown, "# A");
    let mut wrong = receipt(&a);
    wrong.source_id = first
        .documents
        .iter()
        .find(|d| d.id != a.id)
        .unwrap()
        .source_id
        .clone();
    assert!(matches!(read(&query, &wrong), Err(Error::Conflict)));
    writer.set_source_enabled(&a.source_id, false).unwrap();
    writer.set_source_enabled(&a.source_id, true).unwrap();
    assert!(matches!(
        page(
            &query,
            first.next_after.as_deref(),
            first.snapshot.as_ref(),
            2
        ),
        Err(Error::Conflict)
    ));
    assert!(
        matches!(
            page_checked(&query, None, None, 2, || {
                writer
                    .with_library_connection(|db| {
                        db.execute(
                            "UPDATE device_documents_v1 SET pinned=1 WHERE id=?1",
                            [&a.id],
                        )
                    })
                    .unwrap();
            }),
            Err(Error::Conflict)
        ),
        "a final fresh observation cannot adopt a newer epoch"
    );
    writer.set_source_enabled(&a.source_id, false).unwrap();
    assert!(read(&query, &receipt(&a)).is_err());
    // Reads and listing used an actual read-only connection; no implicit schema writes.
    assert_eq!(query.device_connection().total_changes(), 0);
}

#[test]
fn absent_original_future_and_malformed_namespaces_remain_distinct_without_adoption() {
    let temp = tempfile::tempdir().unwrap();
    let mut writer = ReaderStore::open(temp.path()).unwrap();
    let query = ReaderStore::open_readonly(temp.path()).unwrap();
    assert!(page(&query, None, None, 100).unwrap().snapshot.is_none());
    writer
        .with_library_connection(|db| {
            db.execute_batch(include_str!("../migrations/001-device-documents.sql"))
        })
        .unwrap();
    assert!(page(&query, None, None, 100).unwrap().snapshot.is_some());
    let markers: u32 = query
        .device_connection()
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='device_document_schema_v1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        markers, 0,
        "reading the known original layout must not adopt it"
    );
    writer
        .with_library_connection(|db| {
            db.execute_batch(include_str!(
                "../migrations/002-adopt-document-namespace.sql"
            ))
        })
        .unwrap();
    writer
        .with_library_connection(|db| {
            db.execute("UPDATE device_document_schema_v1 SET version=99", [])
        })
        .unwrap();
    assert!(matches!(
        page(&query, None, None, 1),
        Err(Error::UnsupportedSchema)
    ));
    writer.with_library_connection(|db| db.execute_batch("DROP TABLE device_document_schema_v1; CREATE VIEW device_document_schema_v1 AS SELECT 1 singleton,2 version;")).unwrap();
    assert!(matches!(
        page(&query, None, None, 1),
        Err(Error::InvalidSchema)
    ));
    assert_eq!(query.device_connection().total_changes(), 0);
}
