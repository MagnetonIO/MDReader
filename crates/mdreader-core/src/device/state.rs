//! One monotonic owner rule for local and incoming pin/line mutations.
use super::*;
use serde::{Deserialize, Serialize};
pub(crate) const MAX_GENERATION: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetirementKind {
    Replaced,
    Deleted,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetirementRecord {
    pub kind: RetirementKind,
    pub operation_id: uuid::Uuid,
    pub replacement: Option<uuid::Uuid>,
}
impl RetirementRecord {
    pub(super) fn validate(&self) -> Result<()> {
        if self.operation_id.get_version_num() != 4
            || self.replacement.is_some_and(|id| id.get_version_num() != 4)
            || (self.kind == RetirementKind::Replaced) != self.replacement.is_some()
        {
            return Err(Error::InvalidSchema);
        }
        Ok(())
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct DocumentState {
    pub document: DeviceDocument,
    pub generation: u64,
    pub retirement: Option<RetirementRecord>,
}
pub(super) fn row(
    r: &rusqlite::Row<'_>,
) -> rusqlite::Result<(DeviceDocument, u64, Option<String>)> {
    Ok((super::documents::row(r)?, r.get(8)?, r.get(9)?))
}
pub(super) const COLUMNS:&str="d.id,d.name,d.source_id,d.revision,d.bytes,d.pinned,d.line,d.indexed,d.state_generation,d.retirement";
pub(super) fn load(db: &rusqlite::Connection, id: &str, active: bool) -> Result<DocumentState> {
    uuid::Uuid::parse_str(id).map_err(|_| Error::InvalidSelection)?;
    let raw=db.query_row(&format!("SELECT {COLUMNS} FROM device_documents_v1 d JOIN sources s ON s.id=d.source_id WHERE d.id=?1 AND (?2=0 OR (d.retired=0 AND s.enabled=1)) AND length(CAST(d.name AS BLOB))<=240 AND length(d.source_id)<=256 AND length(d.revision)=64 AND d.bytes BETWEEN 0 AND 1048576 AND length(CAST(coalesce(d.retirement,'') AS BLOB))<=2048"),params![id,active],row).optional()?.ok_or(Error::Unavailable)?;
    let retirement = raw.2.as_deref().map(serde_json::from_str).transpose()?;
    Ok(DocumentState {
        document: raw.0,
        generation: raw.1,
        retirement,
    })
}
pub fn current(store: &mut ReaderStore, id: &str) -> Result<DocumentState> {
    schema(store)?;
    store.with_library_connection(|db| load(db, id, true))
}
pub(super) fn local_update(
    store: &mut ReaderStore,
    id: &str,
    revision: Option<&str>,
    pinned: Option<bool>,
    line: Option<u32>,
) -> Result<()> {
    schema(store)?;
    store.with_library_connection(|db| -> Result<()> {
        let tx = db.transaction()?;
        let old = load(&tx, id, true)?;
        if revision.is_some_and(|revision| revision != old.document.revision) {
            return Err(Error::Conflict);
        }
        update_in(
            &tx,
            &old,
            pinned.unwrap_or(old.document.pinned),
            line.unwrap_or(old.document.line),
        )?;
        super::schema::initialize_in(&tx)?;
        let current = load(&tx, id, true)?;
        let mut expected = old;
        expected.generation += 1;
        expected.document.pinned = pinned.unwrap_or(expected.document.pinned);
        expected.document.line = line.unwrap_or(expected.document.line);
        if current != expected {
            return Err(Error::Conflict);
        }
        tx.commit()?;
        Ok(())
    })
}
/// Caller has already checked exact body/line bounds and owner capability.
pub(super) fn update_in(
    tx: &rusqlite::Transaction<'_>,
    old: &DocumentState,
    pinned: bool,
    line: u32,
) -> Result<()> {
    if line == 0 {
        return Err(Error::InvalidPosition);
    }
    if old.generation >= MAX_GENERATION {
        return Err(Error::StateLimit);
    }
    if tx.execute("UPDATE device_documents_v1 SET pinned=?1,line=?2,state_generation=state_generation+1 WHERE id=?3 AND revision=?4 AND source_id=?5 AND state_generation=?6 AND retired=0 AND EXISTS(SELECT 1 FROM sources WHERE id=?5 AND enabled=1)",params![pinned,line,old.document.id,old.document.revision,old.document.source_id,old.generation])?!=1 {return Err(Error::Conflict);}
    Ok(())
}

/// Device-only grant changes share the state generation fence. Retired sources
/// cannot be regranted. At exhaustion revocation remains possible, but re-enable
/// and fresh document transitions are refused rather than wrapping a generation.
pub(super) fn source_enabled(
    db: &mut rusqlite::Connection,
    id: &str,
    enabled: bool,
) -> Result<bool> {
    if !super::schema::owns_source(db, id)? {
        return Ok(false);
    }
    let tx = db.transaction()?;
    super::schema::initialize_in(&tx)?;
    let old = load(
        &tx,
        &tx.query_row(
            "SELECT id FROM device_documents_v1 WHERE source_id=?1",
            [id],
            |r| r.get::<_, String>(0),
        )?,
        false,
    )?;
    if enabled && old.retirement.is_some() {
        return Err(Error::Unavailable);
    }
    if enabled && old.generation >= MAX_GENERATION {
        return Err(Error::StateLimit);
    }
    let generation = old.generation.saturating_add(1).min(MAX_GENERATION);
    if tx.execute(
        "UPDATE sources SET enabled=?1 WHERE id=?2",
        params![enabled, id],
    )? != 1
    {
        return Err(Error::Unavailable);
    }
    if tx.execute(
        "UPDATE device_documents_v1 SET state_generation=?1 WHERE id=?2 AND state_generation=?3",
        params![generation, old.document.id, old.generation],
    )? != 1
    {
        return Err(Error::Conflict);
    }
    super::schema::initialize_in(&tx)?;
    let mut expected = old;
    expected.generation = generation;
    let actual = load(&tx, &expected.document.id, false)?;
    let permission: bool = tx.query_row("SELECT enabled FROM sources WHERE id=?1", [id], |r| {
        r.get(0)
    })?;
    if actual != expected || permission != enabled {
        return Err(Error::Conflict);
    }
    tx.commit()?;
    Ok(true)
}
