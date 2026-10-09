use super::*;
use mdreader_capture::SelectedDirectory;
use std::io::Write;

#[test]
fn sqlite_uri_raw_records_and_reviews_survive_windows_native_names() {
    let temporary = tempfile::tempdir().unwrap();
    let selected = SelectedDirectory::open(temporary.path()).unwrap();
    // A literal '?' is illegal in a Windows leaf. Literal "%3F" must stay a
    // filename, not become a URI query delimiter after decoding twice.
    let directory = Path::new("archive space 雪 # 100% %3F");
    let private = selected.create_private_child(directory).unwrap();
    let root = temporary.path().join(directory);
    let name = Path::new("proposal résumé # 100% %3F.sqlite");
    let file = private.create_database_file(name).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let original = root.join(name);
    let connection = rusqlite::Connection::open(&original).unwrap();
    connection
        .execute_batch(
            "PRAGMA journal_mode=DELETE;
         CREATE TABLE proposal(fact_id TEXT PRIMARY KEY,kind TEXT,schema_name TEXT,
           extractor TEXT,rule TEXT,qualified TEXT,source_path TEXT,start_line INTEGER,
           end_line INTEGER,content_hash TEXT,endpoint_src TEXT,endpoint_dst TEXT,
           state TEXT,reviewer TEXT,reviewed_hash TEXT,payload TEXT,future_integer INTEGER,
           future_blob BLOB,future_real REAL);
         INSERT INTO proposal VALUES('stable','object','decision','extract@1','D1',
           'Decision','missing.md',2,3,'changed',NULL,NULL,'rejected','Reviewer',
           'reviewed','{\"unknown\":true}',9223372036854775807,x'00ff',1.25);
         CREATE TABLE future_history(i INTEGER,t TEXT,b BLOB,r REAL);
         INSERT INTO future_history VALUES(9223372036854775807,'authored future history',
           x'00ff',1.25);
         PRAGMA user_version=2;",
        )
        .unwrap();
    drop(connection);
    let before = std::fs::read(&original).unwrap();
    // Exercise the actual Windows drive URI and extended canonical path before
    // export renames this member to its portable archive name.
    let immutable = records::open_snapshot(&original).unwrap();
    assert_eq!(
        immutable
            .query_row::<i64, _, _>("SELECT future_integer FROM proposal", [], |row| row.get(0))
            .unwrap(),
        i64::MAX
    );
    assert!(immutable.execute("DELETE FROM proposal", []).is_err());
    drop(immutable);
    let limits = Limits::default();
    let archive = root.join("archive");
    let manifest = export(
        &[Input {
            role: Role::Proposals,
            path: original.clone(),
        }],
        &archive,
        &limits,
    )
    .unwrap();
    assert!(matches!(
        manifest.artifacts[0].conversion,
        Conversion::Known { rows: 1 }
    ));
    assert_eq!(manifest.artifacts[0].locator.restore().unwrap(), original);
    assert_eq!(std::fs::read(&original).unwrap(), before);
    assert_eq!(std::fs::read(archive.join("0000.raw")).unwrap(), before);
    verify(&archive, &limits).unwrap();
    let archived = records::open_snapshot(&archive.join("0000.raw")).unwrap();
    let unknown = archived
        .query_row::<(i64, Vec<u8>, f64), _, _>("SELECT i,b,r FROM future_history", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap();
    assert_eq!(unknown, (i64::MAX, vec![0, 255], 1.25));
    drop(archived);
    let identity = migration::archive::identity(&archive, &limits).unwrap();
    let destination = root.join("reviews");
    let receipt = reviews::stage_reviews(&archive, &identity, &destination, &limits).unwrap();
    assert_eq!(receipt.records, 1);
    let staged = reviews::ReviewArchive::open(&destination, &identity, &limits).unwrap();
    let page = staged.list(0, 10).unwrap();
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0].state, reviews::ArchivedState::Rejected);
    let raw = staged
        .raw_record(&page.records[0].record_id)
        .unwrap()
        .unwrap();
    let record: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(
        record["fields"]["future_integer"]["value"],
        i64::MAX.to_string()
    );
    assert_eq!(record["fields"]["future_blob"]["value"], "00ff");
    assert_eq!(
        record["fields"]["future_real"]["value"],
        format!("{:016x}", 1.25_f64.to_bits())
    );
    assert_eq!(std::fs::read(&original).unwrap(), before);
    private.verify().unwrap();
    selected.verify().unwrap();
}

#[test]
fn private_ntfs_archive_export_copy_reviews_and_profile_preserve_original_bytes() {
    let temporary = tempfile::tempdir().unwrap();
    let selected = SelectedDirectory::open(temporary.path()).unwrap();
    let private = selected.create_private_child(Path::new("private")).unwrap();
    let root = temporary.path().join("private");
    let input = b"{\"future\":9223372036854775807,\"settings\":{\"theme\":\"dark\"}}";
    let mut file = private
        .create_database_file(Path::new("settings.json"))
        .unwrap();
    file.write_all(input).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let inputs = [Input {
        role: Role::Settings,
        path: root.join("settings.json"),
    }];
    let limits = Limits::default();
    let archive = root.join("archive");
    export(&inputs, &archive, &limits).unwrap();
    let identity = migration::archive::identity(&archive, &limits).unwrap();
    assert_eq!(std::fs::read(archive.join("0000.raw")).unwrap(), input);
    assert_eq!(std::fs::read(&inputs[0].path).unwrap(), input);
    assert!(matches!(
        export(&inputs, &archive, &limits),
        Err(Error::Conflict)
    ));
    migration::archive::copy(&archive, &identity, &root.join("copy"), &limits).unwrap();
    assert_eq!(
        migration::archive::identity(&root.join("copy"), &limits).unwrap(),
        identity
    );
    let spec = SnapshotSpec::from_inputs(&inputs).unwrap();
    let mut file = private
        .create_database_file(Path::new("spec.json"))
        .unwrap();
    file.write_all(&serde_json::to_vec(&spec).unwrap()).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let prepared =
        migration::prepare(&root.join("spec.json"), &root.join("profile"), &limits).unwrap();
    assert_eq!(prepared.version, 1);
    assert_eq!(
        std::fs::read(root.join("profile/settings.json")).unwrap(),
        input
    );
    migration::verify(&root.join("profile"), &limits).unwrap();
    let reviews = reviews::ReviewArchive::open(
        &root.join("profile/archived-reviews"),
        &prepared.archive,
        &limits,
    )
    .unwrap();
    assert!(reviews.list(0, 10).unwrap().records.is_empty());
    drop(reviews);
    let materialized = migration::archive::prepare(
        &archive,
        &identity,
        &root.join("materialized"),
        &limits,
        |_, _, destination| {
            assert!(destination.join("legacy-archive/0000.raw").is_file());
            Ok(0)
        },
    )
    .unwrap();
    assert_eq!(materialized.materialized_records, Some(0));
    assert!(!root.join("materialized/settings.json").exists());
    migration::verify(&root.join("materialized"), &limits).unwrap();
    private.verify().unwrap();
    selected.verify().unwrap();
}
