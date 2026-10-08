//! Compiled beneath incoming to reuse its exact owner, selection and operation gate.
use super::*;
use crate::sync::outgoing::{self, association, snapshot::Snapshot, Intent, QueuedDocument};
use mdreader_sync::{outgoing::OutgoingContent, CommitOperation, SyncLimits};

impl IncomingCoordinator {
    /// Native blocking-worker seam only. The product exposes no QueueDocument
    /// RPC/UI until real admission, self-echo and cancellation acceptance pass.
    /// Caller supplies one explicit device document and stable operation UUID.
    pub fn queue_selected_document(
        &self,
        selected: &SelectedIncoming,
        authority: &NativeAuthority,
        document: Uuid,
        operation: Uuid,
    ) -> Result<QueuedDocument> {
        let _operation = self.operation.try_lock().map_err(|_| Error::Busy)?;
        if document.get_version_num() != 4 || operation.get_version_num() != 4 {
            return Err(Error::Scope);
        }
        let authority_permit = authority.projection_permit()?;
        if authority_permit.scope() != selected.permit.scope()
            || authority_permit.session_token() != selected.permit.session_token()
        {
            return Err(Error::Scope);
        }
        let scope = selected.permit.scope();
        let key = identity::scope_key(scope)?;
        // Replaying an already admitted intent never generates new ciphertext.
        if let Some(existing) = self.short(selected, |owner, session| {
            owner.with_library_connection(|db| -> Result<_> {
                let tx = db.transaction()?;
                let stored = association::get(&tx, scope, &operation.to_string())?;
                if let Some(stored) = &stored {
                    let intent = &stored.intent;
                    if intent.native_document != document.to_string() {
                        return Err(Error::Conflict);
                    }
                    if intent.origin.session_epoch != selected.permit.session_token()
                        || intent.origin.selection_epoch != selected.token
                    {
                        return Err(Error::Inactive);
                    }
                    let graph = mdreader_sync::graph_entry_in(&tx, scope, &operation.to_string())?;
                    if graph.operation != intent.operation {
                        return Err(Error::Conflict);
                    }
                }
                session.check()?;
                tx.commit()?;
                Ok(stored)
            })
        })? {
            return existing.intent.result();
        }
        let (plan, archive, head) = self.short(selected, |owner, _| {
            let head = owner.with_library_connection(|db| -> Result<_> {
                let tx = db.transaction()?;
                association::require_unqueued(&tx, scope, &document.to_string())?;
                let head = heads::for_document(&tx, &key, &document.to_string())?;
                association::verify_existing_head(
                    &tx,
                    scope,
                    &document.to_string(),
                    head.as_ref(),
                )?;
                if head.as_ref().is_some_and(|h| {
                    h.kind != "content" || h.tombstone.is_some() || h.generation.is_none()
                }) {
                    return Err(Error::Conflict);
                }
                tx.commit()?;
                Ok(head)
            })?;
            // Strict absent detection precedes controlled creation. Partial/future
            // asset namespaces never fall back to initialization.
            let assets = match self
                .runtime
                .existing_assets(owner, &self.assets)
                .map_err(publication::PublicationError::from)?
            {
                Some(assets) => assets,
                None => self
                    .runtime
                    .assets(owner, &self.assets)
                    .map_err(publication::PublicationError::from)?,
            };
            let item = head
                .as_ref()
                .map(|h| Uuid::parse_str(&h.item).map_err(|_| Error::Schema))
                .transpose()?
                .unwrap_or_else(Uuid::new_v4);
            let origin = publication::IncomingOrigin {
                scope_digest: outgoing::scope_digest(scope)?,
                item_id: item,
                wire_revision: Uuid::new_v4(),
                session_epoch: selected.permit.session_token(),
                selection_epoch: selected.token,
            };
            let plan =
                publication::prepare_current(owner, &document.to_string(), &assets, &origin)?;
            let archive =
                publication::prepare_archive(owner, &document.to_string(), &assets, &self.runtime)?;
            Ok(((plan, origin), archive, head))
        })?;
        let (plan, origin) = plan;
        // All original/asset reads, hashes and encryption occur off owner/session locks.
        let current = plan.verify()?;
        let snapshot = Snapshot::new(archive.read()?, &current)?;
        let asset_inputs = snapshot.asset_inputs()?;
        let graph = authority.seal_selected_content(
            &origin.item_id.to_string(),
            &origin.wire_revision.to_string(),
            OutgoingContent {
                title: &current.state().document.name,
                markdown: snapshot.archive.markdown(),
                assets: &asset_inputs,
            },
        )?;
        let intent = Intent {
            operation: CommitOperation {
                operation_id: operation.to_string(),
                item_id: origin.item_id.to_string(),
                revision_id: origin.wire_revision.to_string(),
                parents: head
                    .as_ref()
                    .map(|h| vec![h.revision.clone()])
                    .unwrap_or_default(),
                blob: Some(graph.root().blob_id().into()),
                deleted: false,
            },
            native_document: document.to_string(),
            native_revision: current.state().document.revision.clone(),
            generation: current.state().generation,
            markdown_sha256: snapshot.markdown_sha256,
            assets_sha256: snapshot.assets_sha256,
            origin,
            head,
        };
        self.short(selected, |owner, session| {
            admit(
                owner,
                scope,
                Admission {
                    intent: &intent,
                    graph: &graph,
                    current: &current,
                    archive: &snapshot.archive,
                },
                |_| {
                    session.check()?;
                    Ok(())
                },
            )
        })
    }
}
pub(super) struct Admission<'a> {
    pub intent: &'a Intent,
    pub graph: &'a mdreader_sync::outgoing::SealedGraph,
    pub current: &'a publication::VerifiedCurrent,
    pub archive: &'a publication::ArchivedDocument,
}
pub(super) fn admit(
    owner: &mut ReaderStore,
    scope: &mdreader_sync::SyncScope,
    admission: Admission<'_>,
    final_fence: impl Fn(&rusqlite::Transaction<'_>) -> Result<()>,
) -> Result<QueuedDocument> {
    let Admission {
        intent,
        graph,
        current,
        archive,
    } = admission;
    let key = identity::scope_key(scope)?;
    if current.state().document.id != intent.native_document
        || current.state().document.revision != intent.native_revision
        || current.state().generation != intent.generation
    {
        return Err(Error::Conflict);
    }
    publication::with_transition(owner, current, &intent.origin, |db, guard| -> Result<_> {
        let tx = db.transaction()?;
        publication::revalidate_current_archive(&tx, &guard, current, archive)?;
        verify_head(&tx, &key, intent)?;
        association::insert(&tx, scope, intent)?;
        let entry = mdreader_sync::enqueue_graph_in(
            &tx,
            scope,
            &SyncLimits::default(),
            &intent.operation.operation_id,
            &intent.operation.parents,
            graph,
        )?;
        if entry.operation != intent.operation {
            return Err(Error::Conflict);
        }
        association::verify(&tx, scope, intent, None)?;
        verify_head(&tx, &key, intent)?;
        publication::revalidate_current_archive(&tx, &guard, current, archive)?;
        final_fence(&tx)?;
        // No caller SQL is allowed after the final fence.
        tx.commit()?;
        intent.result()
    })?
}

pub(super) fn verify_head(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    intent: &Intent,
) -> Result<()> {
    if heads::get(tx, key, &intent.operation.item_id)?.as_ref() != intent.head.as_ref()
        || heads::for_document(tx, key, &intent.native_document)?.as_ref() != intent.head.as_ref()
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
