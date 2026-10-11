//! Private durable editor checkpoints. Recovery data confers no source grant,
//! never touches originals or the rebuildable index, and never resends writes.
mod schema;
mod types;
use crate::{ReaderError, ReaderStore, Result};
use rusqlite::{Connection, OptionalExtension};
use std::time::{SystemTime, UNIX_EPOCH};
pub use types::*;
use types::{MAX_GENERATION, MAX_SLOTS};

fn invalid() -> ReaderError {
    ReaderError::Invalid("editor draft evidence is unavailable or exceeds bounds".into())
}
fn id(profile: uuid::Uuid, key: &DraftKey) -> Result<(String, String)> {
    key.validate()?;
    let json = serde_json::to_string(key).map_err(|_| invalid())?;
    Ok((
        uuid::Uuid::new_v5(&profile, json.as_bytes()).to_string(),
        json,
    ))
}
fn checked_id(raw: &str) -> Result<()> {
    let parsed = uuid::Uuid::parse_str(raw).map_err(|_| invalid())?;
    if parsed.to_string() != raw || parsed.get_version_num() != 5 {
        return Err(invalid());
    }
    Ok(())
}
fn generation(value: u64) -> Result<()> {
    if value > MAX_GENERATION {
        Err(invalid())
    } else {
        Ok(())
    }
}
fn increment(value: u64) -> Result<u64> {
    generation(value)?;
    value
        .checked_add(1)
        .filter(|v| *v <= MAX_GENERATION)
        .ok_or_else(invalid)
}
fn ensure_slot(db: &Connection, draft_id: &str, key: &str) -> Result<()> {
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.reader_editor_drafts_v1 WHERE draft_id=?1)",
        [draft_id],
        |r| r.get(0),
    )?;
    if !exists {
        let count: u64 = db.query_row(
            "SELECT count(*) FROM main.reader_editor_drafts_v1",
            [],
            |r| r.get(0),
        )?;
        if count >= MAX_SLOTS {
            return Err(invalid());
        }
        db.execute("INSERT INTO main.reader_editor_drafts_v1(draft_id,key_json,generation) VALUES(?1,?2,0)", (draft_id,key))?;
    }
    Ok(())
}
fn metadata(
    profile: uuid::Uuid,
    raw: &str,
    draft_id: &str,
    key: &str,
    gen: u64,
) -> Result<DraftMetadata> {
    if raw.len() > 16 * 1024 {
        return Err(invalid());
    }
    let entry: DraftMetadata = serde_json::from_str(raw).map_err(|_| invalid())?;
    let (expected_id, expected_key) = id(profile, &entry.key)?;
    generation(gen)?;
    DraftInput {
        base_revision: entry.base_revision.clone(),
        base_hash: entry.base_hash.clone(),
        title: entry.title.clone(),
        markdown: String::new(),
    }
    .validate()?;
    if entry.draft_id != draft_id
        || expected_id != draft_id
        || expected_key != key
        || entry.generation != gen
        || gen == 0
        || entry.bytes > MAX_DRAFT_BYTES as u64
        || entry.modified_ms > MAX_GENERATION
        || entry.draft_hash.len() != 64
        || !entry.draft_hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid());
    }
    Ok(entry)
}
fn slot(db: &Connection, profile: uuid::Uuid, draft_id: &str) -> Result<DraftSlot> {
    let (key, gen, raw): (String,u64,Option<String>) = db.query_row("SELECT key_json,generation,metadata FROM main.reader_editor_drafts_v1 WHERE draft_id=?1", [draft_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?.ok_or(ReaderError::Unavailable)?;
    generation(gen)?;
    let locator: DraftKey = serde_json::from_str(&key).map_err(|_| invalid())?;
    if id(profile, &locator)? != (draft_id.to_owned(), key.clone()) {
        return Err(invalid());
    }
    let draft = raw
        .map(|raw| metadata(profile, &raw, draft_id, &key, gen))
        .transpose()?;
    Ok(DraftSlot {
        draft_id: draft_id.to_owned(),
        generation: gen,
        draft,
    })
}
impl ReaderStore {
    /// All operations borrow this exact durable library owner. The service
    /// additionally takes its normal preservation/freeze admission lease.
    pub fn editor_drafts(&mut self, request: DraftRequest) -> Result<DraftReply> {
        let owner = self.library_identity()?;
        owner.verify()?;
        self.with_library_connection(|db| schema::initialize(db))?;
        let profile = self.profile_uuid()?;
        self.with_library_connection(|db| {
        let tx = db.transaction()?;
        let reply = match request {
            DraftRequest::Inspect { key } => {
                let (draft_id, json) = id(profile, &key)?;
                ensure_slot(&tx, &draft_id, &json)?;
                DraftReply::Slot {
                    slot: slot(&tx, profile, &draft_id)?,
                }
            }
            DraftRequest::Checkpoint {
                key,
                expected_generation,
                draft,
            } => {
                generation(expected_generation)?;
                draft.validate()?;
                let (draft_id, json) = id(profile, &key)?;
                ensure_slot(&tx, &draft_id, &json)?;
                let previous = slot(&tx, profile, &draft_id)?;
                if previous.generation != expected_generation {
                    return Err(ReaderError::Conflict);
                }
                if previous.draft.is_none() {
                    let active: u64 = tx.query_row(
                        "SELECT count(*) FROM main.reader_editor_drafts_v1 WHERE body IS NOT NULL",
                        [],
                        |r| r.get(0),
                    )?;
                    if active >= MAX_ACTIVE_DRAFTS {
                        return Err(invalid());
                    }
                }
                let gen = increment(expected_generation)?;
                let modified_ms = u64::try_from(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|_| invalid())?
                        .as_millis(),
                )
                .map_err(|_| invalid())?;
                generation(modified_ms)?;
                let entry = DraftMetadata {
                    draft_id: draft_id.clone(),
                    key,
                    generation: gen,
                    base_revision: draft.base_revision,
                    base_hash: draft.base_hash,
                    title: draft.title,
                    draft_hash: blake3::hash(draft.markdown.as_bytes()).to_hex().to_string(),
                    bytes: draft.markdown.len() as u64,
                    modified_ms,
                };
                let raw = serde_json::to_string(&entry).map_err(|_| invalid())?;
                let changed = tx.execute("UPDATE main.reader_editor_drafts_v1 SET generation=?3,body=?4,metadata=?5 WHERE draft_id=?1 AND generation=?2 AND key_json=?6", (&draft_id,expected_generation,gen,&draft.markdown,raw,json))?;
                if changed != 1 {
                    return Err(ReaderError::Conflict);
                }
                DraftReply::Slot {
                    slot: DraftSlot {
                        draft_id,
                        generation: gen,
                        draft: Some(entry),
                    },
                }
            }
            DraftRequest::Discard {
                draft_id,
                expected_generation,
            } => {
                checked_id(&draft_id)?;
                let gen = increment(expected_generation)?;
                // Retain the row as a suppression tombstone. No time-based purge
                // or eviction can let a delayed older checkpoint recreate it.
                let changed = tx.execute("UPDATE main.reader_editor_drafts_v1 SET generation=?3,body=NULL,metadata=NULL WHERE draft_id=?1 AND generation=?2", (&draft_id,expected_generation,gen))?;
                if changed != 1 {
                    return Err(ReaderError::Conflict);
                }
                DraftReply::Slot {
                    slot: slot(&tx, profile, &draft_id)?,
                }
            }
            DraftRequest::List => {
                let mut query = tx.prepare("SELECT draft_id,key_json,generation,metadata FROM main.reader_editor_drafts_v1 WHERE body IS NOT NULL ORDER BY draft_id LIMIT 65")?;
                let rows = query.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, u64>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })?;
                let mut drafts = Vec::new();
                for row in rows {
                    let (draft_id, key, gen, raw) = row?;
                    drafts.push(metadata(profile, &raw, &draft_id, &key, gen)?);
                }
                if drafts.len() > MAX_ACTIVE_DRAFTS as usize {
                    return Err(invalid());
                }
                drafts.sort_by(|a, b| {
                    b.modified_ms
                        .cmp(&a.modified_ms)
                        .then_with(|| a.draft_id.cmp(&b.draft_id))
                });
                DraftReply::Listed { drafts }
            }
            DraftRequest::Read {
                draft_id,
                generation: gen,
            } => {
                checked_id(&draft_id)?;
                generation(gen)?;
                let current = slot(&tx, profile, &draft_id)?;
                if current.generation != gen {
                    return Err(ReaderError::Conflict);
                }
                let entry = current.draft.ok_or(ReaderError::Unavailable)?;
                let body: String = tx.query_row("SELECT body FROM main.reader_editor_drafts_v1 WHERE draft_id=?1 AND length(CAST(body AS BLOB))<=?2", (&draft_id, MAX_DRAFT_BYTES as u64), |r| r.get(0)).optional()?.ok_or(ReaderError::Unavailable)?;
                if body.len() as u64 != entry.bytes
                    || blake3::hash(body.as_bytes()).to_hex().as_str() != entry.draft_hash
                {
                    return Err(ReaderError::Conflict);
                }
                DraftReply::Read {
                    draft: entry,
                    markdown: body,
                }
            }
        };
        owner.verify()?;
        schema::check(&tx)?;
        tx.commit()?;
        Ok(reply)
        })
    }
}
