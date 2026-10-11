//! Borrowed-owner SQLite namespace; grants are checked before and after retrieval.
use super::{invalid, Record};
use crate::Request;
use mdreader_core::{ReaderError, ReaderStore, Result};
use rusqlite::OptionalExtension;
fn initialize(store: &mut ReaderStore) -> Result<()> {
    store.with_library_connection(|db| -> Result<()> {
        let tx = db.transaction()?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS mdreader_agent_setup_meta(id INTEGER PRIMARY KEY CHECK(id=1),version INTEGER NOT NULL); INSERT OR IGNORE INTO mdreader_agent_setup_meta VALUES(1,1);")?;
        let version: u32 = tx.query_row("SELECT version FROM mdreader_agent_setup_meta WHERE id=1", [], |r| r.get(0))?;
        if version != 1 { return Err(invalid()); }
        tx.execute_batch("CREATE TABLE IF NOT EXISTS mdreader_agent_setup(id TEXT PRIMARY KEY,record TEXT NOT NULL,enabled INTEGER NOT NULL DEFAULT 0,sources TEXT NOT NULL DEFAULT '[]',generation INTEGER NOT NULL DEFAULT 0);")?;
        tx.commit()?; Ok(())
    })
}
pub(super) fn record(store: &mut ReaderStore, id: &str) -> Result<Option<Record>> {
    initialize(store)?;
    store.with_library_connection(|db| -> Result<_> {
        let text: Option<String> = db
            .query_row(
                "SELECT record FROM mdreader_agent_setup WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        text.map(|text| serde_json::from_str(&text).map_err(|_| invalid()))
            .transpose()
    })
}
pub(super) fn save_install(store: &mut ReaderStore, id: &str, record: &Record) -> Result<()> {
    initialize(store)?;
    let json = serde_json::to_string(record).map_err(|_| invalid())?;
    store.with_library_connection(|db| -> Result<()> {
        db.execute("INSERT INTO mdreader_agent_setup(id,record) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET record=excluded.record", (id,json))?; Ok(())
    })
}
pub(super) fn grant(store: &mut ReaderStore, id: &str) -> Result<(bool, Vec<String>, u64)> {
    initialize(store)?;
    store.with_library_connection(|db| -> Result<_> {
        let row: Option<(bool, String, u64)> = db
            .query_row(
                "SELECT enabled,sources,generation FROM mdreader_agent_setup WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        match row {
            Some((enabled, json, generation)) => Ok((
                enabled,
                serde_json::from_str(&json).map_err(|_| invalid())?,
                generation,
            )),
            None => Ok((false, vec![], 0)),
        }
    })
}
pub(super) fn set(
    store: &mut ReaderStore,
    id: &str,
    enabled: bool,
    sources: &[String],
) -> Result<()> {
    initialize(store)?;
    store.with_library_connection(|db| -> Result<()> {
        db.execute("UPDATE mdreader_agent_setup SET enabled=?2,sources=?3,generation=generation+1 WHERE id=?1", (id,enabled,serde_json::to_string(sources).map_err(|_| invalid())?))?; Ok(())
    })
}
pub fn sources(store: &mut ReaderStore, id: &str) -> Result<Vec<String>> {
    let (enabled, sources, _) = grant(store, id)?;
    let record = record(store, id)?.ok_or(ReaderError::Unavailable)?;
    if !enabled || record.phase != "installed" {
        return Err(ReaderError::Unavailable);
    }
    record.selection.checked()?;
    let admitted = store.list_sources()?;
    let sources: Vec<_> = sources
        .into_iter()
        .filter(|id| admitted.iter().any(|s| s.enabled && &s.id == id))
        .collect();
    if sources.is_empty() {
        return Err(ReaderError::Unavailable);
    }
    Ok(sources)
}
pub fn authorize(store: &mut ReaderStore, id: &str, request: &Request) -> Result<u64> {
    let allowed = sources(store, id)?;
    let check = |id: &String| allowed.contains(id);
    let valid = match request {
        Request::ListSources => true,
        Request::Status { sources }
        | Request::Search { sources, .. }
        | Request::SearchRanked { sources, .. } => !sources.is_empty() && sources.iter().all(check),
        Request::Read { source_id, .. }
        | Request::Resolve { source_id, .. }
        | Request::Related { source_id, .. } => check(source_id),
        _ => false,
    };
    if !valid {
        return Err(ReaderError::Unavailable);
    }
    Ok(grant(store, id)?.2)
}
