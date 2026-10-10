//! Exact lexical index membership on the existing SQLite owner.
use super::{catalog, ReaderStore, Result};
use crate::device::DeviceDocument;

/// Exact derived-row membership, without scanning passages, vectors or jobs.
/// The owner has already selected current enabled, nonretired device receipts.
impl ReaderStore {
    pub(crate) fn observe_device_index(&self, documents: &mut [DeviceDocument]) -> Result<()> {
        catalog::schema::validate(&self.index, true)?;
        let index = self.index.unchecked_transaction()?;
        {
            let mut selected = index.prepare(
            "SELECT EXISTS(SELECT 1 FROM main.documents WHERE source_id=?1 AND path='document.md' AND revision=?2)",
        )?;
            for document in documents {
                // A derived row cannot substitute for the retained source object.
                document.indexed = match self.stored_root(&document.source_id, true) {
                    Ok(root) => {
                        let indexed = selected.query_row(
                            rusqlite::params![document.source_id, document.revision],
                            |row| row.get::<_, bool>(0),
                        )?;
                        indexed && root.verify().is_ok()
                    }
                    Err(_) => false,
                };
            }
        }
        index.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{self, lexical_readiness};
    #[test]
    fn stale_index_revision_is_not_reported_ready_for_the_current_receipt() {
        let owner = tempfile::tempdir().unwrap();
        let files = tempfile::tempdir().unwrap();
        let mut store = ReaderStore::open(owner.path()).unwrap();
        let current = device::import(
            &mut store,
            files.path(),
            "current.md".into(),
            "# Exact current".into(),
        )
        .unwrap();
        let ready = lexical_readiness(&device::list(&mut store).unwrap()).unwrap();
        assert_eq!(ready.indexed_documents, 1);
        // The derived fixture retains an old revision while the authoritative
        // body/receipt stay untouched. A mere source document count is unsafe.
        store
            .index
            .execute(
                "UPDATE documents SET revision=?2 WHERE source_id=?1",
                rusqlite::params![current.source_id, "f".repeat(64)],
            )
            .unwrap();
        let stale = lexical_readiness(&device::list(&mut store).unwrap()).unwrap();
        assert_eq!(stale.indexed_documents, 0);
        assert_ne!(stale.snapshot, ready.snapshot);
        assert_eq!(
            device::read(&mut store, &current.id, Some(&current.revision))
                .unwrap()
                .markdown,
            "# Exact current"
        );
    }
}
