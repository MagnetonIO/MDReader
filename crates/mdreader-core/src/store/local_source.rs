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
        if local.starts_with(&private)
            || (private.starts_with(&local) && !root.excludes_namespace(&private))
        {
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
    let key =
        serde_json::to_string(&root.identity).map_err(|_| invalid("invalid source identity"))?;
    // Folders are matched by path: an existing source at this path keeps its
    // id (and index, grants, bookmarks) even when the volume renumbered its
    // device/inode. Only a new path derives a new id. The observed identity
    // is persisted so the stored value follows the filesystem.
    let observed = serde_json::to_string(&root.observed_identity()?)
        .map_err(|_| invalid("invalid source identity"))?;
    let existing: Option<String> = transaction
        .query_row(
            "SELECT id FROM sources WHERE path=?1 AND id NOT LIKE 'provider:%' ORDER BY identity=?2 DESC,id LIMIT 1",
            params![encode_path(&root.path), key],
            |row| row.get(0),
        )
        .optional()?;
    let id = existing.unwrap_or_else(|| revision(key.as_bytes()));
    // Idempotent admission cannot implicitly re-grant a disabled identity.
    transaction.execute(
        "INSERT INTO sources VALUES(?1,?2,?3,?4,1) ON CONFLICT(id) DO UPDATE SET name=excluded.name,path=excluded.path,identity=excluded.identity WHERE sources.name<>excluded.name OR sources.path<>excluded.path OR sources.identity<>excluded.identity",
        params![id, name, encode_path(&root.path), observed],
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
    /// Reopen only an explicitly admitted, enabled user folder. Provider caches
    /// and device-owned immutable revisions are never agent project roots.
    pub fn chosen_local_root(&self, source: &str) -> Result<SourceRoot> {
        if source.starts_with("provider:")
            || crate::local_authority::is_owned(&self.library, source)?
            || crate::device::is_device_source(&self.library, source)?
        {
            return Err(crate::ReaderError::Unavailable);
        }
        self.stored_root(source, true)
    }
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
        let root = crate::local_authority::protect(&self.library, root)?;
        let transaction = self.library.transaction()?;
        let result = publish_local_source(&transaction, name, root, publish)?;
        crate::local_authority::refresh(&transaction)?;
        transaction.commit()?;
        Ok(result)
    }
}

#[cfg(all(test, unix))]
mod path_identity_tests {
    use crate::{ReaderStore, SourceRoot, SourceRootIdentity};

    #[test]
    fn a_renumbered_device_keeps_the_source_and_persists_the_observed_identity() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("notes");
        std::fs::create_dir(&folder).unwrap();
        let folder = folder.canonicalize().unwrap();
        let mut store = ReaderStore::open(&temp.path().join("reader")).unwrap();
        let current = SourceRoot::capture(folder.clone()).unwrap();
        let source = store.add_local_source("Notes", current.clone()).unwrap();
        let SourceRootIdentity::Unix { device, inode } = current.identity.clone() else {
            unreachable!()
        };
        // Simulate a reboot that renumbered st_dev (and a different inode).
        let stale = SourceRootIdentity::Unix {
            device: device.wrapping_add(1),
            inode: inode.wrapping_add(3),
        };
        let stored = |store: &mut ReaderStore| -> String {
            store
                .with_library_connection(|db| {
                    db.query_row(
                        "SELECT identity FROM sources WHERE id=?1",
                        [&source.id],
                        |r| r.get(0),
                    )
                })
                .unwrap()
        };
        let stale_json = serde_json::to_string(&stale).unwrap();
        store
            .with_library_connection(|db| {
                db.execute(
                    "UPDATE sources SET identity=?2 WHERE id=?1",
                    [&source.id, &stale_json],
                )
            })
            .unwrap();
        // Lookup by the stale or the observed identity finds the same source.
        for identity in [&stale, &current.identity] {
            assert_eq!(
                store
                    .find_local_source(&folder, identity)
                    .unwrap()
                    .unwrap()
                    .id,
                source.id
            );
        }
        // Reopening adopts the folder and persists the observed identity.
        store
            .chosen_local_root(&source.id)
            .unwrap()
            .verify()
            .unwrap();
        assert_eq!(
            stored(&mut store),
            serde_json::to_string(&current.identity).unwrap()
        );
        // Re-admitting with the stale key (an older registry) keeps the id.
        store
            .with_library_connection(|db| {
                db.execute(
                    "UPDATE sources SET identity=?2 WHERE id=?1",
                    [&source.id, &stale_json],
                )
            })
            .unwrap();
        let readmitted = store
            .add_local_source("Notes", SourceRoot::restore(folder.clone(), stale).unwrap())
            .unwrap();
        assert_eq!(readmitted.id, source.id);
        assert_eq!(store.list_sources().unwrap().len(), 1);
        assert_eq!(
            stored(&mut store),
            serde_json::to_string(&current.identity).unwrap()
        );
    }
}
