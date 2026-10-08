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
            "SELECT binding FROM mdreader_provider_mutations WHERE operation_id=?1",
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
