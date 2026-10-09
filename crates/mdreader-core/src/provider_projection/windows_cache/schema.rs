//! Exact private namespace: never repair unknown, partial or future caches.
use super::*;
const APPLICATION: u32 = 0x4d445250;
const TABLES: [(&str, &str); 2] = [
    ("provider_meta", "CREATE TABLE provider_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL,library_id TEXT NOT NULL CHECK(length(library_id)=36))"),
    ("provider_bodies", "CREATE TABLE provider_bodies(path TEXT PRIMARY KEY CHECK(length(path)=197),body_hash TEXT NOT NULL CHECK(length(body_hash)=64),body BLOB NOT NULL CHECK(typeof(body)='blob' AND length(body)<=2097152))"),
];
pub(super) fn initialize(connection: &mut Connection, library: &str) -> Result<()> {
    let tx = connection.transaction()?;
    tx.pragma_update(None, "application_id", APPLICATION)?;
    for (_, sql) in TABLES {
        tx.execute_batch(sql)?;
    }
    tx.execute("INSERT INTO provider_meta VALUES(1,1,?1)", [library])?;
    tx.commit()?;
    verify(connection, library)
}
pub(super) fn verify(connection: &Connection, library: &str) -> Result<()> {
    let app: u32 = connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let mut stmt = connection.prepare("SELECT CASE WHEN length(name)<=64 THEN name END,CASE WHEN length(sql)<=4096 THEN sql END FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 3")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if app != APPLICATION
        || rows.len() != TABLES.len()
        || rows
            .iter()
            .any(|(name, sql)| !TABLES.iter().any(|(n, s)| name == n && sql == s))
    {
        return Err(invalid("unsupported provider cache namespace"));
    }
    let (version, stored, count): (u64,String,u64) = connection.query_row("SELECT version,CASE WHEN length(library_id)=36 THEN library_id END,(SELECT count(*) FROM provider_meta) FROM provider_meta WHERE singleton=1", [], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if version != 1 || stored != library || count != 1 {
        return Err(ReaderError::Unavailable);
    }
    Ok(())
}
