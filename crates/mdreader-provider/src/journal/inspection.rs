//! Readonly exact-operation reconciliation. Never initializes or upgrades the
//! durable namespace, and never mistakes another binding's row for absence.
use super::*;

pub fn inspect(
    connection: &Connection,
    binding: &SourceBinding,
    id: uuid::Uuid,
) -> Result<Option<PendingMutation>> {
    let tables: u64 = db(connection.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('mdreader_provider_meta','mdreader_provider_mutations','mdreader_provider_conflicts')",
        [], |row| row.get(0),
    ))?;
    if tables == 0 {
        return Ok(None);
    }
    if !matches!(tables, 2 | 3) {
        return Err(ProviderError::Persistence);
    }
    let version: u64 = db(connection.query_row(
        "SELECT version FROM mdreader_provider_meta WHERE id=1",
        [],
        |row| row.get(0),
    ))?;
    if !matches!((tables, version), (2, 1) | (3, 2)) {
        return Err(ProviderError::Persistence);
    }
    let observed: Option<Vec<u8>> = db(connection
        .query_row(
            "SELECT CASE WHEN length(binding)=32 THEN binding END FROM mdreader_provider_mutations WHERE operation_id=?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .optional())?;
    let Some(observed) = observed else {
        return Ok(None);
    };
    if observed != binding.digest() {
        return Err(ProviderError::Unauthorized);
    }
    let (intent, body, state, receipt) = record(connection, binding, id)?;
    Ok(Some(PendingMutation {
        operation_id: id,
        key: intent.key,
        expected: intent.expected,
        body,
        state,
        receipt,
    }))
}

/// Non-replayable offline evidence from the actual closed owner's journal.
/// No initialization, upgrades, credentials, provider effects or dispatch permit.
pub struct ArchivedMutation {
    pub binding: SourceBinding,
    pub pending: PendingMutation,
    pub conflict: Option<DocumentBody>,
}
pub fn archive(connection: &Connection) -> Result<Vec<ArchivedMutation>> {
    let tables: u64 = db(connection.query_row("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name IN ('mdreader_provider_meta','mdreader_provider_mutations','mdreader_provider_conflicts')", [], |r| r.get(0)))?;
    if tables == 0 {
        return Ok(vec![]);
    }
    if !matches!(tables, 2 | 3) {
        return Err(ProviderError::Persistence);
    }
    let version: u64 = db(connection.query_row(
        "SELECT version FROM mdreader_provider_meta WHERE id=1",
        [],
        |r| r.get(0),
    ))?;
    if !matches!((tables, version), (2, 1) | (3, 2)) {
        return Err(ProviderError::Persistence);
    }
    let invalid:bool=db(connection.query_row("SELECT EXISTS(SELECT 1 FROM mdreader_provider_mutations WHERE length(binding)!=32 OR length(digest)!=32)",[],|r|r.get(0)))?;
    if invalid {
        return Err(ProviderError::Persistence);
    }
    let mutation_bytes: u64 = db(connection.query_row(
        "SELECT coalesce(sum(length(body)+length(record)),0) FROM mdreader_provider_mutations",
        [],
        |r| r.get(0),
    ))?;
    let conflict_bytes: u64 = if tables == 3 {
        db(connection.query_row(
            "SELECT coalesce(sum(length(body)+length(version)),0) FROM mdreader_provider_conflicts",
            [],
            |r| r.get(0),
        ))?
    } else {
        0
    };
    if mutation_bytes.saturating_add(conflict_bytes) > 64 * 1024 * 1024 {
        return Err(ProviderError::TooLarge);
    }
    let mut query = db(connection.prepare("SELECT operation_id,CASE WHEN length(record)<=32768 THEN record END,length(body) FROM mdreader_provider_mutations ORDER BY operation_id LIMIT 1001"))?;
    let rows = db(query.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, usize>(2)?,
        ))
    }))?;
    let rows = db(rows.collect::<std::result::Result<Vec<_>, _>>())?;
    if rows.len() > 1000 {
        return Err(ProviderError::TooLarge);
    }
    let mut result = Vec::new();
    for (id, record, bytes) in rows {
        if bytes > MAX_BODY_BYTES {
            return Err(ProviderError::TooLarge);
        }
        let id = uuid::Uuid::parse_str(&id).map_err(|_| ProviderError::Persistence)?;
        let intent: Intent =
            serde_json::from_str(&record).map_err(|_| ProviderError::Persistence)?;
        let pending =
            inspect(connection, &intent.binding, id)?.ok_or(ProviderError::Persistence)?;
        let conflict = if pending.state == MutationState::Conflict {
            let (version,body,digest):(String,Vec<u8>,Vec<u8>)=db(connection.query_row("SELECT CASE WHEN length(version)<=32768 THEN version END,CASE WHEN length(body)<=2097152 THEN body END,CASE WHEN length(digest)=32 THEN digest END FROM mdreader_provider_conflicts WHERE operation_id=?1",[id.to_string()], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))))?;
            if digest != body_hash(&body) {
                return Err(ProviderError::Persistence);
            }
            let (version, observed_at_ms): (DocumentVersion, i64) =
                serde_json::from_str(&version).map_err(|_| ProviderError::Persistence)?;
            Some(DocumentBody {
                key: pending.key.clone(),
                version,
                body: MarkdownBytes::new(body)?,
                observed_at_ms,
            })
        } else {
            None
        };
        result.push(ArchivedMutation {
            binding: intent.binding,
            pending,
            conflict,
        });
    }
    Ok(result)
}
