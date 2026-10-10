//! Reconcile only a fully finalized native candidate. Existing partial data
//! remains intact; no pathname or serialized receipt manufactures closure.
use super::super::legacy_record::Record;
use super::*;

pub(super) fn candidate(
    root: &SourceRoot,
    record: &Record,
    snapshot: &mdreader_profile::VerifiedSnapshot,
    archive: &ConversionArchive,
    expected: Option<&CandidateReceipt>,
) -> Result<(ClosedOwner, CandidateReceipt)> {
    root.verify().map_err(|_| Error::Conflict)?;
    archive.verify()?;
    let path = root.path.join("candidate");
    let mut candidate = ClosedOwner::acquire(&path)?;
    let receipt = candidate.committed_candidate(record.operation)?;
    let selected_root = super::super::selection::restore(&receipt.candidate)?;
    if let Some(expected) = expected {
        if serde_json::to_vec(expected).map_err(|_| Error::Integrity)?
            != serde_json::to_vec(&receipt).map_err(|_| Error::Integrity)?
        {
            return Err(Error::Conflict);
        }
    }
    if receipt.original != *record.launch.profile()
        || receipt.original != snapshot.manifest().original
        || receipt.snapshot != *snapshot.identity()
        || selected_root.path != path
        || record.draft.as_ref().is_some_and(|draft| {
            draft
                .selected
                .as_ref()
                .is_some_and(|selected| receipt.imported_records != selected.len() as u64)
        })
    {
        return Err(Error::Conflict);
    }
    let profile = legacy::migration::verify(&path, &Default::default()).map_err(failure)?;
    if &profile.archive != archive.identity()
        || profile.materialized_records != Some(receipt.imported_records)
    {
        return Err(Error::Conflict);
    }
    candidate.verify_candidate(&receipt)?;
    root.verify().map_err(|_| Error::Conflict)?;
    archive.verify()?;
    Ok((candidate, receipt))
}
