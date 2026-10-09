use crate::{ReaderError, Result};
use rusqlite::{Connection, OptionalExtension};
const V1: &str = include_str!("migrations/001.sql");
fn unsupported() -> ReaderError {
    ReaderError::Invalid("provider reading state schema is unsupported".into())
}
fn presence(db: &Connection) -> Result<u32> {
    Ok(db.query_row("SELECT count(*) FROM sqlite_schema WHERE name GLOB 'provider_reading_*' OR (type='trigger' AND tbl_name IN ('provider_reading_schema','provider_reading_states'))", [], |row| row.get(0))?)
}
pub(crate) fn initialize(db: &Connection) -> Result<()> {
    let tx = db.unchecked_transaction()?;
    if presence(&tx)? == 0 {
        tx.execute_batch(V1)?;
    }
    check(&tx)?;
    tx.commit()?;
    Ok(())
}
fn normal(sql: &str) -> String {
    sql.trim()
        .trim_end_matches(';')
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}
/// Older profiles may omit this additive subsystem; never initialize on readonly open.
pub(crate) fn check(db: &Connection) -> Result<()> {
    match presence(db)? {
        0 => return Ok(()),
        2 => (),
        _ => return Err(unsupported()),
    }
    // Compare complete versioned definitions, including PK/FK/CHECK rules.
    // Reject attached triggers regardless of their name; a Pin cannot execute
    // an unrecognized program against unrelated durable state.
    for (name, statement) in [
        ("provider_reading_schema", 0),
        ("provider_reading_states", 2),
    ] {
        let expected = V1.split(';').nth(statement).ok_or_else(unsupported)?;
        let actual: Option<(String, String)> = db
            .query_row(
                "SELECT type,sql FROM sqlite_schema WHERE name=?1",
                [name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if actual.is_none_or(|(kind, sql)| kind != "table" || normal(&sql) != normal(expected)) {
            return Err(unsupported());
        }
    }
    let versions = db
        .prepare("SELECT version FROM provider_reading_schema")?
        .query_map([], |row| row.get::<_, u32>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if versions != [1] {
        return Err(unsupported());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_weakened_layout_and_attached_trigger_without_repairing_retained_state() {
        for trigger in [false, true] {
            let db = Connection::open_in_memory().unwrap();
            db.execute_batch("CREATE TABLE sources(id TEXT PRIMARY KEY); INSERT INTO sources VALUES('original');").unwrap();
            initialize(&db).unwrap();
            db.execute(
                "INSERT INTO provider_reading_states VALUES('original','note.md','rev',1,3,4)",
                [],
            )
            .unwrap();
            if trigger {
                db.execute_batch("CREATE TRIGGER unrelated_name AFTER UPDATE ON provider_reading_states BEGIN DELETE FROM sources; END;").unwrap();
            } else {
                db.execute_batch("ALTER TABLE provider_reading_states RENAME TO saved_states; CREATE TABLE provider_reading_states(source_id TEXT NOT NULL REFERENCES sources(id),path TEXT NOT NULL,revision TEXT NOT NULL,pinned INTEGER NOT NULL,line INTEGER NOT NULL CHECK(line>=1),generation INTEGER NOT NULL CHECK(generation>=0),PRIMARY KEY(source_id,path,revision)); INSERT INTO provider_reading_states SELECT * FROM saved_states; DROP TABLE saved_states;").unwrap();
            }
            assert!(check(&db).is_err());
            assert!(initialize(&db).is_err());
            assert_eq!(
                db.query_row(
                    "SELECT pinned,line,generation FROM provider_reading_states",
                    [],
                    |row| Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?
                    ))
                )
                .unwrap(),
                (1, 3, 4)
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM sources", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
    }
}
