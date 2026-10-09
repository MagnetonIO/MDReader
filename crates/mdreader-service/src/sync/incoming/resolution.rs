//! Explicit peer-content preservation. No implicit overwrite or server merge.
use super::*;
use mdreader_core::device::{self, state::DocumentState};
use mdreader_sync::{incoming::IncomingPayload, SyncScope};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingKind {
    Content,
    UserState,
    Deletion,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionChoice {
    KeepBoth,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingEntry {
    pub sequence: u64,
    pub revision_id: String,
    pub kind: PendingKind,
    pub title: String,
    pub reason: PendingReason,
    pub resolution_token: String,
    pub choices: Vec<ResolutionChoice>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolutionRequest {
    pub sequence: u64,
    pub revision_id: String,
    pub resolution_token: String,
    pub choice: ResolutionChoice,
}
impl ResolutionRequest {
    pub fn validate(&self) -> Result<()> {
        if self.sequence == 0
            || self.sequence > 9_007_199_254_740_991
            || !Uuid::parse_str(&self.revision_id)
                .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == self.revision_id)
            || self.resolution_token.len() != 64
            || !self
                .resolution_token
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Scope);
        }
        Ok(())
    }
}
struct View {
    entry: PendingEntry,
    head: Option<Head>,
}
fn view(
    owner: &mut ReaderStore,
    input: &PreparedIncoming,
    origin: &publication::IncomingOrigin,
) -> Result<View> {
    let identity = identity::Identity::from_prepared(input)?;
    let key = scope_key(input.scope())?;
    let (head, baseline, raw, mut reason, peer) = owner.with_library_connection(|db| -> Result<_> {
        let (exact, applied)=storage::staged(db,input.scope(),input.change())?;
        if !exact || applied.checked_add(1)!=Some(input.change().sequence) { return Err(Error::Conflict); }
        let tx=db.transaction()?;
        schema::require_current_library(&tx,input.scope().library_id())?;
        let entry=ledger::inspect(&tx,&key,&identity,origin)?;
        let mut reason=match entry {
            Some(ledger::Entry::Applied(_))=>return Err(Error::Conflict),
            Some(ledger::Entry::Pending(reason))=>Some(reason), _=>None,
        };
        let raw:Option<(String,Option<String>)>=tx.query_row("SELECT origin,document_id FROM native_incoming_items_v1 WHERE scope=?1 AND wire_revision=?2 AND state='reserved'",params![key,input.change().revision_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let head=heads::get(&tx,&key,&input.change().item_id)?;
        let baseline = if matches!(input.payload(), IncomingPayload::Content(_)) {
            if reason.is_none() { reason=heads::content_admission(&identity,head.as_ref()); }
            head.clone()
        } else {
            match super::transition_start::classify(&tx,&key,&identity)? {
                Ok(super::transition_start::Kind::State {target,..}) | Ok(super::transition_start::Kind::DocumentDelete {target,..})=>Some(target),
                Ok(super::transition_start::Kind::StateDelete {..})=>None,
                Err(pending)=>{if reason.is_none(){reason=Some(pending);} None},
            }
        };
        let mut peer=input.change().device!=input.scope().device_id();
        if let Some(stored)=crate::sync::outgoing::association::by_revision(&tx,input.scope(),&input.change().revision_id)? {
            peer=false;
            if stored.intent.origin.session_epoch!=origin.session_epoch || stored.intent.origin.selection_epoch!=origin.selection_epoch {
                reason=Some(PendingReason::ReauthorizationRequired);
            } else if mdreader_sync::outbox_revision_in(&tx,input.scope(),&input.change().revision_id)?.and_then(|entry|entry.receipt).is_some_and(|receipt|receipt.conflict) {
                reason=Some(PendingReason::HeadConflict);
            }
        }
        if let Some(head)=&head {
            match crate::sync::outgoing::association::require_unqueued(&tx,input.scope(),&head.document) {
                Ok(())=>{}, Err(Error::Busy)=>peer=false, Err(error)=>return Err(error),
            }
        }
        tx.commit()?;
        Ok((head,baseline,raw,reason,peer))
    })?;
    let current: Option<DocumentState> = match &baseline {
        None => None,
        Some(head) => match device::state::current(owner, &head.document) {
            Ok(state) => Some(state),
            Err(device::Error::Unavailable) => {
                reason = Some(PendingReason::TargetUnavailable);
                None
            }
            Err(error) => return Err(publication::PublicationError::from(error).into()),
        },
    };
    let kind = match input.payload() {
        IncomingPayload::Content(_) => PendingKind::Content,
        IncomingPayload::UserState(_) => PendingKind::UserState,
        IncomingPayload::OpaqueTombstone => PendingKind::Deletion,
    };
    if reason.is_none() {
        reason = identity::unsupported(input).or_else(|| {
            baseline.as_ref().zip(current.as_ref()).and_then(|(h, c)| {
                (h.generation != Some(c.generation)).then_some(PendingReason::LocalStateChanged)
            })
        });
    }
    let reason = reason.unwrap_or(PendingReason::ReadyToApply);
    let keep = peer
        && kind == PendingKind::Content
        && matches!(
            reason,
            PendingReason::HeadConflict
                | PendingReason::BaselineUnknown
                | PendingReason::LocalStateChanged
        )
        && head
            .as_ref()
            .is_some_and(|h| h.kind == "content" && h.tombstone.is_none())
        && current.as_ref().is_some_and(|s| s.retirement.is_none());
    let state = current.as_ref().map(|s| (&s.document, s.generation));
    let token = mdreader_sync::blob_digest(&serde_json::to_vec(&(
        input.scope(),
        identity.encode()?,
        origin,
        &head,
        &baseline,
        state,
        raw,
    ))?);
    Ok(View {
        entry: PendingEntry {
            sequence: input.change().sequence,
            revision_id: input.change().revision_id.clone(),
            kind,
            title: identity.title,
            reason,
            resolution_token: token,
            choices: if keep {
                vec![ResolutionChoice::KeepBoth]
            } else {
                vec![]
            },
        },
        head,
    })
}
impl IncomingCoordinator {
    pub fn inspect_pending(
        &self,
        selected: &SelectedIncoming,
        input: &PreparedIncoming,
    ) -> Result<PendingEntry> {
        let _operation = self.operation.try_lock().map_err(|_| Error::Busy)?;
        if input.scope() != selected.permit.scope() {
            return Err(Error::Scope);
        }
        let origin = identity::origin(
            input.scope(),
            input,
            selected.permit.session_token(),
            selected.token,
        )?;
        self.short_readonly(selected, |owner, _| Ok(view(owner, input, &origin)?.entry))
    }
    pub fn resolve_pending(
        &self,
        selected: &SelectedIncoming,
        input: &PreparedIncoming,
        request: &ResolutionRequest,
    ) -> Result<Receipt> {
        request.validate()?;
        let _operation = self.operation.try_lock().map_err(|_| Error::Busy)?;
        if input.scope() != selected.permit.scope()
            || input.change().sequence != request.sequence
            || input.change().revision_id != request.revision_id
        {
            return Err(Error::Scope);
        }
        let identity = identity::Identity::from_prepared(input)?;
        let origin = identity::origin(
            input.scope(),
            input,
            selected.permit.session_token(),
            selected.token,
        )?;
        let document = identity::document(input)?;
        let (head, current, archive, plan) = self.short(selected, |owner, _| {
            let observed = view(owner, input, &origin)?;
            if observed.entry.resolution_token != request.resolution_token
                || !observed.entry.choices.contains(&request.choice)
            {
                return Err(Error::Conflict);
            }
            let head = observed.head.ok_or(Error::Conflict)?;
            let assets = self
                .runtime
                .assets(owner, &self.assets)
                .map_err(publication::PublicationError::from)?;
            let current = publication::prepare_current(owner, &head.document, &assets, &origin)?;
            let archive =
                publication::prepare_archive(owner, &head.document, &assets, &self.runtime)?;
            owner.with_library_connection(|db| -> Result<()> {
                let tx = db.transaction()?;
                if ledger::inspect(&tx, &scope_key(input.scope())?, &identity, &origin)?.is_none() {
                    ledger::reserve(&tx, &scope_key(input.scope())?, &identity, &origin)?;
                }
                tx.commit()?;
                Ok(())
            })?;
            let plan =
                publication::reserve_new(owner, &self.documents, &assets, &origin, &document)?;
            owner.with_library_connection(|db| -> Result<()> {
                let tx = db.transaction()?;
                ledger::associate(
                    &tx,
                    &scope_key(input.scope())?,
                    &identity,
                    &origin,
                    plan.document_id(),
                )?;
                tx.commit()?;
                Ok(())
            })?;
            Ok((head, current, archive, plan))
        })?;
        let current = current.verify()?;
        let archive = archive.read()?;
        let ready = plan.write(document)?.validate(
            self.runtime
                .validation_permit()
                .map_err(publication::PublicationError::from)?,
        )?;
        self.short(selected, |owner, session| {
            let receipt = commit_preserved(
                owner,
                input.scope(),
                &identity,
                &origin,
                Preserved {
                    head: &head,
                    current: &current,
                    archive: &archive,
                    ready: &ready,
                },
                |tx| {
                    session.check()?;
                    schema::require_current_library(tx, input.scope().library_id())
                },
            )?;
            let _ = storage::queue_index(owner, &receipt);
            Ok(receipt)
        })
    }
}
struct Preserved<'a> {
    head: &'a Head,
    current: &'a publication::VerifiedCurrent,
    archive: &'a publication::ArchivedDocument,
    ready: &'a publication::ReadyIncoming,
}
fn commit_preserved(
    owner: &mut ReaderStore,
    scope: &SyncScope,
    identity: &identity::Identity,
    origin: &publication::IncomingOrigin,
    preserved: Preserved<'_>,
    final_fence: impl Fn(&rusqlite::Transaction<'_>) -> Result<()>,
) -> Result<Receipt> {
    let Preserved {
        head,
        current,
        archive,
        ready,
    } = preserved;
    publication::with_replacement(owner, ready, current, origin, |db, guard| {
        commit::commit_on(
            db,
            scope,
            identity,
            origin,
            |tx| {
                let key = scope_key(scope)?;
                heads::verify(tx, &key, head)?;
                crate::sync::outgoing::association::require_unqueued(tx, scope, &head.document)?;
                let published =
                    publication::publish_preserved_copy(tx, &guard, ready, current, archive)?;
                let receipt = content_commit::content_receipt(identity, published, 0);
                heads::save(
                    tx,
                    &key,
                    Some(head),
                    &content_commit::content_head(&receipt),
                )?;
                Ok(receipt)
            },
            |tx, receipt| {
                heads::verify(
                    tx,
                    &scope_key(scope)?,
                    &content_commit::content_head(receipt),
                )?;
                publication::revalidate_preserved_copy(tx, &guard, ready, current, archive)?;
                crate::sync::outgoing::association::require_unqueued(tx, scope, &head.document)?;
                Ok(())
            },
            final_fence,
        )
    })?
}
