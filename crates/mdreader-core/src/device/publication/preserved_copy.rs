//! Publish a separate incoming copy while retaining the exact current original.
use super::*;
use rusqlite::Transaction;

/// Uses the same owner-bound dual guard as replacement, but never retires or
/// changes the original document, its source, assets, pin or reading position.
pub fn publish_preserved_copy(
    tx: &Transaction<'_>,
    guard: &ReplacementGuard<'_>,
    ready: &ReadyIncoming,
    current: &VerifiedCurrent,
    archive: &ArchivedDocument,
) -> PublicationResult<PublicationReceipt> {
    revalidate_current_archive(tx, &guard.old, current, archive)?;
    let receipt = publish_new(tx, &guard.new, ready)?;
    revalidate_preserved_copy(tx, guard, ready, current, archive)?;
    Ok(receipt)
}

pub fn revalidate_preserved_copy(
    tx: &Transaction<'_>,
    guard: &ReplacementGuard<'_>,
    ready: &ReadyIncoming,
    current: &VerifiedCurrent,
    archive: &ArchivedDocument,
) -> PublicationResult<()> {
    revalidate_current_archive(tx, &guard.old, current, archive)?;
    revalidate_publication(tx, &guard.new, ready)
}
