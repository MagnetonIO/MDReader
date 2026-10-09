use super::*;
use std::{
    future::Future,
    time::{SystemTime, UNIX_EPOCH},
};

fn now_ms() -> Result<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ProviderError::Protocol)?
        .as_millis()
        .try_into()
        .map_err(|_| ProviderError::Protocol)
}
// Bound each explicit refresh; committed staging resumes on the next request.
const MAX_BATCH_PAGES: usize = 8;
/// Bound even a misbehaving injected provider to the caller's operation context.
async fn io<T>(context: &IoContext, future: impl Future<Output = Result<T>>) -> Result<T> {
    context.check()?;
    tokio::select! {biased;_=context.cancellation().cancelled()=>Err(ProviderError::Cancelled),result=tokio::time::timeout_at(tokio::time::Instant::from_std(context.deadline()),future)=>result.map_err(|_|ProviderError::Timeout)?}
}
#[derive(Debug, Default)]
pub struct SyncResult {
    pub pages: usize,
    pub metadata_complete: bool,
}
/// Resume durable staging or refresh a published checkpoint. No writer lock is
/// held across any await. Initial change tracking begins before enumeration.
/// Completed metadata is not completed indexing; bodies are admitted separately.
pub async fn synchronize<O: Owner, P: DocumentProvider + ?Sized>(
    owner: &O,
    provider: &P,
    projection: &dyn ProjectionAdmission,
    context: &IoContext,
) -> Result<SyncResult> {
    let binding = provider.descriptor().binding();
    owner.with_connection(|c| revalidate(c, binding))?;
    let mut progress = owner.with_connection(|c| resume(c, binding))?;
    if let Some(pending) = &progress {
        if !owner.with_connection(|c| super::registry::snapshot_is_current(c, &pending.snapshot))? {
            owner.with_connection(|c| discard_staging(c, binding))?;
            progress = None;
        }
    }
    if progress.is_none() {
        let checkpoint = owner.with_connection(|c| checkpoint(c, binding))?;
        let snapshot = if checkpoint.is_some() && provider.descriptor().capabilities().changes {
            owner.with_connection(|c| begin_delta(c, binding))?
        } else {
            let start = if provider.descriptor().capabilities().changes {
                Some(io(context, provider.start_changes(context)).await?)
            } else {
                None
            };
            owner.with_connection(|c| begin_snapshot(c, binding, start.as_ref()))?
        };
        progress = owner.with_connection(|c| resume(c, binding))?;
        if progress
            .as_ref()
            .is_none_or(|p| p.snapshot.id() != snapshot.id())
        {
            return Err(ProviderError::Persistence);
        }
    }
    let mut progress = progress.ok_or(ProviderError::Persistence)?;
    let mut result = SyncResult::default();
    while progress.enumerating {
        if result.pages >= MAX_BATCH_PAGES {
            return Ok(result);
        }
        let page = match io(
            context,
            provider.list(progress.list_cursor.as_ref(), context),
        )
        .await
        {
            Err(ProviderError::ResetCursor) => {
                owner.with_connection(|c| discard_staging(c, binding))?;
                return Ok(result);
            }
            other => other?,
        };
        if page.rows().len() > context.page_rows() {
            return Err(ProviderError::TooLarge);
        }
        context.check()?;
        owner.with_connection(|c| {
            stage_page(c, &progress.snapshot, progress.list_cursor.as_ref(), &page)
        })?;
        progress = owner
            .with_connection(|c| resume(c, binding))?
            .ok_or(ProviderError::Persistence)?;
        result.pages += 1;
    }
    // A ready snapshot can survive process exit before publication; do not
    // dispatch the already-committed final change checkpoint a second time.
    if provider.descriptor().capabilities().changes {
        while !owner.with_connection(|c| super::registry::ready(c, &progress.snapshot))? {
            if result.pages >= MAX_BATCH_PAGES {
                return Ok(result);
            }
            let current = progress
                .change_cursor
                .as_ref()
                .ok_or(ProviderError::Persistence)?;
            let outcome = match io(context, provider.changes(current, context)).await {
                Err(ProviderError::ResetCursor) => {
                    owner.with_connection(|c| reset_for_rebuild(c, binding))?;
                    return Err(ProviderError::ResetCursor);
                }
                other => other?,
            };
            match outcome {
                ChangeOutcome::RebuildRequired => {
                    owner.with_connection(|c| reset_for_rebuild(c, binding))?;
                    return Err(ProviderError::ResetCursor);
                }
                ChangeOutcome::Page {
                    events,
                    next_cursor,
                    checkpoint,
                } => {
                    if events.len() > context.page_rows() {
                        return Err(ProviderError::TooLarge);
                    }
                    let mut changes = Vec::with_capacity(events.len());
                    for event in events {
                        match event {
                            Change::Upsert { key } => {
                                let metadata =
                                    io(context, provider.metadata(&key, context)).await?;
                                if metadata.key != key {
                                    return Err(ProviderError::IdentityMismatch);
                                }
                                changes.push(ResolvedChange::Upsert(metadata));
                            }
                            Change::Deleted { key, expected_path } => {
                                changes.push(ResolvedChange::Deleted { key, expected_path })
                            }
                        }
                    }
                    context.check()?;
                    owner.with_connection(|c| {
                        stage_changes(
                            c,
                            &progress.snapshot,
                            current,
                            &changes,
                            next_cursor.as_ref(),
                            checkpoint.as_ref(),
                            now_ms()?,
                        )
                    })?;
                    progress = owner
                        .with_connection(|c| resume(c, binding))?
                        .ok_or(ProviderError::Persistence)?;
                    result.pages += 1;
                }
            }
        }
    }
    context.check()?;
    owner.with_connection(|c| publish_snapshot(c, &progress.snapshot, projection))?;
    result.metadata_complete = owner.with_connection(|c| {
        Ok(list(c, binding.library_id())?
            .into_iter()
            .find(|s| s.source_id == *binding.source_id())
            .is_some_and(|s| {
                matches!(
                    s.coverage,
                    Coverage::MetadataCompleteSelected { .. } | Coverage::CompleteSelected { .. }
                )
            }))
    })?;
    Ok(result)
}
/// Fetch exact current metadata revision, durably cache it, then explicitly ask
/// core for admission. Unsupported admission leaves a private cached body only.
pub async fn fetch_and_admit<O: Owner, P: DocumentProvider + ?Sized>(
    owner: &O,
    provider: &P,
    key: &RecordKey,
    projection: &dyn ProjectionAdmission,
    context: &IoContext,
) -> Result<ProjectionReceipt> {
    let binding = provider.descriptor().binding();
    owner.with_connection(|c| {
        revalidate(c, binding)?;
        metadata(c, binding, key).map(|_| ())
    })?;
    let document = io(context, provider.read(key, context)).await?;
    if document.key != *key {
        return Err(ProviderError::IdentityMismatch);
    }
    context.check()?;
    owner.with_connection(|c| cache_body(c, binding, &document))?;
    owner.with_connection(|c| admit_body(c, binding, &document, projection))
}
/// Explicit dispatch only. The durable permit is consumed exactly once. Any
/// post-dispatch error remains uncertain and is never automatically reissued.
pub async fn dispatch_write<O: Owner, P: DocumentProvider + ?Sized>(
    owner: &O,
    provider: &P,
    operation: Uuid,
    projection: &dyn ProjectionAdmission,
    context: &IoContext,
) -> Result<ProjectionReceipt> {
    let binding = provider.descriptor().binding();
    context.check()?;
    let (pending, permit) = owner.with_connection(|c| {
        revalidate(c, binding)?;
        let pending = journal::load(c, binding, operation)?;
        if metadata(c, binding, &pending.key)?.version != pending.expected {
            return Err(ProviderError::Conflict);
        }
        let permit = journal::begin_dispatch(c, binding, operation)?;
        Ok((pending, permit))
    })?;
    let received = io(
        context,
        provider.guarded_write(permit, &pending.body, context),
    )
    .await;
    let receipt = match received {
        Ok(receipt) => receipt,
        Err(error) => {
            owner.with_connection(|c| journal::mark_unknown(c, binding, operation))?;
            return Err(match error {
                ProviderError::Conflict => ProviderError::Conflict,
                _ => ProviderError::UnknownWrite {
                    operation_id: operation,
                },
            });
        }
    };
    let integrity = receipt
        .version
        .content_hash
        .as_ref()
        .map(|hash| verify_content_hash(pending.body.as_bytes(), hash))
        .transpose();
    if receipt.operation_id != operation
        || receipt.key != pending.key
        || receipt.body_hash != body_hash(pending.body.as_bytes())
        || receipt.size_bytes != pending.body.as_bytes().len() as u64
        || integrity.is_err()
    {
        owner.with_connection(|c| journal::mark_unknown(c, binding, operation))?;
        return Err(ProviderError::UnknownWrite {
            operation_id: operation,
        });
    }
    // Receipt evidence remains durable even if authority was revoked while the
    // remote request was in flight. Revocation prevents local publication below.
    if owner
        .with_connection(|c| journal::accept_receipt(c, binding, &receipt))
        .is_err()
    {
        owner.with_connection(|c| journal::mark_unknown(c, binding, operation))?;
        return Err(ProviderError::UnknownWrite {
            operation_id: operation,
        });
    }
    owner
        .with_connection(|c| publish_mutation(c, binding, operation, projection, now_ms()?))
        .map_err(|_| ProviderError::LocalPublicationPending)
}
#[derive(Debug, Default)]
pub struct RepairResult {
    pub recovered_unknown: usize,
    pub published: usize,
    pub conflicts: usize,
    pub pending: usize,
}
/// Startup repair performs read-only reconciliation for unknown writes and
/// idempotent local publication for accepted receipts. Prepared intents require
/// a new explicit dispatch request; no restart path performs a provider write.
pub async fn repair_mutations<O: Owner, P: DocumentProvider + ?Sized>(
    owner: &O,
    provider: &P,
    projection: &dyn ProjectionAdmission,
    context: &IoContext,
) -> Result<RepairResult> {
    let binding = provider.descriptor().binding();
    let mut result = RepairResult {
        recovered_unknown: owner.with_connection(|c| journal::recover_interrupted(c, binding))?,
        ..Default::default()
    };
    owner.with_connection(|c| revalidate(c, binding))?;
    let ids = owner.with_connection(|c| journal::pending_ids(c, binding))?;
    for id in ids {
        context.check()?;
        let mut pending = owner.with_connection(|c| journal::load(c, binding, id))?;
        if pending.state == journal::MutationState::Unknown {
            owner.with_connection(|c| revalidate(c, binding))?;
            match io(context, provider.read(&pending.key, context)).await {
                Ok(document) => {
                    if document.key != pending.key || document.observed_at_ms < 0 {
                        return Err(ProviderError::IdentityMismatch);
                    }
                    if let Some(hash) = &document.version.content_hash {
                        verify_content_hash(document.body.as_bytes(), hash)?
                    }
                    let reconciled = owner.with_connection(|c| {
                        revalidate(c, binding)?;
                        journal::reconcile_read(c, binding, id, &document)
                    });
                    pending = owner.with_connection(|c| journal::load(c, binding, id))?;
                    if let Err(error) = reconciled {
                        if error != ProviderError::Conflict
                            || pending.state != journal::MutationState::Conflict
                        {
                            return Err(error);
                        }
                    }
                }
                Err(ProviderError::Cancelled | ProviderError::Timeout) => {
                    return Err(if context.cancellation().is_cancelled() {
                        ProviderError::Cancelled
                    } else {
                        ProviderError::Timeout
                    })
                }
                Err(_) => {
                    result.pending += 1;
                    continue;
                }
            }
        }
        match pending.state {
            journal::MutationState::Accepted => {
                if owner
                    .with_connection(|c| publish_mutation(c, binding, id, projection, now_ms()?))
                    .is_ok()
                {
                    result.published += 1
                } else {
                    result.pending += 1
                }
            }
            journal::MutationState::Conflict => result.conflicts += 1,
            journal::MutationState::Published => {}
            _ => result.pending += 1,
        }
    }
    Ok(result)
}
