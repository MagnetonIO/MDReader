//! Bounded disposable remote-page observations, in the existing library owner.
//! Slots are exact grant/kind bound; no token or provider body enters a UI DTO.
use super::*;
use rusqlite::{OptionalExtension, params};
const SCHEMA: &str = "CREATE TABLE mdreader_provider_page_spool_v1(binding BLOB NOT NULL CHECK(length(binding)=32),kind TEXT NOT NULL CHECK(kind IN ('list','changes')),slot TEXT NOT NULL,digest BLOB NOT NULL CHECK(length(digest)=32),bytes BLOB NOT NULL CHECK(length(bytes)>0 AND length(bytes)<=8388608),PRIMARY KEY(binding,kind))";
const TOTAL_BYTES: u64 = 64 * 1024 * 1024;
fn kind(kind: CursorKind) -> Result<&'static str> {
    match kind {
        CursorKind::List => Ok("list"),
        CursorKind::Changes => Ok("changes"),
        _ => Err(ProviderError::Invalid),
    }
}
fn validate_schema(c: &Connection, create: bool) -> Result<()> {
    let shadow: u64 = registry::db(c.query_row("SELECT count(*) FROM temp.sqlite_schema WHERE name='mdreader_provider_page_spool_v1' OR tbl_name='mdreader_provider_page_spool_v1'", [], |r| r.get(0)))?;
    if shadow != 0 {
        return Err(ProviderError::Persistence);
    }
    let schema: Option<String> = registry::db(c.query_row("SELECT sql FROM main.sqlite_schema WHERE type='table' AND name='mdreader_provider_page_spool_v1'", [], |r| r.get(0)).optional())?;
    match schema {
        None if create => registry::db(c.execute_batch(&SCHEMA.replacen(
            "CREATE TABLE ",
            "CREATE TABLE main.",
            1,
        )))?,
        None => return Err(ProviderError::ResetCursor),
        Some(schema) => {
            let normalize = |s: &str| {
                s.chars()
                    .filter(|c| !c.is_whitespace())
                    .flat_map(char::to_lowercase)
                    .collect::<String>()
            };
            if normalize(&schema) != normalize(SCHEMA) {
                return Err(ProviderError::Persistence);
            }
        }
    }
    let foreign: u64 = registry::db(c.query_row("SELECT count(*) FROM main.sqlite_schema WHERE tbl_name='mdreader_provider_page_spool_v1' AND type IN ('trigger','index') AND sql IS NOT NULL", [], |r| r.get(0)))?;
    if foreign != 0 {
        return Err(ProviderError::Persistence);
    }
    Ok(())
}
fn admit(c: &Connection, binding: &SourceBinding) -> Result<()> {
    let source = registry::descriptor_at(c, binding, true)?;
    if source.kind() != ProviderKind::Dropbox {
        return Err(ProviderError::Unauthorized);
    }
    Ok(())
}
pub(super) fn store(
    c: &mut Connection,
    binding: &SourceBinding,
    cursor_kind: CursorKind,
    slot: Uuid,
    digest: [u8; 32],
    bytes: &[u8],
) -> Result<()> {
    if bytes.is_empty() || bytes.len() > MAX_PAGE_SPOOL_BYTES {
        return Err(ProviderError::TooLarge);
    }
    if body_hash(bytes) != digest {
        return Err(ProviderError::Invalid);
    }
    let kind = kind(cursor_kind)?;
    let tx = registry::db(c.transaction())?;
    admit(&tx, binding)?;
    validate_schema(&tx, true)?;
    let prior: Option<(String, Vec<u8>)> = registry::db(
        tx.query_row(
            "SELECT slot,digest FROM main.mdreader_provider_page_spool_v1 WHERE binding=?1 AND kind=?2",
            params![binding.digest().as_slice(), kind],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional(),
    )?;
    if prior.is_some_and(|(id, hash)| id == slot.to_string() && hash != digest) {
        return Err(ProviderError::Conflict);
    }
    // Retired bindings cannot own a resumable traversal. Paused bindings retain it.
    registry::db(tx.execute("DELETE FROM main.mdreader_provider_page_spool_v1 WHERE binding NOT IN (SELECT binding FROM main.mdreader_provider_registry_sources)", []))?;
    let retained: u64 = registry::db(tx.query_row("SELECT coalesce(sum(length(bytes)),0) FROM main.mdreader_provider_page_spool_v1 WHERE NOT(binding=?1 AND kind=?2)", params![binding.digest().as_slice(),kind], |r| r.get(0)))?;
    if retained.saturating_add(bytes.len() as u64) > TOTAL_BYTES {
        return Err(ProviderError::TooLarge);
    }
    registry::db(tx.execute("INSERT INTO main.mdreader_provider_page_spool_v1(binding,kind,slot,digest,bytes) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(binding,kind) DO UPDATE SET slot=excluded.slot,digest=excluded.digest,bytes=excluded.bytes", params![binding.digest().as_slice(),kind,slot.to_string(),digest.as_slice(),bytes]))?;
    registry::db(tx.commit())
}
pub(super) fn load(
    c: &mut Connection,
    binding: &SourceBinding,
    cursor_kind: CursorKind,
    slot: Uuid,
    expected: [u8; 32],
) -> Result<Vec<u8>> {
    admit(c, binding)?;
    validate_schema(c, false)?;
    let value: Option<(Vec<u8>,Vec<u8>)> = registry::db(c.query_row("SELECT digest,bytes FROM main.mdreader_provider_page_spool_v1 WHERE binding=?1 AND kind=?2 AND slot=?3", params![binding.digest().as_slice(),kind(cursor_kind)?,slot.to_string()], |r| Ok((r.get(0)?,r.get(1)?))).optional())?;
    let (digest, bytes) = value.ok_or(ProviderError::ResetCursor)?;
    if digest != expected {
        return Err(ProviderError::ResetCursor);
    }
    if bytes.is_empty() || bytes.len() > MAX_PAGE_SPOOL_BYTES || body_hash(&bytes) != expected {
        return Err(ProviderError::Persistence);
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "page_spool_tests.rs"]
mod tests;
