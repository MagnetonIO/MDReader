//! Only the borrowed reader owner writes this namespace. No second database.
use super::*;
use rusqlite::{OptionalExtension, Transaction};
pub(super) struct Binding {
    pub library: Uuid,
    pub reference: SyncCredentialReference,
}
fn schema(tx: &Transaction<'_>) -> Result<()> {
    let count:u64=tx.query_row("SELECT count(*) FROM sqlite_master WHERE name IN('native_sync_accounts_schema','native_sync_account_binding')",[],|r|r.get(0))?;
    if count == 0 {
        tx.execute_batch(include_str!("migrations/001-account-binding.sql"))?;
        tx.execute("INSERT INTO native_sync_accounts_schema VALUES(1,1)", [])?;
    } else if count != 2 {
        return Err(AccountError::Schema);
    }
    let version: Option<i64> = tx
        .query_row(
            "SELECT version FROM native_sync_accounts_schema WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| AccountError::Schema)?;
    if version != Some(1) {
        return Err(AccountError::Schema);
    }
    let rows: u64 = tx.query_row(
        "SELECT count(*) FROM native_sync_accounts_schema",
        [],
        |r| r.get(0),
    )?;
    if rows != 1 {
        return Err(AccountError::Schema);
    }
    for name in ["native_sync_accounts_schema", "native_sync_account_binding"] {
        let expected = include_str!("migrations/001-account-binding.sql")
            .split(';')
            .find(|s| s.trim_start().starts_with(&format!("CREATE TABLE {name}(")))
            .ok_or(AccountError::Schema)?;
        let actual: String = tx
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
                [name],
                |r| r.get(0),
            )
            .map_err(|_| AccountError::Schema)?;
        let normalize = |s: &str| {
            s.chars()
                .filter(|c| !c.is_ascii_whitespace() && *c != ';')
                .collect::<String>()
        };
        if normalize(&actual) != normalize(expected) {
            return Err(AccountError::Schema);
        }
    }
    Ok(())
}
fn read(tx: &Transaction<'_>) -> Result<Option<Binding>> {
    let count: u64 = tx.query_row(
        "SELECT count(*) FROM native_sync_account_binding",
        [],
        |r| r.get(0),
    )?;
    if count > 1 {
        return Err(AccountError::Schema);
    }
    let row:Option<(String,String)>=tx.query_row("SELECT remote_library,credential_reference FROM native_sync_account_binding WHERE singleton=1",[],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    row.map(|(library, reference)| {
        let id = Uuid::parse_str(&library).map_err(|_| AccountError::Schema)?;
        if id.get_version_num() != 4 || id.to_string() != library {
            return Err(AccountError::Schema);
        }
        Ok(Binding {
            library: id,
            reference: SyncCredentialReference::parse(&reference)
                .map_err(|_| AccountError::Schema)?,
        })
    })
    .transpose()
}
pub(super) fn load(owner: &mut ReaderStore) -> Result<Option<Binding>> {
    owner.with_library_connection(|db| {
        let tx = db.transaction()?;
        schema(&tx)?;
        let binding = read(&tx)?;
        tx.commit()?;
        Ok(binding)
    })
}
/// Ref is retained before OS-vault create. Existing material is never replaced;
/// interruption/restart cannot turn missing/uncertain custody into fresh keys.
pub(super) fn reserve(
    owner: &mut ReaderStore,
    library: Uuid,
    reference: &SyncCredentialReference,
) -> Result<Binding> {
    if library.get_version_num() != 4 {
        return Err(AccountError::Invalid);
    }
    owner.with_library_connection(|db| {
        let tx = db.transaction()?;
        schema(&tx)?;
        if let Some(old) = read(&tx)? {
            if old.library != library || old.reference != *reference {
                return Err(AccountError::Conflict);
            }
            tx.commit()?;
            return Ok(old);
        }
        tx.execute(
            "INSERT INTO native_sync_account_binding VALUES(1,?1,?2)",
            rusqlite::params![library.to_string(), reference.opaque_reference()],
        )?;
        tx.commit()?;
        Ok(Binding {
            library,
            reference: reference.clone(),
        })
    })
}
