//! Service-owned durable mutation intent. This module never opens a database or
//! changes the owner's journal mode, synchronous policy or user_version.
use crate::*;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
#[path = "journal/inspection.rs"]
mod inspection;
pub use inspection::{archive, inspect, ArchivedMutation};

const MAX_RECORD: usize = 32 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationState {
    Prepared,
    Dispatched,
    Unknown,
    Accepted,
    Published,
    Conflict,
}
impl MutationState {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "prepared" => Ok(Self::Prepared),
            "dispatched" => Ok(Self::Dispatched),
            "unknown" => Ok(Self::Unknown),
            "accepted" => Ok(Self::Accepted),
            "published" => Ok(Self::Published),
            "conflict" => Ok(Self::Conflict),
            _ => Err(ProviderError::Persistence),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    v: u8,
    operation_id: uuid::Uuid,
    binding: SourceBinding,
    key: RecordKey,
    expected: DocumentVersion,
    body_hash: [u8; 32],
}
/// Linear in-process authority returned only after durable prepared->dispatched
/// commit. It is neither Clone nor Deserialize. A crash never recreates it.
pub struct DispatchPermit {
    intent: Intent,
}
impl DispatchPermit {
    pub fn operation_id(&self) -> uuid::Uuid {
        self.intent.operation_id
    }
    pub fn binding(&self) -> &SourceBinding {
        &self.intent.binding
    }
    pub fn key(&self) -> &RecordKey {
        &self.intent.key
    }
    pub fn expected(&self) -> &DocumentVersion {
        &self.intent.expected
    }
    pub fn body_hash(&self) -> [u8; 32] {
        self.intent.body_hash
    }
    pub fn validate(&self, binding: &SourceBinding, body: &MarkdownBytes) -> Result<()> {
        if *binding != self.intent.binding || body_hash(body.as_bytes()) != self.intent.body_hash {
            return Err(ProviderError::Unauthorized);
        }
        Ok(())
    }
}
impl std::fmt::Debug for DispatchPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatchPermit")
            .field("operation_id", &self.operation_id())
            .finish_non_exhaustive()
    }
}
#[derive(Debug)]
pub struct PendingMutation {
    pub operation_id: uuid::Uuid,
    pub key: RecordKey,
    pub expected: DocumentVersion,
    pub body: MarkdownBytes,
    pub state: MutationState,
    pub receipt: Option<WriteReceipt>,
}
fn db<T>(value: std::result::Result<T, rusqlite::Error>) -> Result<T> {
    value.map_err(|_| ProviderError::Persistence)
}
fn initialize(connection: &mut Connection) -> Result<()> {
    if db(connection.query_row("PRAGMA synchronous", [], |r| r.get::<_, u8>(0)))? < 2 {
        return Err(ProviderError::Persistence);
    }
    let tx = db(connection.transaction())?;
    let tables:u64=db(tx.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('mdreader_provider_meta','mdreader_provider_mutations','mdreader_provider_conflicts')",[],|r|r.get(0)))?;
    if tables == 0 {
        db(tx.execute_batch("CREATE TABLE mdreader_provider_meta(id INTEGER PRIMARY KEY CHECK(id=1),version INTEGER NOT NULL);INSERT INTO mdreader_provider_meta VALUES(1,1);CREATE TABLE mdreader_provider_mutations(operation_id TEXT PRIMARY KEY,binding BLOB NOT NULL,record TEXT NOT NULL,digest BLOB NOT NULL,body BLOB NOT NULL,state TEXT NOT NULL CHECK(state IN ('prepared','dispatched','unknown','accepted','published','conflict')),receipt TEXT);"))?;
    } else {
        if !matches!(tables, 2 | 3) {
            return Err(ProviderError::Persistence);
        }
        let version: u64 = db(tx.query_row(
            "SELECT version FROM mdreader_provider_meta WHERE id=1",
            [],
            |r| r.get(0),
        ))?;
        if !matches!((tables, version), (2, 1) | (3, 2)) {
            return Err(ProviderError::Persistence);
        }
    }
    let version: u64 = db(tx.query_row(
        "SELECT version FROM mdreader_provider_meta WHERE id=1",
        [],
        |r| r.get(0),
    ))?;
    if version == 1 {
        db(tx.execute_batch("CREATE TABLE mdreader_provider_conflicts(operation_id TEXT PRIMARY KEY,version TEXT NOT NULL,body BLOB NOT NULL,digest BLOB NOT NULL);UPDATE mdreader_provider_meta SET version=2 WHERE id=1;"))?;
    }
    db(tx.prepare("SELECT operation_id,binding,record,digest,body,state,receipt FROM mdreader_provider_mutations LIMIT 0"))?;
    db(tx.prepare(
        "SELECT operation_id,version,body,digest FROM mdreader_provider_conflicts LIMIT 0",
    ))?;
    db(tx.commit())
}

fn record(
    connection: &Connection,
    binding: &SourceBinding,
    id: uuid::Uuid,
) -> Result<(Intent, MarkdownBytes, MutationState, Option<WriteReceipt>)> {
    type Row = (String, Vec<u8>, Vec<u8>, String, Option<String>);
    let row:Option<Row>=db(connection.query_row("SELECT record,digest,body,state,receipt FROM mdreader_provider_mutations WHERE operation_id=?1 AND binding=?2",rusqlite::params![id.to_string(),binding.digest().as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional())?;
    let Some((json, digest, body, state, receipt)) = row else {
        return Err(ProviderError::Unauthorized);
    };
    if json.len() > MAX_RECORD || digest != body_hash(json.as_bytes()) {
        return Err(ProviderError::Persistence);
    }
    let intent: Intent = serde_json::from_str(&json).map_err(|_| ProviderError::Persistence)?;
    if intent.v != 1
        || intent.operation_id != id
        || intent.binding != *binding
        || body_hash(&body) != intent.body_hash
    {
        return Err(ProviderError::Persistence);
    }
    let body = MarkdownBytes::new(body)?;
    let state = MutationState::parse(&state)?;
    let receipt = receipt
        .map(|json| {
            if json.len() > MAX_RECORD {
                return Err(ProviderError::Persistence);
            }
            let receipt: WriteReceipt =
                serde_json::from_str(&json).map_err(|_| ProviderError::Persistence)?;
            verify_receipt(&intent, &body, &receipt)?;
            Ok(receipt)
        })
        .transpose()?;
    if matches!(state, MutationState::Accepted | MutationState::Published) != receipt.is_some() {
        return Err(ProviderError::Persistence);
    }
    Ok((intent, body, state, receipt))
}
/// Commit the full intended body and exact authority before any provider effect.
/// No caller operation UUID is accepted: a fresh intent cannot collide/replay.
pub fn prepare(
    connection: &mut Connection,
    binding: &SourceBinding,
    key: &RecordKey,
    expected: &DocumentVersion,
    body: &MarkdownBytes,
) -> Result<uuid::Uuid> {
    prepare_with_id(
        connection,
        binding,
        key,
        expected,
        body,
        uuid::Uuid::new_v4(),
    )
}
/// Owner-generated stable operation identity makes a lost prepare reply replay
/// the same intent. Matching replays never create a fresh dispatch permit.
pub fn prepare_with_id(
    connection: &mut Connection,
    binding: &SourceBinding,
    key: &RecordKey,
    expected: &DocumentVersion,
    body: &MarkdownBytes,
    id: uuid::Uuid,
) -> Result<uuid::Uuid> {
    if id.get_version_num() != 4 {
        return Err(ProviderError::Invalid);
    }
    initialize(connection)?;
    match record(connection, binding, id) {
        Ok((intent, prior, _, _)) => {
            if intent.key != *key || intent.expected != *expected || prior != *body {
                return Err(ProviderError::Conflict);
            }
            return Ok(id);
        }
        Err(ProviderError::Unauthorized) => {}
        Err(error) => return Err(error),
    }
    let intent = Intent {
        v: 1,
        operation_id: id,
        binding: binding.clone(),
        key: key.clone(),
        expected: expected.clone(),
        body_hash: body_hash(body.as_bytes()),
    };
    let json = serde_json::to_string(&intent).map_err(|_| ProviderError::Invalid)?;
    if json.len() > MAX_RECORD {
        return Err(ProviderError::TooLarge);
    }
    let tx = db(connection.transaction())?;
    // Retain live/uncertain mutations. Pruning is limited to terminal publication.
    db(tx.execute("DELETE FROM mdreader_provider_mutations WHERE state='published' AND operation_id NOT IN (SELECT operation_id FROM mdreader_provider_mutations WHERE state='published' ORDER BY rowid DESC LIMIT 64)",[]))?;
    let count: u64 = db(tx.query_row(
        "SELECT count(*) FROM mdreader_provider_mutations WHERE state!='published'",
        [],
        |r| r.get(0),
    ))?;
    if count >= 128 {
        return Err(ProviderError::TooLarge);
    }
    let mut total: u64 = db(tx.query_row(
        "SELECT (SELECT coalesce(sum(length(body)+length(record)),0) FROM mdreader_provider_mutations)+(SELECT coalesce(sum(length(body)+length(version)),0) FROM mdreader_provider_conflicts)",
        [],
        |r| r.get(0),
    ))?;
    while total.saturating_add((body.as_bytes().len() + json.len()) as u64) > 256 * 1024 * 1024 {
        if db(tx.execute("DELETE FROM mdreader_provider_mutations WHERE rowid=(SELECT rowid FROM mdreader_provider_mutations WHERE state='published' ORDER BY rowid LIMIT 1)",[]))?==0{return Err(ProviderError::TooLarge);}
        total = db(tx.query_row(
            "SELECT (SELECT coalesce(sum(length(body)+length(record)),0) FROM mdreader_provider_mutations)+(SELECT coalesce(sum(length(body)+length(version)),0) FROM mdreader_provider_conflicts)",
            [],
            |r| r.get(0),
        ))?;
    }
    db(tx.execute(
        "INSERT INTO mdreader_provider_mutations VALUES(?1,?2,?3,?4,?5,'prepared',NULL)",
        rusqlite::params![
            id.to_string(),
            binding.digest().as_slice(),
            json,
            body_hash(json.as_bytes()).as_slice(),
            body.as_bytes()
        ],
    ))?;
    db(tx.commit())?;
    Ok(id)
}
/// Commit dispatch intent first. Retrying this call after interruption cannot
/// create a second permit or resend a possibly accepted provider mutation.
pub fn begin_dispatch(
    connection: &mut Connection,
    binding: &SourceBinding,
    id: uuid::Uuid,
) -> Result<DispatchPermit> {
    initialize(connection)?;
    let tx = db(connection.transaction())?;
    let (intent, _, state, _) = record(&tx, binding, id)?;
    if state != MutationState::Prepared {
        return Err(ProviderError::UnknownWrite { operation_id: id });
    }
    if db(tx.execute("UPDATE mdreader_provider_mutations SET state='dispatched' WHERE operation_id=?1 AND state='prepared'",[id.to_string()]))?!=1{return Err(ProviderError::Conflict);}
    db(tx.commit())?;
    Ok(DispatchPermit { intent })
}
pub fn load(
    connection: &mut Connection,
    binding: &SourceBinding,
    id: uuid::Uuid,
) -> Result<PendingMutation> {
    initialize(connection)?;
    let (intent, body, state, receipt) = record(connection, binding, id)?;
    Ok(PendingMutation {
        operation_id: id,
        key: intent.key,
        expected: intent.expected,
        body,
        state,
        receipt,
    })
}
pub fn mark_unknown(
    connection: &mut Connection,
    binding: &SourceBinding,
    id: uuid::Uuid,
) -> Result<()> {
    initialize(connection)?;
    let (_, _, state, _) = record(connection, binding, id)?;
    if state == MutationState::Unknown {
        return Ok(());
    }
    if state != MutationState::Dispatched {
        return Err(ProviderError::Conflict);
    }
    db(connection.execute("UPDATE mdreader_provider_mutations SET state='unknown' WHERE operation_id=?1 AND state='dispatched'",[id.to_string()]))?;
    Ok(())
}
/// Startup can conservatively classify interrupted dispatches as uncertain.
/// It never makes them eligible for re-dispatch.
pub fn recover_interrupted(connection: &mut Connection, binding: &SourceBinding) -> Result<usize> {
    initialize(connection)?;
    db(connection.execute("UPDATE mdreader_provider_mutations SET state='unknown' WHERE binding=?1 AND state='dispatched'",[binding.digest().as_slice()]))
}
fn verify_receipt(intent: &Intent, body: &MarkdownBytes, receipt: &WriteReceipt) -> Result<()> {
    if receipt.operation_id != intent.operation_id
        || receipt.key != intent.key
        || receipt.body_hash != intent.body_hash
        || receipt.size_bytes != body.as_bytes().len() as u64
    {
        return Err(ProviderError::IdentityMismatch);
    }
    Ok(())
}
/// A valid provider response or a verified desired-body read can settle an
/// uncertain intent. This is local durability, not index completion.
pub fn accept_receipt(
    connection: &mut Connection,
    binding: &SourceBinding,
    receipt: &WriteReceipt,
) -> Result<()> {
    initialize(connection)?;
    let tx = db(connection.transaction())?;
    let (intent, body, state, previous) = record(&tx, binding, receipt.operation_id)?;
    verify_receipt(&intent, &body, receipt)?;
    if matches!(state, MutationState::Accepted | MutationState::Published) {
        if previous.as_ref() != Some(receipt) {
            return Err(ProviderError::Conflict);
        }
        return Ok(());
    }
    if !matches!(state, MutationState::Dispatched | MutationState::Unknown) {
        return Err(ProviderError::Conflict);
    }
    let json = serde_json::to_string(receipt).map_err(|_| ProviderError::Invalid)?;
    if json.len() > MAX_RECORD {
        return Err(ProviderError::TooLarge);
    }
    db(tx.execute(
        "UPDATE mdreader_provider_mutations SET state='accepted',receipt=?1 WHERE operation_id=?2",
        (&json, receipt.operation_id.to_string()),
    ))?;
    db(tx.commit())
}
/// Read reconciliation never dispatches. A different body is retained as a
/// conflict with the original intended body still durable for explicit review.
pub fn reconcile_read(
    connection: &mut Connection,
    binding: &SourceBinding,
    id: uuid::Uuid,
    observed: &DocumentBody,
) -> Result<()> {
    let pending = load(connection, binding, id)?;
    if observed.key != pending.key {
        return Err(ProviderError::IdentityMismatch);
    }
    if !matches!(
        pending.state,
        MutationState::Dispatched | MutationState::Unknown
    ) {
        return Err(ProviderError::Conflict);
    }
    if observed.body != pending.body {
        let version = serde_json::to_string(&(observed.version.clone(), observed.observed_at_ms))
            .map_err(|_| ProviderError::Invalid)?;
        if version.len() > MAX_RECORD {
            return Err(ProviderError::TooLarge);
        }
        let tx = db(connection.transaction())?;
        let total:u64=db(tx.query_row("SELECT (SELECT coalesce(sum(length(body)+length(record)),0) FROM mdreader_provider_mutations)+(SELECT coalesce(sum(length(body)+length(version)),0) FROM mdreader_provider_conflicts)",[],|r|r.get(0)))?;
        if total.saturating_add((observed.body.as_bytes().len() + version.len()) as u64)
            > 256 * 1024 * 1024
        {
            return Err(ProviderError::TooLarge);
        }
        db(tx.execute(
            "INSERT INTO mdreader_provider_conflicts VALUES(?1,?2,?3,?4)",
            rusqlite::params![
                id.to_string(),
                version,
                observed.body.as_bytes(),
                body_hash(observed.body.as_bytes()).as_slice()
            ],
        ))?;
        if db(tx.execute("UPDATE mdreader_provider_mutations SET state='conflict' WHERE operation_id=?1 AND state IN ('dispatched','unknown')",[id.to_string()]))?!=1{return Err(ProviderError::Conflict);}
        db(tx.commit())?;
        return Err(ProviderError::Conflict);
    }
    accept_receipt(
        connection,
        binding,
        &WriteReceipt {
            operation_id: id,
            key: observed.key.clone(),
            version: observed.version.clone(),
            body_hash: body_hash(observed.body.as_bytes()),
            size_bytes: observed.body.as_bytes().len() as u64,
        },
    )
}
/// Owner calls this only after durable metadata/body admission and index-job
/// commit. Index success alone cannot acknowledge local publication.
pub fn mark_published(
    connection: &mut Connection,
    binding: &SourceBinding,
    id: uuid::Uuid,
) -> Result<()> {
    let pending = load(connection, binding, id)?;
    if pending.state == MutationState::Published {
        return Ok(());
    }
    if pending.state != MutationState::Accepted {
        return Err(ProviderError::Conflict);
    }
    db(connection.execute("UPDATE mdreader_provider_mutations SET state='published' WHERE operation_id=?1 AND state='accepted'",[id.to_string()]))?;
    Ok(())
}
/// Bounded startup/reconciliation inventory, scoped to the exact binding.
pub fn pending_ids(
    connection: &mut Connection,
    binding: &SourceBinding,
) -> Result<Vec<uuid::Uuid>> {
    initialize(connection)?;
    let mut statement=db(connection.prepare("SELECT operation_id FROM mdreader_provider_mutations WHERE binding=?1 AND state!='published' ORDER BY rowid LIMIT 129"))?;
    let rows = db(statement.query_map([binding.digest().as_slice()], |r| r.get::<_, String>(0)))?;
    let rows = db(rows.collect::<std::result::Result<Vec<_>, _>>())?;
    if rows.len() > 128 {
        return Err(ProviderError::Persistence);
    }
    rows.into_iter()
        .map(|id| uuid::Uuid::parse_str(&id).map_err(|_| ProviderError::Persistence))
        .collect()
}
/// Conflicting remote body snapshot, kept separately from the intended body.
pub fn conflict_body(
    connection: &mut Connection,
    binding: &SourceBinding,
    id: uuid::Uuid,
) -> Result<Option<DocumentBody>> {
    let pending = load(connection, binding, id)?;
    if pending.state != MutationState::Conflict {
        return Ok(None);
    }
    let (version, body, digest): (String, Vec<u8>, Vec<u8>) = db(connection.query_row(
        "SELECT version,body,digest FROM mdreader_provider_conflicts WHERE operation_id=?1",
        [id.to_string()],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ))?;
    if version.len() > MAX_RECORD || digest != body_hash(&body) {
        return Err(ProviderError::Persistence);
    }
    let (version, observed_at_ms): (DocumentVersion, i64) =
        serde_json::from_str(&version).map_err(|_| ProviderError::Persistence)?;
    Ok(Some(DocumentBody {
        key: pending.key,
        version,
        body: MarkdownBytes::new(body)?,
        observed_at_ms,
    }))
}
/// Explicit owner-directed retirement of terminal history; uncertain, prepared,
/// dispatched and accepted-but-unpublished intents can never be garbage collected.
pub fn forget_terminal(
    connection: &mut Connection,
    binding: &SourceBinding,
    id: uuid::Uuid,
) -> Result<()> {
    let pending = load(connection, binding, id)?;
    if !matches!(
        pending.state,
        MutationState::Published | MutationState::Conflict
    ) {
        return Err(ProviderError::Conflict);
    }
    let tx = db(connection.transaction())?;
    db(tx.execute(
        "DELETE FROM mdreader_provider_conflicts WHERE operation_id=?1",
        [id.to_string()],
    ))?;
    db(tx.execute(
        "DELETE FROM mdreader_provider_mutations WHERE operation_id=?1",
        [id.to_string()],
    ))?;
    db(tx.commit())
}
