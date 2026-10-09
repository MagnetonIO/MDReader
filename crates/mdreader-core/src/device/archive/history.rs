//! Explicit user matching of archived history to existing exact reader copies.
//! A binding is a retained reference, never source authority or a saved position.
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reference {
    pub document_id: Option<String>,
    pub source_id: String,
    pub path: String,
    pub revision: String,
}
impl std::fmt::Debug for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ImportedReference { [redacted] }")
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    pub entry_id: String,
    pub reference: Reference,
}
const SQL:&str="CREATE TABLE reader_import_history_schema_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1));
CREATE TABLE reader_import_history_bindings_v1(archive_id TEXT NOT NULL,archive_sha256 TEXT NOT NULL,entry_id TEXT NOT NULL,reference_json BLOB NOT NULL,PRIMARY KEY(archive_id,archive_sha256,entry_id));";
fn schema(db: &rusqlite::Connection, create: bool) -> Result<bool> {
    let count: u32 = db.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE name GLOB 'reader_import_history_*'",
        [],
        |r| r.get(0),
    )?;
    if count == 0 {
        if !create {
            return Ok(false);
        }
        db.execute_batch(SQL)?;
        db.execute(
            "INSERT INTO reader_import_history_schema_v1 VALUES(1,1)",
            [],
        )?;
    } else if count != 2 {
        return Err(Error::UnsupportedSchema);
    }
    for sql in SQL.split(';').filter(|s| !s.trim().is_empty()) {
        let name = sql
            .split_whitespace()
            .nth(2)
            .and_then(|s| s.split('(').next())
            .ok_or(Error::InvalidSchema)?;
        let actual: Option<String> = db
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
                [name],
                |r| r.get(0),
            )
            .optional()?;
        if actual.as_deref().map(str::trim) != Some(sql.trim()) {
            return Err(Error::UnsupportedSchema);
        }
    }
    let version: i64 = db.query_row(
        "SELECT version FROM reader_import_history_schema_v1 WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if version != 1 {
        return Err(Error::UnsupportedSchema);
    }
    Ok(true)
}
fn scope(archive: &str, sha: &str) -> Result<()> {
    if uuid::Uuid::parse_str(archive).is_err() || !hash(sha) {
        return Err(Error::InvalidSelection);
    }
    Ok(())
}
fn read(db: &rusqlite::Connection, archive: &str, sha: &str) -> Result<Vec<Binding>> {
    if !schema(db, false)? {
        return Ok(vec![]);
    }
    let mut s=db.prepare("SELECT entry_id,CASE WHEN length(reference_json)<=32768 THEN reference_json END,length(reference_json) FROM reader_import_history_bindings_v1 WHERE archive_id=?1 AND archive_sha256=?2 ORDER BY entry_id LIMIT 1001")?;
    let mut rows = s.query(params![archive, sha])?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        if result.len() >= 1000 || row.get::<_, u64>(2)? > 32768 {
            return Err(Error::InvalidSchema);
        }
        let raw: Vec<u8> = row.get(1)?;
        result.push(Binding {
            entry_id: row.get(0)?,
            reference: serde_json::from_slice(&raw)?,
        });
    }
    Ok(result)
}
pub fn digest(bindings: &[Binding]) -> Result<String> {
    Ok(blake3::hash(&serde_json::to_vec(bindings)?)
        .to_hex()
        .to_string())
}
pub fn bindings(store: &ReaderStore, archive: &str, sha: &str) -> Result<Vec<Binding>> {
    scope(archive, sha)?;
    read(store.device_connection(), archive, sha)
}
/// Native adapter must verify exact existing Library/provider membership before
/// and after this short CAS. Every later open still performs a fresh exact read.
/// This row alone can never authorize a source, provider fetch, or filesystem.
pub fn bind(
    store: &mut ReaderStore,
    archive: &str,
    sha: &str,
    entry: &str,
    expected: &str,
    reference: &Reference,
) -> Result<()> {
    scope(archive, sha)?;
    if uuid::Uuid::parse_str(entry).is_err()
        || !hash(expected)
        || !hash(&reference.revision)
        || reference.source_id.is_empty()
        || reference.source_id.len() > 256
        || reference.path.is_empty()
        || reference.path.len() > 16384
        || reference
            .document_id
            .as_ref()
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_err())
    {
        return Err(Error::InvalidSelection);
    }
    let raw = serde_json::to_vec(reference)?;
    if raw.len() > 32768 {
        return Err(Error::InvalidSelection);
    }
    store.with_library_connection(|db|->Result<()> {
        let tx=db.transaction()?;
        if digest(&read(&tx,archive,sha)?)?!=expected{return Err(Error::Conflict);}
        schema(&tx,true)?;
        tx.execute("INSERT INTO reader_import_history_bindings_v1 VALUES(?1,?2,?3,?4) ON CONFLICT(archive_id,archive_sha256,entry_id) DO UPDATE SET reference_json=excluded.reference_json",params![archive,sha,entry,raw])?;
        tx.commit()?;Ok(())
    })
}
