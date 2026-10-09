//! Windows incoming Markdown is durable SQLite data before an NTFS projection.
//! File names and directory flushes never constitute a publication receipt.
use super::types::*;
use rusqlite::{params, Connection, Transaction};
const DDL: &str = include_str!("migrations/002-windows-input.sql");
const NAMES: [&str; 2] = [
    "device_incoming_windows_schema_v1",
    "device_incoming_windows_input_v1",
];
fn normal(sql: &str) -> String {
    sql.chars().filter(|c| !c.is_ascii_whitespace()).collect()
}
fn check(db: &Connection) -> PublicationResult<bool> {
    let shadow: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE name GLOB 'device_incoming_*' OR tbl_name GLOB 'device_incoming_*' OR name IN('device_documents_v1','device_import_intents_v1','sources'))", [], |r| r.get(0))?;
    if shadow {
        return Err(PublicationError::Conflict);
    }
    for schema in ["sqlite_schema", "sqlite_temp_schema"] {
        let sql = format!("SELECT EXISTS(SELECT 1 FROM {schema} WHERE type='trigger' AND (tbl_name GLOB 'device_incoming_*' OR tbl_name IN('device_documents_v1','device_import_intents_v1','sources')))");
        if db.query_row(&sql, [], |r| r.get::<_, bool>(0))? {
            return Err(PublicationError::Conflict);
        }
    }
    let mut query = db.prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name GLOB 'device_incoming_windows_*' OR tbl_name GLOB 'device_incoming_windows_*'")?;
    let rows = query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        return Ok(false);
    }
    let definitions: Vec<_> = DDL.split(';').take(2).collect();
    let mut found = [false; 2];
    for (kind, name, table, sql) in rows {
        if kind == "index" && sql.is_none() && NAMES.contains(&table.as_str()) {
            continue;
        }
        let index = NAMES
            .iter()
            .position(|n| *n == name)
            .ok_or(PublicationError::Conflict)?;
        if kind != "table"
            || table != name
            || found[index]
            || sql
                .as_ref()
                .is_none_or(|s| normal(s) != normal(definitions[index]))
        {
            return Err(PublicationError::Conflict);
        }
        found[index] = true;
    }
    if !found.iter().all(|v| *v) {
        return Err(PublicationError::Conflict);
    }
    let (count, version): (u64, Option<u64>) = db.query_row(
        "SELECT count(*),max(version) FROM device_incoming_windows_schema_v1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != 1 || version != Some(1) {
        return Err(PublicationError::Conflict);
    }
    let (count,bytes,invalid):(u64,u64,bool)=db.query_row("SELECT count(*),coalesce(sum(length(b.body)),0),coalesce(max(i.id IS NULL OR i.state='dropped' OR b.revision!=i.revision OR b.digest!=i.input_digest OR typeof(b.body)!='blob' OR length(b.body)!=i.bytes OR length(b.body)>1048576),0) FROM device_incoming_windows_input_v1 b LEFT JOIN device_incoming_intents_v1 i ON i.id=b.id",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if count > 1000 || bytes > 64 * 1024 * 1024 || invalid {
        return Err(PublicationError::Conflict);
    }
    Ok(true)
}
pub(super) fn initialize(tx: &Transaction<'_>) -> PublicationResult<()> {
    if !check(tx)? {
        tx.execute_batch(DDL)?;
    }
    check(tx)?;
    Ok(())
}
pub(super) fn reserve(
    tx: &Transaction<'_>,
    intent: &Intent,
    input: &IncomingDocument,
) -> PublicationResult<()> {
    tx.execute(
        "INSERT INTO device_incoming_windows_input_v1(id,revision,digest,body) VALUES(?1,?2,?3,?4)",
        params![intent.id, intent.revision, intent.digest, input.markdown],
    )?;
    fence(tx, intent)?;
    Ok(())
}
pub(super) fn fence(db: &Connection, intent: &Intent) -> PublicationResult<()> {
    if !check(db)? {
        return Err(PublicationError::Incomplete);
    }
    let exact:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM device_incoming_windows_input_v1 b JOIN device_incoming_intents_v1 i ON i.id=b.id WHERE b.id=?1 AND b.revision=?2 AND b.digest=?3 AND length(b.body)=?4 AND i.state=?5)",params![intent.id,intent.revision,intent.digest,intent.bytes,intent.state],|r|r.get(0))?;
    if !exact {
        return Err(PublicationError::Conflict);
    }
    Ok(())
}
pub(super) fn body(db: &Connection, intent: &Intent) -> PublicationResult<Vec<u8>> {
    fence(db, intent)?;
    let bytes: Vec<u8> = db.query_row(
        "SELECT body FROM device_incoming_windows_input_v1 WHERE id=?1 AND length(body)<=1048576",
        [&intent.id],
        |r| r.get(0),
    )?;
    if bytes.len() != intent.bytes as usize {
        return Err(PublicationError::Incomplete);
    }
    Ok(bytes)
}
pub(super) fn exact(
    db: &Connection,
    intent: &Intent,
    input: &IncomingDocument,
) -> PublicationResult<()> {
    fence(db, intent)?;
    let matches:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM device_incoming_windows_input_v1 WHERE id=?1 AND revision=?2 AND digest=?3 AND body=?4)",params![intent.id,intent.revision,intent.digest,input.markdown],|r|r.get(0))?;
    if !matches {
        return Err(PublicationError::Conflict);
    }
    Ok(())
}
pub(super) fn remove(tx: &Transaction<'_>, intent: &Intent) -> PublicationResult<()> {
    fence(tx, intent)?;
    if tx.execute(
        "DELETE FROM device_incoming_windows_input_v1 WHERE id=?1 AND revision=?2 AND digest=?3",
        params![intent.id, intent.revision, intent.digest],
    )? != 1
    {
        return Err(PublicationError::Conflict);
    }
    Ok(())
}
