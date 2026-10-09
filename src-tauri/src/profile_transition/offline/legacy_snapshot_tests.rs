//! Actual closed fixture files, including uncheckpointed committed WAL bytes.
//! This tests snapshot fidelity; application closure remains the native observer.
use super::*;
use std::os::unix::fs::PermissionsExt;
use uuid::Uuid;

#[test]
fn closed_snapshot_recovers_wal_keeps_unknown_empty_originals_and_refuses_change() {
    let temporary = tempfile::tempdir().unwrap();
    let parent = temporary.path().canonicalize().unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    let source = parent.join("profile");
    std::fs::create_dir(&source).unwrap();
    let seed = parent.join("seed.sqlite");
    let db = Connection::open(&seed).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
        CREATE TABLE objects(object_id TEXT PRIMARY KEY,entity TEXT,value TEXT,provenance TEXT);
        CREATE TABLE morphisms(morphism_id TEXT PRIMARY KEY,arrow TEXT,source TEXT,target TEXT,provenance TEXT);
        PRAGMA user_version=10;").unwrap();
    db.execute("INSERT INTO objects VALUES(?1,'kn.Claim','{\"claim_text\":\"WAL-only recovered insight\"}','{}')", [Uuid::new_v4().to_string()]).unwrap();
    // Construct a quiescent offline snapshot with committed WAL still present.
    // The seed remains outside the selected profile and never writes it.
    for suffix in ["", "-wal", "-shm"] {
        std::fs::copy(
            parent.join(format!("seed.sqlite{suffix}")),
            source.join(format!("catdb.db{suffix}")),
        )
        .unwrap();
    }
    std::fs::create_dir(source.join("unknown")).unwrap();
    std::fs::write(source.join("unknown/empty"), b"").unwrap();
    std::fs::write(source.join("unknown/opaque"), [0, 255, 1]).unwrap();
    std::fs::write(
        source.join("session.json"),
        br#"{"session":{"version":1,"tabs":[],"recent":[]}}"#,
    )
    .unwrap();
    let originals = [
        "catdb.db",
        "catdb.db-wal",
        "catdb.db-shm",
        "unknown/empty",
        "unknown/opaque",
        "session.json",
    ]
    .map(|name| (name, std::fs::read(source.join(name)).unwrap()));
    let root = SourceRoot::capture(source.clone()).unwrap();
    let archive = capture(&root, || Ok(())).unwrap();
    verify_originals(&root, &archive).unwrap();
    let identity = legacy::migration::archive::identity(&archive, &Default::default()).unwrap();
    let documents =
        legacy::catalog::reader::documents(&archive, &identity, &Default::default()).unwrap();
    assert_eq!(documents.len(), 1);
    assert!(documents[0].markdown.contains("WAL-only recovered insight"));
    let manifest = legacy::verify(&archive, &Default::default()).unwrap();
    let raw = manifest
        .artifacts
        .iter()
        .find(|a| a.role == Role::Raw)
        .unwrap();
    let mut zip = zip::ZipArchive::new(File::open(archive.join(&raw.raw)).unwrap()).unwrap();
    for (name, bytes) in &originals {
        let mut copied = Vec::new();
        zip.by_name(&format!("original/{name}"))
            .unwrap()
            .read_to_end(&mut copied)
            .unwrap();
        assert_eq!(copied, *bytes);
        assert_eq!(std::fs::read(source.join(name)).unwrap(), *bytes);
    }
    assert!(zip.by_name("original-metadata.json").is_ok());
    // A source changing after capture cannot be accepted by the final fence.
    let mut checks = 0;
    let result = capture(&root, || {
        checks += 1;
        if checks == 2 {
            std::fs::write(source.join("unknown/opaque"), [2, 3]).unwrap();
        }
        Ok(())
    });
    assert!(matches!(result, Err(Error::Conflict)));
    assert!(matches!(
        verify_originals(&root, &archive),
        Err(Error::Conflict)
    ));
    drop(db);
}
