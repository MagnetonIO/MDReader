//! Validate the private grant namespace before any read or mutation.
use super::invalid;
use mdreader_core::{ReaderStore, Result};
use rusqlite::Connection;

const TABLES: [(&str, &str); 2] = [
    ("mdreader_agent_setup_meta", "CREATE TABLE mdreader_agent_setup_meta(id INTEGER PRIMARY KEY CHECK(id=1),version INTEGER NOT NULL)"),
    ("mdreader_agent_setup", "CREATE TABLE mdreader_agent_setup(id TEXT PRIMARY KEY,record TEXT NOT NULL,enabled INTEGER NOT NULL DEFAULT 0,sources TEXT NOT NULL DEFAULT '[]',generation INTEGER NOT NULL DEFAULT 0)"),
];

fn producers(db: &Connection) -> Result<()> {
    let foreign: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM temp.sqlite_schema WHERE name GLOB 'mdreader_agent_setup*' OR (type='trigger' AND tbl_name IN('mdreader_agent_setup','mdreader_agent_setup_meta'))) OR EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='trigger' AND tbl_name IN('mdreader_agent_setup','mdreader_agent_setup_meta'))",
        [], |r| r.get(0),
    )?;
    if foreign {
        return Err(invalid());
    }
    Ok(())
}

fn validate(db: &Connection) -> Result<()> {
    producers(db)?;
    let count: u64 = db.query_row(
        "SELECT count(*) FROM main.sqlite_schema WHERE name GLOB 'mdreader_agent_setup*'",
        [],
        |r| r.get(0),
    )?;
    if count != TABLES.len() as u64 {
        return Err(invalid());
    }
    for (name, expected) in TABLES {
        let (kind, actual): (String, String) = db.query_row(
            "SELECT type,sql FROM main.sqlite_schema WHERE name=?1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if kind != "table" || actual.trim() != expected {
            return Err(invalid());
        }
    }
    let marker: (u64, Option<i64>) = db.query_row(
        "SELECT count(*),max(CASE WHEN id=1 THEN version END) FROM main.mdreader_agent_setup_meta",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if marker != (1, Some(1)) {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn initialize(store: &mut ReaderStore) -> Result<()> {
    store.with_library_connection(|db| -> Result<()> {
        producers(db)?;
        let count: u64 = db.query_row(
            "SELECT count(*) FROM main.sqlite_schema WHERE name GLOB 'mdreader_agent_setup*'",
            [],
            |r| r.get(0),
        )?;
        if count == 0 {
            let tx = db.transaction()?;
            for (_, sql) in TABLES {
                tx.execute_batch(&sql.replacen("CREATE TABLE ", "CREATE TABLE main.", 1))?;
            }
            tx.execute("INSERT INTO main.mdreader_agent_setup_meta VALUES(1,1)", [])?;
            validate(&tx)?;
            tx.commit()?;
        } else {
            validate(db)?;
        }
        Ok(())
    })
}
