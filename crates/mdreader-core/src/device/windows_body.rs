//! Canonical Windows import bytes precede their rebuildable NTFS projection.
//! This optional namespace neither grants a source nor advances library version.
use super::*;
use rusqlite::{Connection, Transaction};

const DDL: &str = include_str!("migrations/004-windows-import-bodies.sql");
const NAMES: [&str; 2] = ["device_windows_body_schema_v1", "device_windows_bodies_v1"];

pub(super) fn check(db: &Connection) -> Result<bool> {
    let mut statement = db.prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name GLOB 'device_windows_*' OR tbl_name GLOB 'device_windows_*' ORDER BY name")?;
    let rows = statement
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
            .position(|expected| *expected == name)
            .ok_or(Error::InvalidSchema)?;
        if kind != "table"
            || table != name
            || found[index]
            || sql
                .as_ref()
                .is_none_or(|sql| schema::normal(sql) != schema::normal(definitions[index]))
        {
            return Err(Error::InvalidSchema);
        }
        found[index] = true;
    }
    if !found.iter().all(|present| *present) {
        return Err(Error::InvalidSchema);
    }
    let (count, version): (u64, Option<i64>) = db.query_row(
        "SELECT count(*),max(version) FROM device_windows_body_schema_v1 WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != 1 || version != Some(1) {
        return Err(Error::UnsupportedSchema);
    }
    let (count, bytes, invalid): (u64, u64, bool) = db.query_row(
        "SELECT count(*),coalesce(sum(length(b.body)),0),coalesce(max(i.id IS NULL OR b.revision!=i.revision OR typeof(b.body)!='blob' OR length(b.body)!=i.bytes OR length(b.body)>1048576),0) FROM device_windows_bodies_v1 b LEFT JOIN device_import_intents_v1 i ON i.id=b.id", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if count > MAX_DOCUMENTS || bytes > MAX_LIBRARY_BYTES || invalid {
        return Err(Error::InvalidSchema);
    }
    Ok(true)
}

pub(super) fn initialize(tx: &Transaction<'_>) -> Result<()> {
    supporting_schema(tx)?;
    if !check(tx)? {
        tx.execute_batch(DDL)?;
    }
    check(tx)?;
    Ok(())
}

// A trigger on either producer table can substitute a reservation or publish
// side effects. Validate both durable and TEMP trigger namespaces in this tx.
fn supporting_schema(db: &Connection) -> Result<()> {
    // ReaderStore's catalog owns three exact source triggers. Validate their
    // definitions, never admit another producer solely by its familiar name.
    crate::store::catalog::schema::validate_source_triggers(db)
        .map_err(|_| Error::InvalidSchema)?;
    let shadow: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE name GLOB 'device_windows_*' OR tbl_name GLOB 'device_windows_*' OR name IN('sources','device_import_intents_v1','device_documents_v1'))", [], |r| r.get(0))?;
    if shadow {
        return Err(Error::InvalidSchema);
    }
    for schema in ["sqlite_schema", "sqlite_temp_schema"] {
        let sql = format!(
            "SELECT EXISTS(SELECT 1 FROM {schema} WHERE type='trigger' AND tbl_name IN('device_import_intents_v1','device_documents_v1','device_windows_bodies_v1','device_windows_body_schema_v1'))"
        );
        if db.query_row(&sql, [], |r| r.get::<_, bool>(0))? {
            return Err(Error::InvalidSchema);
        }
    }
    Ok(())
}

pub(super) fn reserve(tx: &Transaction<'_>, intent: &ImportIntent, bytes: &[u8]) -> Result<()> {
    if bytes.len() != intent.bytes as usize
        || blake3::hash(bytes).to_hex().as_str() != intent.revision
    {
        return Err(Error::Conflict);
    }
    tx.execute(
        "INSERT INTO device_windows_bodies_v1(id,revision,body) VALUES(?1,?2,?3)",
        params![intent.id, intent.revision, bytes],
    )?;
    fence(tx, intent, bytes)
}

pub(super) fn load(store: &mut ReaderStore, intent: &ImportIntent) -> Result<Vec<u8>> {
    store.with_library_connection(|db| {
        if !check(db)? {
            return Err(Error::IncompleteImport);
        }
        let bytes: Vec<u8> = db.query_row("SELECT body FROM device_windows_bodies_v1 WHERE id=?1 AND revision=?2 AND length(body)<=1048576", params![intent.id,intent.revision], |r| r.get(0))?;
        fence(db, intent, &bytes)?;
        if blake3::hash(&bytes).to_hex().as_str() != intent.revision || std::str::from_utf8(&bytes).is_err() {
            return Err(Error::IncompleteImport);
        }
        Ok(bytes)
    })
}

pub(super) fn fence(db: &Connection, intent: &ImportIntent, bytes: &[u8]) -> Result<()> {
    fence_state(db, intent, bytes, "reserved")
}
pub(super) fn published(db: &Connection, intent: &ImportIntent, bytes: &[u8]) -> Result<()> {
    fence_state(db, intent, bytes, "published")
}
fn fence_state(db: &Connection, intent: &ImportIntent, bytes: &[u8], state: &str) -> Result<()> {
    supporting_schema(db)?;
    if !check(db)? {
        return Err(Error::IncompleteImport);
    }
    let exact: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM device_import_intents_v1 i JOIN device_windows_bodies_v1 b ON b.id=i.id WHERE i.id=?1 AND i.name=?2 AND i.revision=?3 AND i.bytes=?4 AND i.parent_identity=?5 AND i.root_identity IS ?6 AND i.state=?8 AND b.revision=i.revision AND b.body=?7)", params![intent.id,intent.name,intent.revision,intent.bytes,intent.parent_identity,intent.root_identity,bytes,state], |r| r.get(0))?;
    if !exact || bytes.len() != intent.bytes as usize {
        return Err(Error::Conflict);
    }
    Ok(())
}
