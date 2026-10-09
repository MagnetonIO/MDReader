use super::*;
use crate::{reviews::ArchiveIdentity, Input, Role};
use rusqlite::params;
use std::os::unix::fs::PermissionsExt;
#[test]
fn offline_graph_keeps_legacy_identity_edges_history_and_exact_chain_while_projecting_reader_insights(
) {
    let temporary = tempfile::tempdir().unwrap();
    std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = temporary.path().join("legacy.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE objects(object_id TEXT PRIMARY KEY,entity TEXT,value TEXT,provenance TEXT);
      CREATE TABLE morphisms(morphism_id TEXT PRIMARY KEY,arrow TEXT,source TEXT,target TEXT,provenance TEXT);
      CREATE TABLE catalog_versions(version_id TEXT PRIMARY KEY,parent TEXT,actor TEXT,timestamp INTEGER);
      CREATE TABLE operation_log(envelope TEXT NOT NULL);
      CREATE TABLE operation_log_anchor(singleton INTEGER PRIMARY KEY,op_count INTEGER,head_digest TEXT);
      CREATE TABLE unknown_authored_record(body BLOB);
      INSERT INTO unknown_authored_record VALUES(x'0061ff'); PRAGMA user_version=10;").unwrap();
    let a = uuid::Uuid::new_v4().to_string();
    let b = uuid::Uuid::new_v4().to_string();
    let edge = uuid::Uuid::new_v4().to_string();
    db.execute("INSERT INTO objects VALUES(?1,'kn.Claim',?2,?3)",params![a,r#"{"claim_text":"Use the reliable design","confidence":0.75,"unknownUserField":"kept"}"#,r#"{"source":"old/file.md","range":[2,4],"approved_by":"reviewer"}"#]).unwrap();
    db.execute(
        "INSERT INTO objects VALUES(?1,'kn.Concept',?2,'{}')",
        params![b, r#"{"concept_name":"Reliable systems"}"#],
    )
    .unwrap();
    db.execute(
        "INSERT INTO morphisms VALUES(?1,'kn.about',?2,?3,'{}')",
        params![edge, a, b],
    )
    .unwrap();
    db.execute(
        "INSERT INTO catalog_versions VALUES(?1,NULL,'author',123)",
        [&a],
    )
    .unwrap();
    let architecture = [
        (
            "architecture_overlay.Component",
            "component_name",
            "Reader engine",
        ),
        ("architecture_overlay.Layer", "layer_name", "Storage layer"),
        (
            "architecture_overlay.Interface",
            "interface_name",
            "Reader API",
        ),
        (
            "architecture_overlay.Dependency",
            "dependency_kind",
            "depends on",
        ),
    ]
    .map(|(entity, key, title)| (uuid::Uuid::new_v4().to_string(), entity, key, title));
    for (id, entity, key, title) in &architecture {
        db.execute(
            "INSERT INTO objects VALUES(?1,?2,?3,'{}')",
            params![
                id,
                entity,
                serde_json::to_string(&serde_json::json!({key:*title})).unwrap()
            ],
        )
        .unwrap();
    }
    let source_document = uuid::Uuid::new_v4().to_string();
    db.execute("INSERT INTO objects VALUES(?1,'md.Document',?2,'{}')",params![source_document,r#"{"title":"Original design note","file_path":"/synthetic/ungranted/design.md","repo":"synthetic"}"#]).unwrap();
    db.execute(
        "INSERT INTO morphisms VALUES(?1,'architecture_overlay.extracted_from',?2,?3,'{}')",
        params![
            uuid::Uuid::new_v4().to_string(),
            architecture[0].0,
            source_document
        ],
    )
    .unwrap();
    let envelope = format!(
        r#"{{"version":"catdb.ir/v1","operation_id":"{a}","workspace_id":"{b}","branch_id":"{edge}","actor":"author","timestamp":123,"operation":{{"type":"assert_object","entity":"kn.Claim","object_id":"{a}","value":{{"claim_text":"Use the reliable design"}},"provenance":{{"confidence":0.75}}}}}}"#
    );
    db.execute("INSERT INTO operation_log VALUES(?1)", [&envelope])
        .unwrap();
    let digest = chain::extend(&"0".repeat(64), &envelope).unwrap();
    db.execute("INSERT INTO operation_log_anchor VALUES(0,1,?1)", [&digest])
        .unwrap();
    drop(db);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let original = std::fs::read(&path).unwrap();
    let archive = temporary.path().join("archive");
    let limits = Limits::default();
    let manifest = crate::export(
        &[Input {
            role: Role::Catalog,
            path: path.clone(),
        }],
        &archive,
        &limits,
    )
    .unwrap();
    let manifest_bytes = std::fs::read(archive.join("manifest.json")).unwrap();
    use sha2::{Digest, Sha256};
    let identity = ArchiveIdentity {
        export_id: manifest.export_id,
        manifest_sha256: crate::hex(&Sha256::digest(&manifest_bytes)),
    };
    let data = std::fs::read(archive.join("0000.records.jsonl")).unwrap();
    let records = data
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
        .map(|line| decode_record(line, &limits).unwrap().unwrap())
        .collect::<Vec<_>>();
    assert!(records.iter().any(|r|matches!(r,ArchivedRecord::Chain{status:ChainStatus::MatchesStoredAnchor,computed_digest:Some(h),..} if h==&digest)));
    assert!(records
        .iter()
        .any(|r| matches!(r,ArchivedRecord::History{table,..} if table=="catalog_versions")));
    assert!(records
        .iter()
        .any(|r| matches!(r,ArchivedRecord::Operation{envelope:raw,..} if raw==&envelope)));
    let docs = reader::documents(&archive, &identity, &limits).unwrap();
    assert_eq!(docs.len(), 7);
    for (id, _, _, title) in &architecture {
        let projected = docs.iter().find(|d| &d.legacy_id == id).unwrap();
        assert_eq!(&projected.title, title);
        assert!(projected.markdown.contains(title));
    }
    let component = docs
        .iter()
        .find(|d| d.legacy_id == architecture[0].0)
        .unwrap();
    assert!(component
        .markdown
        .contains(&format!("legacy/0/{source_document}.md")));
    let source = docs
        .iter()
        .find(|d| d.legacy_id == source_document)
        .unwrap();
    assert!(source.markdown.contains("not the original Markdown body"));
    let references = crate::history::entries(&archive, &identity, &limits).unwrap();
    assert_eq!(references.len(), 1);
    assert_eq!(references[0].context, "source_document");
    assert_eq!(references[0].title, "Original design note");
    assert!(!references[0].archived_position);

    let claim = docs.iter().find(|d| d.legacy_id == a).unwrap();
    assert!(claim.markdown.contains("Use the reliable design"));
    assert!(claim.markdown.contains("unknownUserField"));
    assert!(claim.markdown.contains(&format!("legacy/0/{b}.md")));
    assert_eq!(claim.relationships[0].id, edge);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read(archive.join("0000.raw")).unwrap(), original);
    assert_eq!(
        reader::documents(&archive, &identity, &limits).unwrap()[0].id,
        docs[0].id
    );
    assert!(records.iter().any(|r|matches!(r,ArchivedRecord::Scope{raw_only_tables,..} if raw_only_tables.contains(&"unknown_authored_record".to_owned()))));
}
