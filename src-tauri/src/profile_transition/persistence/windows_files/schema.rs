//! Exact two-table Windows state namespace; no initialization of partial,
//! foreign or newer journals. Unknown JSON fields stay byte-preserved in body.
use super::*;
const APPLICATION: u32 = 0x4d44524a;
const TABLES: [(&str, &str); 2] = [
    ("reader_json", "CREATE TABLE reader_json(name TEXT PRIMARY KEY CHECK(name IN('session.json','settings.json')),body BLOB NOT NULL CHECK(typeof(body)='blob' AND length(body)<=1048576))"),
    ("reader_json_meta", "CREATE TABLE reader_json_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL)"),
];
pub(super) fn initialize(connection: &mut Connection) -> Result<()> {
    let tx = connection.transaction().map_err(|_| Error::Storage)?;
    tx.pragma_update(None, "application_id", APPLICATION)
        .map_err(|_| Error::Storage)?;
    for (_, sql) in TABLES {
        tx.execute_batch(sql).map_err(|_| Error::Storage)?;
    }
    tx.execute("INSERT INTO reader_json_meta VALUES(1,1)", [])
        .map_err(|_| Error::Storage)?;
    tx.commit().map_err(|_| Error::PublicationUncertain)?;
    verify(connection)
}
pub(super) fn verify(connection: &Connection) -> Result<()> {
    let application: u32 = connection
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .map_err(|_| Error::Storage)?;
    let mut statement = connection.prepare("SELECT CASE WHEN length(name)<=64 THEN name END,CASE WHEN length(sql)<=4096 THEN sql END FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 3").map_err(|_| Error::UnsupportedSchema)?;
    let rows = statement
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|_| Error::UnsupportedSchema)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| Error::UnsupportedSchema)?;
    if application != APPLICATION
        || rows.len() != TABLES.len()
        || rows
            .iter()
            .any(|(name, sql)| !TABLES.iter().any(|(n, s)| name == n && sql == s))
    {
        return Err(Error::UnsupportedSchema);
    }
    let (version, count): (u64,u64) = connection.query_row("SELECT version,(SELECT count(*) FROM reader_json_meta) FROM reader_json_meta WHERE singleton=1", [], |r|Ok((r.get(0)?,r.get(1)?))).map_err(|_| Error::UnsupportedSchema)?;
    if version != 1 || count != 1 {
        return Err(Error::UnsupportedSchema);
    }
    let (rows, invalid): (u64, bool) = connection.query_row("SELECT count(*),EXISTS(SELECT 1 FROM reader_json WHERE name NOT IN('session.json','settings.json') OR typeof(body)!='blob' OR length(body)>1048576) FROM reader_json", [], |r|Ok((r.get(0)?,r.get(1)?))).map_err(|_| Error::UnsupportedSchema)?;
    if rows > 2 || invalid {
        return Err(Error::UnsupportedSchema);
    }
    Ok(())
}
