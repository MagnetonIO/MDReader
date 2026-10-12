//! Local immutable Markdown revisions, with durable exact-operation replay.
//! This is a device action; it never updates a remote sync head or grants access.
use super::*;
use crate::{
    device::{catalog, state},
    ReaderStore,
};
use rusqlite::params;

mod schema;

impl ArchivedDocument {
    /// Preserve referenced attachment bytes from this exact selected snapshot.
    /// Incomplete originals are refused; they remain retained for export/recovery.
    pub fn edited_input(&self, markdown: Vec<u8>) -> PublicationResult<IncomingDocument> {
        let text = std::str::from_utf8(&markdown).map_err(|_| PublicationError::InvalidInput)?;
        let targets = crate::device::assets::incoming_targets(text);
        let mut copied = Vec::new();
        for asset in self.assets() {
            if asset.state != "ready"
                || asset.received != asset.expected_bytes
                || asset.bytes().is_none()
            {
                return Err(PublicationError::Incomplete);
            }
            if targets.contains(&asset.target) {
                copied.push(IncomingAsset::new(
                    asset.target.clone(),
                    asset.mime.clone().ok_or(PublicationError::Incomplete)?,
                    asset.bytes().ok_or(PublicationError::Incomplete)?.to_vec(),
                )?);
            }
        }
        IncomingDocument::new(self.state().document.name.clone(), markdown, copied)
    }
}

pub struct EditSelection {
    receipt: catalog::Receipt,
    operation: uuid::Uuid,
    revision: String,
}
impl EditSelection {
    pub fn new(
        receipt: catalog::Receipt,
        operation: uuid::Uuid,
        markdown: &[u8],
    ) -> PublicationResult<Self> {
        let hash = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if operation.get_version_num() != 4
            || uuid::Uuid::parse_str(&receipt.document_id).is_err()
            || !hash(&receipt.source_id)
            || !hash(&receipt.revision)
            || markdown.len() > 1024 * 1024
            || std::str::from_utf8(markdown).is_err()
        {
            return Err(PublicationError::InvalidInput);
        }
        Ok(Self {
            receipt,
            operation,
            revision: blake3::hash(markdown).to_hex().to_string(),
        })
    }
    pub fn origin(&self) -> IncomingOrigin {
        let mut hash = blake3::Hasher::new();
        hash.update(b"mdreader-local-edit-v1\0");
        for field in [
            &self.receipt.document_id,
            &self.receipt.source_id,
            &self.receipt.revision,
        ] {
            hash.update(&(field.len() as u64).to_le_bytes());
            hash.update(field.as_bytes());
        }
        IncomingOrigin {
            scope_digest: *hash.finalize().as_bytes(),
            item_id: self.operation,
            wire_revision: self.operation,
            session_epoch: self.operation,
            selection_epoch: self.operation,
        }
    }
    fn key(&self) -> PublicationResult<String> {
        Ok(serde_json::to_string(&self.receipt)?)
    }
}

fn replay_in(
    tx: &Transaction<'_>,
    selection: &EditSelection,
) -> PublicationResult<Option<crate::device::DeviceDocument>> {
    schema::initialize(tx)?;
    let row: Option<(String, String, String)> = tx
        .query_row(
            "SELECT previous,revision,document FROM device_edits_v1 WHERE operation=?1",
            [selection.operation.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((previous, revision, id)) = row else {
        return Ok(None);
    };
    if previous != selection.key()? || revision != selection.revision {
        return Err(PublicationError::Conflict);
    }
    let old = state::load(tx, &selection.receipt.document_id, false)?;
    let new = state::load(tx, &id, true)?;
    let retirement = old.retirement.ok_or(PublicationError::Conflict)?;
    if catalog::receipt(&old.document) != selection.receipt
        || retirement.kind != state::RetirementKind::Replaced
        || retirement.operation_id != selection.operation
        || retirement.replacement.map(|v| v.to_string()).as_deref() != Some(&id)
        || new.document.revision != selection.revision
    {
        return Err(PublicationError::Conflict);
    }
    Ok(Some(new.document))
}

/// Check before requiring the previous receipt to remain active. A revoked or
/// superseded result is refused rather than regranted or republished.
pub fn replay_edit(
    store: &mut ReaderStore,
    selection: &EditSelection,
) -> PublicationResult<Option<crate::device::DeviceDocument>> {
    crate::device::schema(store)?;
    store.with_library_connection(|db| {
        let tx = db.transaction()?;
        let result = replay_in(&tx, selection)?;
        tx.commit()?;
        Ok(result)
    })
}

/// Original body/assets, replacement and replay record commit atomically.
pub fn commit_edit(
    store: &mut ReaderStore,
    ready: &ReadyIncoming,
    current: &VerifiedCurrent,
    archive: &ArchivedDocument,
    selection: &EditSelection,
) -> PublicationResult<crate::device::DeviceDocument> {
    commit_edit_with(store,ready,current,archive,selection,|_|Ok(()),|_,_|Ok(()))
}

/// Compose connection metadata with the exact document replacement transaction.
/// Callbacks must neither commit nor create another SQLite writer.
pub fn commit_edit_with(
    store:&mut ReaderStore,
    ready:&ReadyIncoming,
    current:&VerifiedCurrent,
    archive:&ArchivedDocument,
    selection:&EditSelection,
    before:impl FnOnce(&Transaction<'_>)->PublicationResult<()>,
    publish:impl FnOnce(&Transaction<'_>,&crate::device::DeviceDocument)->PublicationResult<()>,
)->PublicationResult<crate::device::DeviceDocument>{
    if let Some(document) = replay_edit(store, selection)? {
        return Ok(document);
    }
    if catalog::receipt(&current.state().document) != selection.receipt
        || ready.revision() != selection.revision
    {
        return Err(PublicationError::Conflict);
    }
    with_replacement(
        store,
        ready,
        current,
        &selection.origin(),
        |db, guard| -> PublicationResult<_> {
            let tx = db.transaction()?;
            if let Some(document) = replay_in(&tx, selection)? {
                tx.commit()?;
                return Ok(document);
            }
            let count: u64 =
                tx.query_row("SELECT count(*) FROM device_edits_v1", [], |r| r.get(0))?;
            if count >= 4096 {
                return Err(PublicationError::IntentLimit);
            }
            revalidate_current_archive(&tx, &guard.old, current, archive)?;
            before(&tx)?;
            let replacement = publish_replacement(&tx, &guard, ready, current)?;
            let document = replacement.publication().document.clone();
            tx.execute("INSERT INTO device_edits_v1(operation,previous,revision,document) VALUES(?1,?2,?3,?4)",
            params![selection.operation.to_string(),selection.key()?,selection.revision,document.id])?;
            revalidate_replacement(&tx, &guard, ready, current, &replacement)?;
            // Fence retired metadata and retained original bytes again after all
            // provisional writes (canonical SQLite on Windows, files on Unix).
            archive.verify_retired_assets_in(&tx)?;
            let replay = replay_in(&tx, selection)?.ok_or(PublicationError::Conflict)?;
            if catalog::receipt(&replay) != catalog::receipt(&document) {
                return Err(PublicationError::Conflict);
            }
            publish(&tx,&document)?;
            tx.commit()?;
            Ok(document)
        },
    )?
}
