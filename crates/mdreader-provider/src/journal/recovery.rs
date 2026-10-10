//! Local intended-save evidence only. No initialization, migration, credential
//! lookup, remote I/O or dispatch permit is available through this boundary.
use super::*;

pub struct DraftRecord {
    pub binding: SourceBinding,
    pub key: RecordKey,
    pub operation_id: uuid::Uuid,
    pub revision: [u8; 32],
    pub state: MutationState,
    pub byte_length: usize,
}
pub struct DraftPage {
    pub drafts: Vec<DraftRecord>,
    pub next_offset: Option<usize>,
}
fn present(c: &Connection) -> Result<bool> {
    let shadow: bool = db(c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE name IN ('mdreader_provider_meta','mdreader_provider_mutations','mdreader_provider_conflicts'))", [], |r| r.get(0)))?;
    if shadow {
        return Err(ProviderError::Persistence);
    }
    let count: u64 = db(c.query_row("SELECT count(*) FROM main.sqlite_schema WHERE type='table' AND name IN ('mdreader_provider_meta','mdreader_provider_mutations','mdreader_provider_conflicts')", [], |r| r.get(0)))?;
    if count == 0 {
        return Ok(false);
    }
    let version: u64 = db(c.query_row(
        "SELECT version FROM main.mdreader_provider_meta WHERE id=1",
        [],
        |r| r.get(0),
    ))?;
    if !matches!((count, version), (2, 1) | (3, 2)) {
        return Err(ProviderError::Persistence);
    }
    Ok(true)
}
fn evidence(c: &Connection, id: uuid::Uuid) -> Result<DraftRecord> {
    type Row = (String, Vec<u8>, Vec<u8>, String, usize);
    let (text,digest,binding,state,bytes):Row = db(c.query_row("SELECT CASE WHEN length(record)<=32768 THEN record END,digest,binding,state,length(body) FROM main.mdreader_provider_mutations WHERE operation_id=?1", [id.to_string()], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))))?;
    if bytes > MAX_BODY_BYTES {
        return Err(ProviderError::TooLarge);
    }
    if text.len() > MAX_RECORD || digest != body_hash(text.as_bytes()) {
        return Err(ProviderError::Persistence);
    }
    let intent: Intent = serde_json::from_str(&text).map_err(|_| ProviderError::Persistence)?;
    if intent.v != 1 || intent.operation_id != id || binding != intent.binding.digest() {
        return Err(ProviderError::Persistence);
    }
    Ok(DraftRecord {
        binding: intent.binding,
        key: intent.key,
        operation_id: id,
        revision: intent.body_hash,
        state: MutationState::parse(&state)?,
        byte_length: bytes,
    })
}
/// Bounded metadata only. A body is verified on the explicit exact read below.
/// Pagination does not claim a live provider census or restore its authority.
pub fn draft_page(
    c: &Connection,
    library: &LibraryId,
    offset: usize,
    limit: usize,
) -> Result<DraftPage> {
    if offset > 1000 || !(1..=32).contains(&limit) {
        return Err(ProviderError::Invalid);
    }
    if !present(c)? {
        return Ok(DraftPage {
            drafts: vec![],
            next_offset: None,
        });
    }
    let ids: Vec<String> = {
        let mut q = db(c.prepare("SELECT operation_id FROM main.mdreader_provider_mutations ORDER BY rowid DESC LIMIT 1001"))?;
        let rows = db(q.query_map([], |r| r.get(0)))?;
        db(rows.collect::<std::result::Result<Vec<_>, _>>())?
    };
    if ids.len() > 1000 {
        return Err(ProviderError::TooLarge);
    }
    let mut matching = Vec::new();
    for id in ids {
        let id = uuid::Uuid::parse_str(&id).map_err(|_| ProviderError::Persistence)?;
        let draft = evidence(c, id)?;
        if draft.binding.library_id() == library {
            matching.push(draft);
        }
    }
    let next = offset.saturating_add(limit).min(matching.len());
    let next_offset = (next < matching.len()).then_some(next);
    Ok(DraftPage {
        drafts: matching.into_iter().skip(offset).take(limit).collect(),
        next_offset,
    })
}
/// Reads the exact retained intended bytes, regardless of current cloud grant.
/// Library identity and immutable body digest fence stale/foreign requests.
pub fn read_draft(
    c: &Connection,
    library: &LibraryId,
    id: uuid::Uuid,
    revision: [u8; 32],
) -> Result<(DraftRecord, MarkdownBytes)> {
    if !present(c)? {
        return Err(ProviderError::NotFound);
    }
    let draft = evidence(c, id)?;
    if draft.binding.library_id() != library {
        return Err(ProviderError::Unauthorized);
    }
    if draft.revision != revision {
        return Err(ProviderError::Conflict);
    }
    let pending = inspection::inspect(c, &draft.binding, id)?.ok_or(ProviderError::NotFound)?;
    if body_hash(pending.body.as_bytes()) != revision
        || pending.body.as_bytes().len() != draft.byte_length
        || pending.state != draft.state
    {
        return Err(ProviderError::Persistence);
    }
    Ok((draft, pending.body))
}
