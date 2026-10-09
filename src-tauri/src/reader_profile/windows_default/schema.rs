use mdreader_profile::{Error, Result};
use rusqlite::{Connection, OptionalExtension};

const NAME: &str = "native_default_windows_v1";
const DDL: &str = "CREATE TABLE native_default_windows_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1),generation INTEGER NOT NULL CHECK(generation>=1 AND generation<=9007199254740991),target TEXT CHECK(target IS NULL OR length(CAST(target AS BLOB))<=32768))";
pub(super) fn initialize(db: &mut Connection, fresh: bool) -> Result<()> {
    let tx = db.transaction().map_err(|_| Error::Storage)?;
    if !exists(&tx)? {
        if !fresh {
            return Err(Error::Integrity);
        }
        tx.execute_batch(DDL).map_err(|_| Error::Storage)?;
        tx.execute(
            "INSERT INTO native_default_windows_v1 VALUES(1,1,1,NULL)",
            [],
        )
        .map_err(|_| Error::Storage)?;
    }
    verify(&tx)?;
    tx.commit().map_err(|_| Error::ReconciliationRequired)
}
fn exists(db: &Connection) -> Result<bool> {
    let definition: Option<String> = db
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name=?1 AND type='table'",
            [NAME],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| Error::Storage)?;
    let normalized = |text: &str| {
        text.chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect::<String>()
    };
    if definition
        .as_ref()
        .is_some_and(|text| normalized(text) != normalized(DDL))
    {
        return Err(Error::Conflict);
    }
    for schema in ["sqlite_schema", "sqlite_temp_schema"] {
        let query = format!("SELECT EXISTS(SELECT 1 FROM {schema} WHERE (name GLOB 'native_default_windows_*' OR tbl_name GLOB 'native_default_windows_*') AND NOT(type='table' AND name=?1))");
        if db
            .query_row(&query, [NAME], |row| row.get::<_, bool>(0))
            .map_err(|_| Error::Storage)?
        {
            return Err(Error::Conflict);
        }
    }
    Ok(definition.is_some())
}
pub(super) fn verify(db: &Connection) -> Result<()> {
    if !exists(db)? {
        return Err(Error::Integrity);
    }
    let count: u64 = db
        .query_row("SELECT count(*) FROM native_default_windows_v1", [], |r| {
            r.get(0)
        })
        .map_err(|_| Error::Storage)?;
    if count != 1 {
        return Err(Error::Conflict);
    }
    Ok(())
}
