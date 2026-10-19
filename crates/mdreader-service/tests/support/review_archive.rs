use mdreader_legacy_export::{
    export,
    reviews::{stage_reviews, ArchiveIdentity},
    Input, Limits, Role,
};
use rusqlite::params;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub fn archive(root: &Path, hash: &str) -> (PathBuf, ArchiveIdentity) {
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    let database = root.join("proposals.db");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection.execute_batch("CREATE TABLE proposal(fact_id TEXT PRIMARY KEY,kind TEXT,schema_name TEXT,extractor TEXT,rule TEXT,qualified TEXT,source_path TEXT,start_line INTEGER,end_line INTEGER,content_hash TEXT,endpoint_src TEXT,endpoint_dst TEXT,state TEXT,reviewer TEXT,reviewed_hash TEXT,payload TEXT); PRAGMA user_version=2;").unwrap();
    for (index, state) in ["approved", "rejected", "FUTURE"].into_iter().enumerate() {
        let id = Uuid::new_v4().to_string();
        let locator = "../NEVER_OPEN_deleted_original.md";
        let recorded = format!("blake3:{hash}");
        let reviewed = if index == 1 {
            format!("blake3:{}", "0".repeat(64))
        } else {
            recorded.clone()
        };
        let payload = serde_json::json!({"id":id,"entity":"decision_overlay.Decision","value":{"decision_title":"Keep archival ruling"},"provenance":{"source":locator,"range":[1,1],"content_hash":recorded,"schema_version":"1","extractor":"mdreader.decision@1","confidence":0.95,"approved_by":null}}).to_string();
        connection.execute("INSERT INTO proposal VALUES(?1,'object','decision_overlay','mdreader.decision@1','D1','decision_overlay.Decision',?2,1,1,?3,NULL,NULL,?4,'Reviewer',?5,?6)", params![id,locator,recorded,state,reviewed,payload]).unwrap();
    }
    drop(connection);
    let exported = root.join("export");
    let manifest = export(
        &[Input {
            role: Role::Proposals,
            path: database,
        }],
        &exported,
        &Limits::default(),
    )
    .unwrap();
    let digest = mdreader_provider::body_hash(&fs::read(exported.join("manifest.json")).unwrap());
    let identity = ArchiveIdentity {
        export_id: manifest.export_id,
        manifest_sha256: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
    };
    let stage = root.join("reviews");
    stage_reviews(&exported, &identity, &stage, &Limits::default()).unwrap();
    (stage, identity)
}
