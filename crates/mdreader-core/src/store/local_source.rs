//! Local filesystem grant admission on the existing durable owner transaction.
use super::{decode_path, encode_path, invalid, revision, ReaderStore, Result, Source};
use crate::SourceRoot;
use rusqlite::{params, OptionalExtension, Transaction};

pub(crate) fn validate_root(transaction: &Transaction<'_>, root: &SourceRoot) -> Result<()> {
    root.verify()?;
    let private: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT cache_path FROM core_provider_profile WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(private) = private {
        let private = decode_path(private)?;
        let local = std::fs::canonicalize(&root.path)?;
        if private.starts_with(&local) || local.starts_with(&private) {
            return Err(invalid("local source overlaps private provider cache"));
        }
    }
    Ok(())
}

/// Publish one local source and trusted owner metadata in the caller's exact
/// borrowed SQLite transaction. No transaction/savepoint, connection, file write,
/// index update or commit is created here. Files must already be durable.
///
/// The retained root and both directions of private-cache overlap are validated
/// before admission and after the callback. Repeated admission preserves a revoked
/// source's disabled state; callers requiring an active grant must reject it.
///
/// On any error the caller MUST roll back/drop the WHOLE outer transaction, never
/// catch the error and commit partial writes. Success is provisional until that
/// transaction commits. Rollback does not undo already-published filesystem bytes;
/// the owner is responsible for persisted intents and crash reconciliation.
///
/// This is one root publication unit. Future multi-root composition must retain
/// and finally validate every root/overlap after all callbacks, before committing.
/// Keep only short synchronous owner work after final validation; no network/OS
/// custody waits or second writer. Rebuildable indexing follows durable commit.
pub fn publish_local_source<T>(
    transaction: &Transaction<'_>,
    name: &str,
    root: SourceRoot,
    publish: impl FnOnce(&Transaction<'_>, &Source) -> Result<T>,
) -> Result<(Source, T)> {
    if name.trim().is_empty() || name.len() > 256 {
        return Err(invalid("invalid source name"));
    }
    validate_root(transaction, &root)?;
    let identity =
        serde_json::to_string(&root.identity).map_err(|_| invalid("invalid source identity"))?;
    let id = revision(identity.as_bytes());
    // Idempotent admission cannot implicitly re-grant a disabled identity.
    transaction.execute(
        "INSERT INTO sources VALUES(?1,?2,?3,?4,1) ON CONFLICT(id) DO UPDATE SET name=excluded.name,path=excluded.path",
        params![id, name, encode_path(&root.path), identity],
    )?;
    let source = transaction.query_row(
        "SELECT id,name,enabled FROM sources WHERE id=?1",
        [&id],
        |row| {
            Ok(Source {
                id: row.get(0)?,
                name: row.get(1)?,
                enabled: row.get(2)?,
            })
        },
    )?;
    let value = publish(transaction, &source)?;
    validate_root(transaction, &root)?;
    Ok((source, value))
}

impl ReaderStore {
    pub fn add_local_source(&mut self, name: &str, root: SourceRoot) -> Result<Source> {
        self.add_local_source_with(name, root, |_, _| Ok(()))
            .map(|(source, ())| source)
    }

    /// Owner convenience wrapper that commits source and metadata together, or
    /// rolls both back on callback/final validation failure. No implicit re-grant.
    pub fn add_local_source_with<T>(
        &mut self,
        name: &str,
        root: SourceRoot,
        publish: impl FnOnce(&Transaction<'_>, &Source) -> Result<T>,
    ) -> Result<(Source, T)> {
        let transaction = self.library.transaction()?;
        let result = publish_local_source(&transaction, name, root, publish)?;
        transaction.commit()?;
        Ok(result)
    }
}
