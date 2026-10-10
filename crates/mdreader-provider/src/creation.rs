//! Create-only intent custody. An uncertain dispatch is never replayed.
use crate::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    operation_id: uuid::Uuid,
    binding: SourceBinding,
    filename: String,
    body: MarkdownBytes,
}
pub struct CreatePermit {
    operation_id: uuid::Uuid,
    binding: SourceBinding,
    filename: String,
    body_hash: [u8; 32],
}
impl CreatePermit {
    pub fn operation_id(&self) -> uuid::Uuid {
        self.operation_id
    }
    pub fn binding(&self) -> &SourceBinding {
        &self.binding
    }
    pub fn filename(&self) -> &str {
        &self.filename
    }
    pub fn body_hash(&self) -> [u8; 32] {
        self.body_hash
    }
    pub fn validate(&self, binding: &SourceBinding, body: &MarkdownBytes) -> Result<()> {
        if binding != &self.binding || body_hash(body.as_bytes()) != self.body_hash {
            return Err(ProviderError::Unauthorized);
        }
        validate_filename(&self.filename)
    }
}
pub fn validate_filename(filename: &str) -> Result<()> {
    let lower = filename.to_ascii_lowercase();
    if filename.is_empty()
        || filename.len() > 240
        || filename.trim() != filename
        || filename
            .chars()
            .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
        || filename == ".md"
        || filename == ".markdown"
        || !(lower.ends_with(".md") || lower.ends_with(".markdown"))
    {
        return Err(ProviderError::Invalid);
    }
    Ok(())
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryDiagnostic {
    NoMatchingOperation,
    AmbiguousOperation,
    MetadataMismatch,
    BodyMismatch,
    MissingWriteValidator,
}
pub enum RecoveryObservation {
    Confirmed(Box<Metadata>),
    Unresolved(RecoveryDiagnostic),
}
pub struct PendingCreate {
    pub filename: String,
    pub body: MarkdownBytes,
    pub state: journal::MutationState,
    pub metadata: Option<Metadata>,
    pub projection: Option<String>,
}
fn db<T>(value: std::result::Result<T, rusqlite::Error>) -> Result<T> {
    value.map_err(|_| ProviderError::Persistence)
}
pub fn initialize(c: &mut Connection) -> Result<()> {
    if db(c.query_row("PRAGMA synchronous", [], |r| r.get::<_, u8>(0)))? < 2 {
        return Err(ProviderError::Persistence);
    }
    let tx = db(c.transaction())?;
    let schema = "CREATE TABLE mdreader_provider_creates_v1(operation_id TEXT PRIMARY KEY,binding BLOB NOT NULL,intent TEXT NOT NULL,digest BLOB NOT NULL,state TEXT NOT NULL CHECK(state IN ('prepared','dispatched','unknown','accepted','published','conflict')),metadata TEXT,projection TEXT)";
    let existing:Option<String> = db(tx.query_row("SELECT sql FROM sqlite_schema WHERE type='table' AND name='mdreader_provider_creates_v1'",[],|r|r.get(0)).optional())?;
    if let Some(existing) = existing {
        let normalize = |value: &str| {
            value
                .chars()
                .filter(|c| !c.is_whitespace())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        };
        if normalize(&existing) != normalize(schema) {
            return Err(ProviderError::Unsupported);
        }
    } else {
        db(tx.execute_batch(schema))?;
    }
    db(tx.prepare("SELECT operation_id,binding,intent,digest,state,metadata,projection FROM mdreader_provider_creates_v1 LIMIT 0"))?;
    db(tx.commit())
}
/// Called once when the owner starts, never by inspection during live dispatch.
pub fn recover_interrupted(c: &Connection, binding: &SourceBinding) -> Result<()> {
    db(c.execute("UPDATE mdreader_provider_creates_v1 SET state='unknown' WHERE binding=?1 AND state='dispatched'",[binding.digest().as_slice()]))?;
    Ok(())
}
pub fn load(
    c: &Connection,
    binding: &SourceBinding,
    id: uuid::Uuid,
) -> Result<Option<PendingCreate>> {
    type Row = (String, Vec<u8>, String, Option<String>, Option<String>);
    let row: Option<Row> = db(c.query_row("SELECT intent,digest,state,metadata,projection FROM mdreader_provider_creates_v1 WHERE operation_id=?1 AND binding=?2", params![id.to_string(),binding.digest().as_slice()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional())?;
    let Some((text, digest, state, metadata, projection)) = row else {
        return Ok(None);
    };
    if text.len() > MAX_BODY_BYTES * 5 + 16384 || digest != body_hash(text.as_bytes()) {
        return Err(ProviderError::Persistence);
    }
    let intent: Intent = serde_json::from_str(&text).map_err(|_| ProviderError::Persistence)?;
    if intent.operation_id != id || intent.binding != *binding {
        return Err(ProviderError::Persistence);
    }
    validate_filename(&intent.filename)?;
    let state = match state.as_str() {
        "prepared" => journal::MutationState::Prepared,
        "dispatched" => journal::MutationState::Dispatched,
        "unknown" => journal::MutationState::Unknown,
        "accepted" => journal::MutationState::Accepted,
        "published" => journal::MutationState::Published,
        "conflict" => journal::MutationState::Conflict,
        _ => return Err(ProviderError::Persistence),
    };
    let metadata: Option<Metadata> = metadata
        .map(|text| {
            if text.len() > 32 * 1024 {
                return Err(ProviderError::Persistence);
            }
            serde_json::from_str(&text).map_err(|_| ProviderError::Persistence)
        })
        .transpose()?;
    if matches!(
        state,
        journal::MutationState::Accepted | journal::MutationState::Published
    ) != metadata.is_some()
    {
        return Err(ProviderError::Persistence);
    }
    if let Some(metadata) = &metadata {
        if metadata.name.as_str() != intent.filename
            || metadata.size_bytes != Some(intent.body.as_bytes().len() as u64)
        {
            return Err(ProviderError::Persistence);
        }
        if let Some(hash) = &metadata.version.content_hash {
            verify_content_hash(intent.body.as_bytes(), hash)
                .map_err(|_| ProviderError::Persistence)?;
        }
    }
    if (state == journal::MutationState::Published) != projection.is_some()
        || projection.as_ref().is_some_and(|p| p.len() > 16384)
    {
        return Err(ProviderError::Persistence);
    }
    Ok(Some(PendingCreate {
        filename: intent.filename,
        body: intent.body,
        state,
        metadata,
        projection,
    }))
}
pub fn prepare(
    c: &mut Connection,
    binding: &SourceBinding,
    id: uuid::Uuid,
    filename: &str,
    body: &MarkdownBytes,
) -> Result<()> {
    if id.get_version_num() != 4 {
        return Err(ProviderError::Invalid);
    }
    validate_filename(filename)?;
    initialize(c)?;
    if let Some(prior) = load(c, binding, id)? {
        return if prior.filename == filename && prior.body == *body {
            Ok(())
        } else {
            Err(ProviderError::Conflict)
        };
    }
    let unfinished:Option<String> = db(c.query_row("SELECT operation_id FROM mdreader_provider_creates_v1 WHERE binding=?1 AND state IN ('prepared','dispatched','unknown','accepted') AND lower(json_extract(intent,'$.filename'))=lower(?2) LIMIT 1",params![binding.digest().as_slice(),filename],|r|r.get(0)).optional())?;
    if let Some(unfinished) = unfinished {
        let operation_id =
            uuid::Uuid::parse_str(&unfinished).map_err(|_| ProviderError::Persistence)?;
        return Err(ProviderError::UnknownWrite { operation_id });
    }
    let count: u64 = db(c.query_row(
        "SELECT count(*) FROM mdreader_provider_creates_v1 WHERE state!='published'",
        [],
        |r| r.get(0),
    ))?;
    let (total, bytes): (u64, u64) = db(c.query_row(
        "SELECT count(*),coalesce(sum(length(intent)),0) FROM mdreader_provider_creates_v1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ))?;
    if count >= 256
        || total >= 4096
        || bytes.saturating_add(body.as_bytes().len() as u64 * 5 + 16384) > 256 * 1024 * 1024
    {
        return Err(ProviderError::TooLarge);
    }
    let text = serde_json::to_string(&Intent {
        operation_id: id,
        binding: binding.clone(),
        filename: filename.into(),
        body: body.clone(),
    })
    .map_err(|_| ProviderError::Protocol)?;
    db(c.execute(
        "INSERT INTO mdreader_provider_creates_v1 VALUES(?1,?2,?3,?4,'prepared',NULL,NULL)",
        params![
            id.to_string(),
            binding.digest().as_slice(),
            text,
            body_hash(text.as_bytes()).as_slice()
        ],
    ))?;
    Ok(())
}
pub fn dispatch(
    c: &mut Connection,
    binding: &SourceBinding,
    id: uuid::Uuid,
) -> Result<CreatePermit> {
    let pending = load(c, binding, id)?.ok_or(ProviderError::Unauthorized)?;
    if pending.state != journal::MutationState::Prepared {
        return Err(ProviderError::UnknownWrite { operation_id: id });
    }
    let changed = db(c.execute("UPDATE mdreader_provider_creates_v1 SET state='dispatched' WHERE operation_id=?1 AND binding=?2 AND state='prepared'",params![id.to_string(),binding.digest().as_slice()]))?;
    if changed != 1 {
        return Err(ProviderError::Conflict);
    }
    Ok(CreatePermit {
        operation_id: id,
        binding: binding.clone(),
        filename: pending.filename,
        body_hash: body_hash(pending.body.as_bytes()),
    })
}
pub fn uncertain(c: &Connection, binding: &SourceBinding, id: uuid::Uuid) -> Result<()> {
    db(c.execute("UPDATE mdreader_provider_creates_v1 SET state='unknown' WHERE operation_id=?1 AND binding=?2 AND state='dispatched'",params![id.to_string(),binding.digest().as_slice()]))?;
    Ok(())
}
pub fn reject(c: &Connection, binding: &SourceBinding, id: uuid::Uuid) -> Result<()> {
    db(c.execute("UPDATE mdreader_provider_creates_v1 SET state='conflict' WHERE operation_id=?1 AND binding=?2 AND state='dispatched'",params![id.to_string(),binding.digest().as_slice()]))?;
    Ok(())
}
pub fn accept(
    c: &Connection,
    binding: &SourceBinding,
    id: uuid::Uuid,
    metadata: &Metadata,
) -> Result<()> {
    let pending = load(c, binding, id)?.ok_or(ProviderError::Unauthorized)?;
    if pending.state != journal::MutationState::Dispatched {
        return Err(ProviderError::Conflict);
    }
    if metadata.name.as_str() != pending.filename
        || metadata.size_bytes != Some(pending.body.as_bytes().len() as u64)
    {
        return Err(ProviderError::Protocol);
    }
    let text = serde_json::to_string(metadata).map_err(|_| ProviderError::Protocol)?;
    if text.len() > 32 * 1024 {
        return Err(ProviderError::TooLarge);
    }
    db(c.execute("UPDATE mdreader_provider_creates_v1 SET state='accepted',metadata=?3 WHERE operation_id=?1 AND binding=?2 AND state='dispatched'",params![id.to_string(),binding.digest().as_slice(),text]))?;
    Ok(())
}
pub fn published(
    c: &Connection,
    binding: &SourceBinding,
    id: uuid::Uuid,
    projection: &str,
) -> Result<()> {
    if projection.len() > 16384 || serde_json::from_str::<serde_json::Value>(projection).is_err() {
        return Err(ProviderError::Invalid);
    }
    let count = db(c.execute("UPDATE mdreader_provider_creates_v1 SET state='published',projection=?3 WHERE operation_id=?1 AND binding=?2 AND state='accepted'",params![id.to_string(),binding.digest().as_slice(),projection]))?;
    if count != 1 {
        return Err(ProviderError::Conflict);
    }
    Ok(())
}

/// Native read-only reconciliation of an original uncertain intent. No new
/// dispatch permit is issued; exact binding/intent and state CAS remain durable.
pub fn accept_recovered(
    c: &Connection,
    binding: &SourceBinding,
    id: uuid::Uuid,
    metadata: &Metadata,
) -> Result<()> {
    let pending = load(c, binding, id)?.ok_or(ProviderError::Unauthorized)?;
    if !matches!(
        pending.state,
        journal::MutationState::Dispatched | journal::MutationState::Unknown
    ) {
        return Err(ProviderError::Conflict);
    }
    if metadata.name.as_str() != pending.filename
        || metadata.size_bytes != Some(pending.body.as_bytes().len() as u64)
    {
        return Err(ProviderError::Protocol);
    }
    if let Some(hash) = &metadata.version.content_hash {
        verify_content_hash(pending.body.as_bytes(), hash)?;
    }
    let text = serde_json::to_string(metadata).map_err(|_| ProviderError::Protocol)?;
    if text.len() > 32 * 1024 {
        return Err(ProviderError::TooLarge);
    }
    let count = db(c.execute("UPDATE mdreader_provider_creates_v1 SET state='accepted',metadata=?3 WHERE operation_id=?1 AND binding=?2 AND state IN ('dispatched','unknown')",params![id.to_string(),binding.digest().as_slice(),text]))?;
    if count != 1 {
        return Err(ProviderError::Conflict);
    }
    Ok(())
}
