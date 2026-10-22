//! Ordered, immutable migrations for the agent activity namespace. Each
//! transition and its version marker commit in one transaction.
use crate::{ReaderError, ReaderStore, Result};
use rusqlite::{Connection, OptionalExtension};

/// Applied history is immutable: add the next file, never edit one.
const MIGRATIONS: &[&str] = &[include_str!("migrations/0001_activity.sql")];
const VERSION: i64 = MIGRATIONS.len() as i64;
const OBJECTS: &str = "(name GLOB 'reader_agent_activity_*' OR name='reader_agent_ingest_cursor')";

fn definitions() -> impl Iterator<Item = &'static str> {
    MIGRATIONS
        .iter()
        .flat_map(|sql| sql.split(';'))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn validate(db: &Connection) -> Result<()> {
    let count: usize = db.query_row(
        &format!("SELECT count(*) FROM main.sqlite_schema WHERE {OBJECTS}"),
        [],
        |r| r.get(0),
    )?;
    if count != definitions().count() {
        return Err(ReaderError::Unavailable);
    }
    for definition in definitions() {
        let name = definition
            .split([' ', '('])
            .find(|word| word.starts_with("reader_"))
            .ok_or(ReaderError::Unavailable)?;
        let actual: Option<String> = db
            .query_row(
                "SELECT sql FROM main.sqlite_schema WHERE name=?1",
                [name],
                |r| r.get(0),
            )
            .optional()?;
        if actual.as_deref() != Some(definition) {
            return Err(ReaderError::Unavailable);
        }
    }
    let tables = OBJECTS.replace("name", "tbl_name");
    let foreign: bool = db.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM temp.sqlite_schema WHERE {OBJECTS} OR {tables}) OR EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='trigger' AND {tables})"),
        [],
        |r| r.get(0),
    )?;
    let marker: (usize, Option<i64>) = db.query_row(
        "SELECT count(*),max(CASE WHEN id=1 THEN version END) FROM main.reader_agent_activity_meta",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if foreign || marker != (1, Some(VERSION)) {
        return Err(ReaderError::Unavailable);
    }
    Ok(())
}

fn version(db: &Connection) -> Result<Option<i64>> {
    let meta: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='table' AND name='reader_agent_activity_meta')",
        [],
        |r| r.get(0),
    )?;
    if !meta {
        let leftovers: bool = db.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE {OBJECTS})"),
            [],
            |r| r.get(0),
        )?;
        // Objects without a marker were not written by a committed migration.
        return if leftovers {
            Err(ReaderError::Unavailable)
        } else {
            Ok(None)
        };
    }
    Ok(db.query_row(
        "SELECT max(CASE WHEN id=1 THEN version END) FROM main.reader_agent_activity_meta",
        [],
        |r| r.get(0),
    )?)
}

/// Bring the namespace to the current version. The first transition also
/// imports the project-scoped session rows in the same transaction, so an
/// interruption leaves the old rows untouched and the import replays.
pub(super) fn initialize(store: &mut ReaderStore) -> Result<()> {
    store.with_library_connection(|db| {
        let current = version(db)?.unwrap_or(0);
        if current > VERSION {
            return Err(ReaderError::NewerDatabase(current));
        }
        if current < VERSION {
            let tx = db.transaction()?;
            for sql in MIGRATIONS
                .iter()
                .skip(usize::try_from(current).map_err(|_| ReaderError::Unavailable)?)
            {
                tx.execute_batch(sql)?;
            }
            if current == 0 {
                tx.execute(
                    "INSERT INTO reader_agent_activity_meta VALUES(1,?1,0)",
                    [VERSION],
                )?;
                super::legacy::import(&tx)?;
            } else {
                tx.execute(
                    "UPDATE reader_agent_activity_meta SET version=?1 WHERE id=1",
                    [VERSION],
                )?;
            }
            validate(&tx)?;
            tx.commit()?;
        }
        validate(db)
    })
}
