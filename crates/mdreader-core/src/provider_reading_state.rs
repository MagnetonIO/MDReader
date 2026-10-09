//! Native owner metadata for an exact admitted provider revision. No network,
//! document import, renderer persistence, source grants or remote edits.
pub(crate) mod schema;
use crate::ReaderStore;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reference {
    pub source_id: String,
    pub path: String,
    pub revision: String,
}
impl Reference {
    pub fn validate(&self) -> Result<(), Error> {
        let digest = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if !self.source_id.strip_prefix("provider:").is_some_and(digest)
            || !self
                .path
                .strip_prefix("records/")
                .and_then(|value| value.strip_suffix(".md"))
                .is_some_and(digest)
            || !digest(&self.revision)
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub reference: Reference,
    pub pinned: bool,
    pub line: u32,
    pub line_end: u32,
    pub generation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("invalid")]
    Invalid,
    #[error("stale")]
    Stale,
    #[error("conflict")]
    Conflict,
    #[error("schema")]
    Schema,
    #[error("limit")]
    Limit,
    #[error("storage")]
    Storage,
}
impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self::Storage
    }
}
fn exact(store: &ReaderStore, reference: &Reference) -> Result<u32, Error> {
    reference.validate()?;
    let document = store
        .read_document(
            &reference.source_id,
            &reference.path,
            Some(&reference.revision),
        )
        .map_err(|_| Error::Stale)?;
    u32::try_from(document.markdown.lines().count().max(1)).map_err(|_| Error::Limit)
}
fn current(
    db: &rusqlite::Connection,
    reference: &Reference,
    line_end: u32,
) -> Result<State, Error> {
    let row: Option<(bool, u32, u64)> = db.query_row("SELECT pinned,line,generation FROM provider_reading_states WHERE source_id=?1 AND path=?2 AND revision=?3", params![reference.source_id,reference.path,reference.revision], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
    let (pinned, line, generation) = row.unwrap_or((false, 1, 0));
    if line == 0 || line > line_end || generation > i64::MAX as u64 {
        return Err(Error::Schema);
    }
    Ok(State {
        reference: reference.clone(),
        pinned,
        line,
        line_end,
        generation,
    })
}
pub fn read(store: &mut ReaderStore, reference: &Reference) -> Result<State, Error> {
    let line_end = exact(store, reference)?;
    store.with_library_connection(|db| {
        schema::check(db).map_err(|_| Error::Schema)?;
        current(db, reference, line_end)
    })
}
pub fn update(
    store: &mut ReaderStore,
    reference: &Reference,
    expected: u64,
    pinned: Option<bool>,
    line: Option<u32>,
) -> Result<State, Error> {
    if expected >= i64::MAX as u64 || pinned.is_none() && line.is_none() {
        return Err(Error::Invalid);
    }
    let line_end = exact(store, reference)?;
    if line.is_some_and(|line| line == 0 || line > line_end) {
        return Err(Error::Invalid);
    }
    store.with_library_connection(|db| {
        schema::check(db).map_err(|_| Error::Schema)?;
        let tx = db.transaction()?;
        // Authority is observed again in the same transaction as the metadata CAS.
        let enabled: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM core_provider_records r JOIN core_provider_sources p ON p.reader_source_id=r.reader_source_id JOIN core_provider_slots g ON g.library_id=p.library_id AND g.service_source_id=p.service_source_id AND g.binding_digest=p.binding_digest JOIN sources s ON s.id=p.reader_source_id WHERE r.reader_source_id=?1 AND r.path=?2 AND r.revision=?3 AND g.enabled=1 AND s.enabled=1)", params![reference.source_id,reference.path,reference.revision], |row| row.get(0))?;
        if !enabled { return Err(Error::Stale); }
        let previous = current(&tx, reference, line_end)?;
        if previous.generation != expected { return Err(Error::Conflict); }
        if previous.generation == 0 {
            let count: u32 = tx.query_row("SELECT count(*) FROM provider_reading_states", [], |row| row.get(0))?;
            if count >= 10000 { return Err(Error::Limit); }
        }
        let next = State { pinned: pinned.unwrap_or(previous.pinned), line: line.unwrap_or(previous.line), generation: expected + 1, ..previous };
        if tx.execute("INSERT INTO provider_reading_states VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(source_id,path,revision) DO UPDATE SET pinned=excluded.pinned,line=excluded.line,generation=excluded.generation WHERE provider_reading_states.generation=?7", params![reference.source_id,reference.path,reference.revision,next.pinned,next.line,next.generation,expected])? != 1 { return Err(Error::Conflict); }
        tx.commit()?;
        Ok(next)
    })
}
