//! Exact v1 namespace: unknown/partial/future databases are never repaired.
use super::*;
const TABLES: [(&str,&str);4] = [
("capture_meta","CREATE TABLE capture_meta(id INTEGER PRIMARY KEY CHECK(id=1),version INTEGER NOT NULL,repair_after TEXT NOT NULL CHECK(length(repair_after)<=64))"),
("capture_pending","CREATE TABLE capture_pending(id TEXT PRIMARY KEY CHECK(length(id)=64),record BLOB NOT NULL CHECK(typeof(record)='blob' AND length(record)<=1048576),bytes INTEGER NOT NULL CHECK(bytes>0 AND bytes=length(record)))"),
("capture_bodies","CREATE TABLE capture_bodies(id TEXT PRIMARY KEY CHECK(length(id)=64),projection BLOB NOT NULL CHECK(typeof(projection)='blob' AND length(projection)<=2097152),bytes INTEGER NOT NULL CHECK(bytes>0 AND bytes=length(projection)))"),
("capture_receipts","CREATE TABLE capture_receipts(id TEXT PRIMARY KEY CHECK(length(id)=64),content_id TEXT NOT NULL CHECK(length(content_id)=64) REFERENCES capture_bodies(id))"),
];
const APPLICATION: u32 = 0x4d445243;
pub(super) fn initialize(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.pragma_update(None, "application_id", APPLICATION)?;
    for (_, sql) in TABLES {
        transaction.execute_batch(sql)?;
    }
    transaction.execute("INSERT INTO capture_meta VALUES(1,1,'')", [])?;
    transaction.commit()?;
    verify(connection)
}
pub(super) fn verify(connection: &Connection) -> Result<()> {
    let app: u32 = connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let mut stmt = connection.prepare(
        "SELECT CASE WHEN length(name)<=64 THEN name ELSE NULL END,CASE WHEN length(sql)<=4096 THEN sql ELSE NULL END FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 5",
    )?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if app != APPLICATION
        || rows.len() != 4
        || rows
            .iter()
            .any(|(name, sql)| !TABLES.iter().any(|(n, s)| name == n && sql == s))
    {
        return Err(CaptureError::Invalid);
    }
    let (version, count): (u64, u64) = connection.query_row(
        "SELECT version,(SELECT count(*) FROM capture_meta) FROM capture_meta WHERE id=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if version != 1 || count != 1 {
        return Err(CaptureError::Invalid);
    }
    Ok(())
}
