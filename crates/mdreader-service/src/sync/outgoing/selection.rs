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
        expected: &mdreader_core::device::catalog::Receipt,
        operation: Uuid,
    ) -> Result<QueuedDocument> {
        self.queue_selected(selected, authority, expected, operation, false)
    }
    pub fn queue_selected_state(
        &self,
        selected: &SelectedIncoming,
        authority: &NativeAuthority,
        expected: &mdreader_core::device::catalog::Receipt,
        operation: Uuid,
    ) -> Result<QueuedDocument> {
        self.queue_selected(selected, authority, expected, operation, true)
    }
    fn queue_selected(
        &self,
        selected: &SelectedIncoming,
        authority: &NativeAuthority,
        expected: &mdreader_core::device::catalog::Receipt,
        operation: Uuid,
        user_state: bool,
    ) -> Result<QueuedDocument> {
        let _operation = self.operation.try_lock().map_err(|_| Error::Busy)?;
        outgoing::validate_selected(expected)?;
        let document = Uuid::parse_str(&expected.document_id).map_err(|_| Error::Scope)?;
        if operation.get_version_num() != 4 {
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
        // Metadata-only exact reading receipt fence precedes graph body reads,
        // archive file reads, hashes and encryption. No newer unseen revision.
        self.short(selected, |owner, _| {
            let current = mdreader_core::device::catalog::document(owner, &expected.document_id)
                .map_err(publication::PublicationError::from)?;
            if !outgoing::matches_selected(expected, &current) {
                return Err(Error::Conflict);
            }
            Ok(())
        })?;
        // Replaying an already admitted intent never generates new ciphertext.
        if let Some(existing) = self.short(selected, |owner, session| {
            owner.with_library_connection(|db| -> Result<_> {
                let tx = db.transaction()?;
                let stored = association::get(&tx, scope, &operation.to_string())?;
                if let Some(stored) = &stored {
                    let intent = &stored.intent;
                    if intent.native_document != document.to_string()
                        || intent.state.is_some() != user_state
                    {
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
        let (plan, archive, head, target) = self.short(selected, |owner, _| {
            let (head, target) = owner.with_library_connection(|db| -> Result<_> {
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
                let result = if user_state {
                    let target = head.ok_or(Error::Conflict)?;
                    let receipt = heads::receipt(&tx, &key, &target)?;
                    if !outgoing::matches_selected(expected, &receipt.document) { return Err(Error::Conflict); }
                    let items = tx.prepare("SELECT wire_item FROM native_incoming_heads_v2 WHERE scope=?1 AND kind='user_state' AND target_item=?2 LIMIT 2")?.query_map(rusqlite::params![key,target.item], |r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                    let head = match items.as_slice() { [] => None, [id] => heads::get(&tx,&key,id)?, _ => return Err(Error::Conflict) };
                    if head.as_ref().is_some_and(|h| h.tombstone.is_some() || h.document != expected.document_id) { return Err(Error::Conflict); }
                    (head,Some(target))
                } else { (head,None) };
                tx.commit()?;
                Ok(result)
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
            let current = mdreader_core::device::catalog::document(owner, &expected.document_id)
                .map_err(publication::PublicationError::from)?;
            if !outgoing::matches_selected(expected, &current) {
                return Err(Error::Conflict);
            }
            let plan =
                publication::prepare_current(owner, &document.to_string(), &assets, &origin)?;
            let archive =
                publication::prepare_archive(owner, &document.to_string(), &assets, &self.runtime)?;
            Ok(((plan, origin), archive, head, target))
        })?;
        let (plan, origin) = plan;
        // All original/asset reads, hashes and encryption occur off owner/session locks.
        let current = plan.verify()?;
        let snapshot = Snapshot::new(archive.read()?, &current)?;
        let asset_inputs = snapshot.asset_inputs()?;
        let state = target.map(|target| outgoing::SelectedState {
            target,
            pinned: current.state().document.pinned,
            line: current.state().document.line,
        });
        let graph = if let Some(state) = &state {
            if state.line == 0 || state.line > current.max_line() {
                return Err(Error::Conflict);
            }
            authority.seal_selected_state(
                &origin.item_id.to_string(),
                &origin.wire_revision.to_string(),
                &state.payload(),
            )?
        } else {
            authority.seal_selected_content(
                &origin.item_id.to_string(),
                &origin.wire_revision.to_string(),
                OutgoingContent {
                    title: &current.state().document.name,
                    markdown: snapshot.archive.markdown(),
                    assets: &asset_inputs,
                },
            )?
        };
        let intent = Intent {
            state,
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
        #[cfg(test)]
        self.pause_after_materialization()?;
        self.short(selected, |owner, session| {
            admit(
                owner,
                scope,
                Admission {
                    intent: &intent,
                    graph: &graph,
                    current: &current,
                    archive: &snapshot.archive,
                    expected,
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
    pub expected: &'a mdreader_core::device::catalog::Receipt,
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
        expected,
    } = admission;
    let key = identity::scope_key(scope)?;
    if !outgoing::matches_selected(expected, &current.state().document)
        || current.state().document.id != intent.native_document
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
    let content = intent
        .state
        .as_ref()
        .map(|state| &state.target)
        .or(intent.head.as_ref());
    if heads::get(tx, key, &intent.operation.item_id)?.as_ref() != intent.head.as_ref()
        || heads::for_document(tx, key, &intent.native_document)?.as_ref() != content
    {
        return Err(Error::Conflict);
    }
    if let Some(state) = &intent.state {
        heads::verify(tx, key, &state.target)?;
    }
    Ok(())
}
