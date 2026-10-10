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
    let schema: Option<String> = registry::db(c.query_row("SELECT sql FROM sqlite_schema WHERE type='table' AND name='mdreader_provider_page_spool_v1'", [], |r| r.get(0)).optional())?;
    match schema {
        None if create => registry::db(c.execute_batch(SCHEMA))?,
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
    let foreign: u64 = registry::db(c.query_row("SELECT count(*) FROM sqlite_schema WHERE tbl_name='mdreader_provider_page_spool_v1' AND type IN ('trigger','index') AND sql IS NOT NULL", [], |r| r.get(0)))?;
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
            "SELECT slot,digest FROM mdreader_provider_page_spool_v1 WHERE binding=?1 AND kind=?2",
            params![binding.digest().as_slice(), kind],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional(),
    )?;
    if prior.is_some_and(|(id, hash)| id == slot.to_string() && hash != digest) {
        return Err(ProviderError::Conflict);
    }
    // Retired bindings cannot own a resumable traversal. Paused bindings retain it.
    registry::db(tx.execute("DELETE FROM mdreader_provider_page_spool_v1 WHERE binding NOT IN (SELECT binding FROM mdreader_provider_registry_sources)", []))?;
    let retained: u64 = registry::db(tx.query_row("SELECT coalesce(sum(length(bytes)),0) FROM mdreader_provider_page_spool_v1 WHERE NOT(binding=?1 AND kind=?2)", params![binding.digest().as_slice(),kind], |r| r.get(0)))?;
    if retained.saturating_add(bytes.len() as u64) > TOTAL_BYTES {
        return Err(ProviderError::TooLarge);
    }
    registry::db(tx.execute("INSERT INTO mdreader_provider_page_spool_v1(binding,kind,slot,digest,bytes) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(binding,kind) DO UPDATE SET slot=excluded.slot,digest=excluded.digest,bytes=excluded.bytes", params![binding.digest().as_slice(),kind,slot.to_string(),digest.as_slice(),bytes]))?;
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
    let value: Option<(Vec<u8>,Vec<u8>)> = registry::db(c.query_row("SELECT digest,bytes FROM mdreader_provider_page_spool_v1 WHERE binding=?1 AND kind=?2 AND slot=?3", params![binding.digest().as_slice(),kind(cursor_kind)?,slot.to_string()], |r| Ok((r.get(0)?,r.get(1)?))).optional())?;
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
mod tests {
    use super::*;
    fn source(c: &mut Connection) -> SourceDescriptor {
        let scope = SelectedScope::ScopedRoot {
            root: RecordKey::new("/plans").unwrap(),
            recursive: true,
        };
        let binding = SourceBinding::new(
            registry::library_id(c).unwrap(),
            SourceId::new("source").unwrap(),
            AccountBinding::new("account").unwrap(),
            Uuid::new_v4(),
            scope.digest(),
        )
        .unwrap();
        let descriptor = SourceDescriptor::new(
            binding,
            ProviderKind::Dropbox,
            DisplayText::new("Public notes").unwrap(),
            scope,
            ProviderCapabilities {
                read: true,
                list: true,
                changes: true,
                assets: false,
                guarded_write: false,
            },
            SourceHealth::Ready,
            Coverage::NeverIndexed,
        )
        .unwrap();
        registry::admit(
            c,
            &descriptor,
            &OpaqueValue::new("synthetic-reference").unwrap(),
            [7; 32],
        )
        .unwrap();
        descriptor
    }
    #[test]
    fn exact_page_survives_owner_reopen_and_slot_replacement_never_returns_other_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("library.sqlite");
        let mut c = Connection::open(&path).unwrap();
        let descriptor = source(&mut c);
        let slot = Uuid::new_v4();
        let bytes = b"public frozen page";
        let digest = body_hash(bytes);
        store(
            &mut c,
            descriptor.binding(),
            CursorKind::List,
            slot,
            digest,
            bytes,
        )
        .unwrap();
        drop(c);
        let mut c = Connection::open(path).unwrap();
        assert_eq!(
            load(&mut c, descriptor.binding(), CursorKind::List, slot, digest).unwrap(),
            bytes
        );
        assert_eq!(
            store(
                &mut c,
                descriptor.binding(),
                CursorKind::List,
                slot,
                body_hash(b"changed bytes"),
                b"changed bytes"
            )
            .unwrap_err(),
            ProviderError::Conflict
        );
        assert_eq!(
            load(&mut c, descriptor.binding(), CursorKind::List, slot, digest).unwrap(),
            bytes
        );
        let newer = Uuid::new_v4();
        let next = b"next remote page";
        store(
            &mut c,
            descriptor.binding(),
            CursorKind::List,
            newer,
            body_hash(next),
            next,
        )
        .unwrap();
        assert_eq!(
            load(&mut c, descriptor.binding(), CursorKind::List, slot, digest).unwrap_err(),
            ProviderError::ResetCursor
        );
        assert_eq!(
            load(
                &mut c,
                descriptor.binding(),
                CursorKind::List,
                newer,
                body_hash(next)
            )
            .unwrap(),
            next
        );
    }
    #[test]
    fn kind_slot_digest_and_saved_binding_are_independent_exact_fences() {
        let mut c = Connection::open_in_memory().unwrap();
        let descriptor = source(&mut c);
        let slot = Uuid::new_v4();
        let bytes = b"public page";
        let digest = body_hash(bytes);
        store(
            &mut c,
            descriptor.binding(),
            CursorKind::List,
            slot,
            digest,
            bytes,
        )
        .unwrap();
        assert_eq!(
            load(
                &mut c,
                descriptor.binding(),
                CursorKind::Changes,
                slot,
                digest
            )
            .unwrap_err(),
            ProviderError::ResetCursor
        );
        assert_eq!(
            load(
                &mut c,
                descriptor.binding(),
                CursorKind::List,
                Uuid::new_v4(),
                digest
            )
            .unwrap_err(),
            ProviderError::ResetCursor
        );
        assert_eq!(
            load(
                &mut c,
                descriptor.binding(),
                CursorKind::List,
                slot,
                [0; 32]
            )
            .unwrap_err(),
            ProviderError::ResetCursor
        );
        let changed = SourceBinding::new(
            descriptor.binding().library_id().clone(),
            descriptor.binding().source_id().clone(),
            descriptor.binding().account().clone(),
            Uuid::new_v4(),
            descriptor.scope().digest(),
        )
        .unwrap();
        assert_eq!(
            load(&mut c, &changed, CursorKind::List, slot, digest).unwrap_err(),
            ProviderError::Unauthorized
        );
        registry::db(c.execute(
            "UPDATE mdreader_provider_registry_sources SET enabled=0",
            [],
        ))
        .unwrap();
        assert_eq!(
            load(&mut c, descriptor.binding(), CursorKind::List, slot, digest).unwrap_err(),
            ProviderError::Unauthorized
        );
    }
    #[test]
    fn oversized_corrupt_and_foreign_schema_pages_fail_without_mutating_the_valid_slot() {
        let mut c = Connection::open_in_memory().unwrap();
        let descriptor = source(&mut c);
        let slot = Uuid::new_v4();
        let bytes = b"public page";
        let digest = body_hash(bytes);
        store(
            &mut c,
            descriptor.binding(),
            CursorKind::List,
            slot,
            digest,
            bytes,
        )
        .unwrap();
        assert_eq!(
            store(
                &mut c,
                descriptor.binding(),
                CursorKind::List,
                Uuid::new_v4(),
                [0; 32],
                &vec![0; MAX_PAGE_SPOOL_BYTES + 1]
            )
            .unwrap_err(),
            ProviderError::TooLarge
        );
        assert_eq!(
            load(&mut c, descriptor.binding(), CursorKind::List, slot, digest).unwrap(),
            bytes
        );
        c.execute("UPDATE mdreader_provider_page_spool_v1 SET bytes=x'01'", [])
            .unwrap();
        assert_eq!(
            load(&mut c, descriptor.binding(), CursorKind::List, slot, digest).unwrap_err(),
            ProviderError::Persistence
        );
        c.execute_batch("CREATE TRIGGER foreign_page_write AFTER INSERT ON mdreader_provider_page_spool_v1 BEGIN SELECT 1; END;").unwrap();
        assert_eq!(
            store(
                &mut c,
                descriptor.binding(),
                CursorKind::List,
                slot,
                digest,
                bytes
            )
            .unwrap_err(),
            ProviderError::Persistence
        );
    }
}
