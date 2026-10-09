use super::*;
use std::{io::Read, process::Command};

#[test]
fn private_native_owner_reopens_hot_journal_keeps_bytes_and_readonly_attachment() {
    const CHILD: &str = "MDREADER_TEST_PRIVATE_READER_CRASH";
    if let Some(path) = std::env::var_os(CHILD) {
        let mut owner = ReaderStore::open_private(Path::new(&path)).unwrap();
        owner.with_library_connection(|db| {
            db.execute_batch("PRAGMA cache_size=32; PRAGMA cache_spill=ON; BEGIN IMMEDIATE; UPDATE private_reader_regression SET body=zeroblob(2097152);").unwrap();
        });
        // Real spilled uncommitted pages and hot rollback header, no Drop.
        std::process::exit(72);
    }
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("owned é reader % #");
    let mut owner = ReaderStore::open_private(&directory).unwrap();
    let body = "\u{feff}# Windows\r\n\r\nOriginal searchable naïve.\r\n";
    let document = crate::device::import(
        &mut owner,
        &directory.join("documents"),
        "original.md".into(),
        body.into(),
    )
    .unwrap();
    crate::device::pin(&mut owner, &document.id, true).unwrap();
    crate::device::position(&mut owner, &document.id, &document.revision, 3).unwrap();
    owner.with_library_connection(|db| db.execute_batch("CREATE TABLE private_reader_regression(body BLOB NOT NULL); INSERT INTO private_reader_regression VALUES(CAST(replace(hex(zeroblob(1048576)),'0','1') AS BLOB));")).unwrap();
    let certificate = owner.store_identity().unwrap();
    certificate.verify_at(&directory).unwrap();
    let reader = ReaderStore::open_readonly(&directory).unwrap();
    assert_eq!(
        reader
            .read_document(&document.source_id, "document.md", Some(&document.revision))
            .unwrap()
            .markdown,
        body
    );
    drop(reader);
    drop(owner);
    assert!(certificate.verify_at(&directory).is_err());
    drop(certificate);
    let root = SourceRoot::capture(directory.clone()).unwrap();
    for main in ["library.sqlite", "index.sqlite"] {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let file = root
                .open_optional_file_relative(Path::new(&format!("{main}{suffix}")))
                .unwrap()
                .unwrap();
            mdreader_capture::observe_object(&file, false, true).unwrap();
        }
    }
    let status = Command::new(std::env::current_exe().unwrap())
        .arg("--exact").arg("store::windows_private::tests::private_native_owner_reopens_hot_journal_keeps_bytes_and_readonly_attachment")
        .arg("--nocapture").env(CHILD, &directory).status().unwrap();
    assert_eq!(status.code(), Some(72));
    let mut journal = root
        .open_optional_file_relative(Path::new("library.sqlite-journal"))
        .unwrap()
        .unwrap();
    let mut header = [0; 8];
    journal.read_exact(&mut header).unwrap();
    assert_ne!(header, [0; 8]);
    drop(journal);
    let mut reopened = ReaderStore::open_private(&directory).unwrap();
    let exact = crate::device::read(&mut reopened, &document.id, Some(&document.revision)).unwrap();
    assert_eq!(exact.markdown, body);
    assert!(exact.document.pinned);
    assert_eq!(exact.document.line, 3);
    let unchanged: bool = reopened.with_library_connection(|db| db.query_row("SELECT length(body)=2097152 AND body=CAST(replace(hex(zeroblob(1048576)),'0','1') AS BLOB) FROM private_reader_regression", [], |r| r.get(0))).unwrap();
    assert!(unchanged);
    let sibling = ReaderStore::open_readonly(&directory).unwrap();
    assert_eq!(
        sibling
            .search("searchable", std::slice::from_ref(&document.source_id), 5)
            .unwrap()
            .len(),
        1
    );
    drop(sibling);
    reopened
        .with_library_connection(|db| db.pragma_update(None, "user_version", 999))
        .unwrap();
    drop(reopened);
    let before: Vec<_> = [
        "library.sqlite",
        "library.sqlite-journal",
        "index.sqlite",
        "index.sqlite-journal",
    ]
    .iter()
    .map(|name| {
        let mut file = root
            .open_optional_file_relative(Path::new(name))
            .unwrap()
            .unwrap();
        let stamp = mdreader_capture::observe_object(&file, false, true).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        ((*name).to_string(), stamp, blake3::hash(&bytes))
    })
    .collect();
    assert!(matches!(
        ReaderStore::open_private(&directory),
        Err(ReaderError::NewerDatabase(999))
    ));
    for (name, stamp, digest) in before {
        let mut file = root
            .open_optional_file_relative(Path::new(&name))
            .unwrap()
            .unwrap();
        assert_eq!(
            mdreader_capture::observe_object(&file, false, true).unwrap(),
            stamp
        );
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(blake3::hash(&bytes), digest);
    }
}
