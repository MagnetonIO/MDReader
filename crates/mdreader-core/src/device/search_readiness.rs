//! Stable lexical observations of the bounded current device receipt set.
//! The digest is a change signal, never a source grant or a model-ready claim.
use super::{DeviceDocument, Error, Result, MAX_DOCUMENTS};
pub struct DeviceLexicalReadiness {
    pub indexed_documents: u64,
    pub snapshot: String,
}

/// Supply the current authorized `device::list` observation from the same owner
/// turn. Pin/position/order and scheduler/model progress do not alter this token.
pub fn lexical_readiness(documents: &[DeviceDocument]) -> Result<DeviceLexicalReadiness> {
    if documents.len() as u64 > MAX_DOCUMENTS {
        return Err(Error::Quota);
    }
    let mut selected = documents.iter().collect::<Vec<_>>();
    selected.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    let mut digest = blake3::Hasher::new();
    digest.update(b"mdreader-device-lexical-snapshot-v1\0");
    digest.update(&(selected.len() as u64).to_le_bytes());
    for document in selected {
        for value in [&document.id, &document.source_id, &document.revision] {
            digest.update(&(value.len() as u64).to_le_bytes());
            digest.update(value.as_bytes());
        }
        digest.update(&[u8::from(document.indexed)]);
    }
    Ok(DeviceLexicalReadiness {
        indexed_documents: documents.iter().filter(|d| d.indexed).count() as u64,
        snapshot: digest.finalize().to_hex().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{device, ReaderStore};

    #[test]
    fn snapshot_tracks_exact_readiness_and_membership_but_not_user_state_or_job_replay() {
        let owner = tempfile::tempdir().unwrap();
        let files = tempfile::tempdir().unwrap();
        let mut store = ReaderStore::open(owner.path()).unwrap();
        let first = device::import(
            &mut store,
            files.path(),
            "first.md".into(),
            "# First\n\nExact cedar passage.\n".into(),
        )
        .unwrap();
        let second = device::import(
            &mut store,
            files.path(),
            "second.md".into(),
            "# Second".into(),
        )
        .unwrap();
        let before = lexical_readiness(&device::list(&mut store).unwrap()).unwrap();
        assert_eq!(before.indexed_documents, 2);
        device::pin(&mut store, &first.id, true).unwrap();
        device::position(&mut store, &first.id, &first.revision, 3).unwrap();
        store.queue_index(&first.source_id).unwrap();
        let mut reordered = device::list(&mut store).unwrap();
        reordered.reverse();
        assert_eq!(
            lexical_readiness(&reordered).unwrap().snapshot,
            before.snapshot
        );
        store.forget_source_index(&first.source_id).unwrap();
        // Forget revokes the grant too; explicitly reactivate the retained
        // binding to observe a current receipt with no derived index rows.
        store.set_source_enabled(&first.source_id, true).unwrap();
        let pending = lexical_readiness(&device::list(&mut store).unwrap()).unwrap();
        assert_eq!(pending.indexed_documents, 1);
        assert_ne!(pending.snapshot, before.snapshot);
        store.index_source(&first.source_id).unwrap();
        assert_eq!(
            lexical_readiness(&device::list(&mut store).unwrap())
                .unwrap()
                .snapshot,
            before.snapshot
        );
        store.set_source_enabled(&second.source_id, false).unwrap();
        let revoked = lexical_readiness(&device::list(&mut store).unwrap()).unwrap();
        assert_eq!(revoked.indexed_documents, 1);
        assert_ne!(
            revoked.snapshot, pending.snapshot,
            "equal ready counts do not equate scopes"
        );
    }
}
