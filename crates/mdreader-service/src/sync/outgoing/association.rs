//! Bounded opaque intent history. Core alone owns documents and asset records.
use super::*;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
pub(in crate::sync) struct Stored {
    pub intent: Intent,
    pub applied: Option<u64>,
}
pub(in crate::sync) fn get(
    db: &Connection,
    scope: &SyncScope,
    operation: &str,
) -> Result<Option<Stored>> {
    if !schema::present(db)? {
        return Ok(None);
    }
    schema::require(db)?;
    let key = super::super::incoming::scope_key(scope)?;
    let row:Option<(String,String,Option<u64>,String,String,String)>=db.query_row("SELECT intent,state,sequence,wire_item,wire_revision,native_document FROM native_outgoing_intents_v1 WHERE scope=?1 AND operation_id=?2 AND length(CAST(intent AS BLOB))<=4096",params![key,operation],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
    row.map(|(encoded, state, sequence, item, revision, document)| {
        let intent: Intent = serde_json::from_str(&encoded)?;
        intent.validate(scope)?;
        if intent.operation.operation_id != operation
            || intent.operation.item_id != item
            || intent.operation.revision_id != revision
            || intent.native_document != document
            || (state == "applied") != sequence.is_some()
        {
            return Err(Error::Schema);
        }
        Ok(Stored {
            intent,
            applied: sequence,
        })
    })
    .transpose()
}
pub(in crate::sync) fn by_revision(
    db: &Connection,
    scope: &SyncScope,
    revision: &str,
) -> Result<Option<Stored>> {
    if !schema::present(db)? {
        return Ok(None);
    }
    schema::require(db)?;
    let key = super::super::incoming::scope_key(scope)?;
    let operation:Option<String>=db.query_row("SELECT operation_id FROM native_outgoing_intents_v1 WHERE scope=?1 AND wire_revision=?2",params![key,revision],|r|r.get(0)).optional()?;
    operation
        .map(|op| get(db, scope, &op)?.ok_or(Error::Schema))
        .transpose()
}
pub(in crate::sync) fn require_unqueued(
    tx: &Transaction<'_>,
    scope: &SyncScope,
    document: &str,
) -> Result<()> {
    if !schema::present(tx)? {
        return Ok(());
    }
    schema::require(tx)?;
    let pending:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_outgoing_intents_v1 WHERE scope=?1 AND native_document=?2 AND state='queued')",params![super::super::incoming::scope_key(scope)?,document],|r|r.get(0))?;
    if pending {
        return Err(Error::Busy);
    }
    Ok(())
}
pub(in crate::sync) fn insert(
    tx: &Transaction<'_>,
    scope: &SyncScope,
    intent: &Intent,
) -> Result<()> {
    let encoded = intent.encode(scope)?;
    schema::initialize(tx)?;
    require_unqueued(tx, scope, &intent.native_document)?;
    let count: u64 = tx.query_row("SELECT count(*) FROM native_outgoing_intents_v1", [], |r| {
        r.get(0)
    })?;
    if count >= 4096 {
        return Err(Error::Limit);
    }
    tx.execute(
        "INSERT INTO native_outgoing_intents_v1 VALUES(?1,?2,?3,?4,?5,?6,'queued',NULL)",
        params![
            super::super::incoming::scope_key(scope)?,
            intent.operation.operation_id,
            intent.operation.item_id,
            intent.operation.revision_id,
            intent.native_document,
            encoded
        ],
    )?;
    verify(tx, scope, intent, None)
}
pub(in crate::sync) fn verify(
    db: &Connection,
    scope: &SyncScope,
    intent: &Intent,
    sequence: Option<u64>,
) -> Result<()> {
    let stored = get(db, scope, &intent.operation.operation_id)?.ok_or(Error::Conflict)?;
    if stored.intent != *intent || stored.applied != sequence {
        return Err(Error::Conflict);
    }
    Ok(())
}
pub(in crate::sync) fn complete(
    tx: &Transaction<'_>,
    scope: &SyncScope,
    intent: &Intent,
    sequence: u64,
) -> Result<()> {
    verify(tx, scope, intent, None)?;
    if sequence == 0 || sequence > 9_007_199_254_740_991 {
        return Err(Error::Conflict);
    }
    if tx.execute("UPDATE native_outgoing_intents_v1 SET state='applied',sequence=?1 WHERE scope=?2 AND operation_id=?3 AND state='queued' AND sequence IS NULL",params![sequence,super::super::incoming::scope_key(scope)?,intent.operation.operation_id])?!=1 {return Err(Error::Conflict);}
    verify(tx, scope, intent, Some(sequence))
}

/// An already-applied mapping cannot disappear and silently mint a new wire item.
pub(in crate::sync) fn verify_existing_head(
    tx: &Transaction<'_>,
    scope: &SyncScope,
    document: &str,
    head: Option<&super::super::incoming::Head>,
) -> Result<()> {
    if !schema::present(tx)? {
        return Ok(());
    }
    schema::require(tx)?;
    let key = super::super::incoming::scope_key(scope)?;
    let mismatch:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_outgoing_intents_v1 WHERE scope=?1 AND native_document=?2 AND state='applied' AND (?3 IS NULL OR wire_item!=?3))",params![key,document,head.map(|h|h.item.as_str())],|r|r.get(0))?;
    if mismatch {
        return Err(Error::Conflict);
    }
    Ok(())
}
