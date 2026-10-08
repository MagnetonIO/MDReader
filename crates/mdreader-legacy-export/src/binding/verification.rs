use super::*;
use crate::{
    hex,
    reviews::{interpret, text, Assertion, ReviewArchive},
    KnownRecord, SqlValue,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(super) fn verify(
    archive: &ReviewArchive,
    selection: &SelectedBinding,
    reader: &impl CurrentDocumentReader,
) -> Result<BindingVerification, BindingError> {
    check_owner(reader, selection)?;
    let stored = archive
        .get(&selection.record_id)
        .map_err(|_| BindingError::ArchiveUnavailable)?
        .ok_or(BindingError::ArchiveUnavailable)?;
    if stored.raw.archive != selection.archive {
        return Err(BindingError::ArchiveMismatch);
    }
    // Never take a caller-supplied interpretation as the archival authority.
    let bytes = archive
        .raw_record(&selection.record_id)
        .map_err(|_| BindingError::ArchiveUnavailable)?
        .ok_or(BindingError::ArchiveUnavailable)?;
    if hex(&Sha256::digest(&bytes)) != stored.raw.row_sha256 {
        return Err(BindingError::ArchiveUnavailable);
    }
    let KnownRecord::Proposal { row_id, fields } =
        serde_json::from_slice(&bytes).map_err(|_| BindingError::ArchiveUnavailable)?
    else {
        return Err(BindingError::ArchiveUnavailable);
    };
    let id_bytes = serde_json::to_vec(&(&selection.archive, stored.raw.artifact_index, row_id))
        .map_err(|_| BindingError::ArchiveUnavailable)?;
    if row_id.to_string() != stored.raw.row_id
        || hex(&Sha256::digest(&id_bytes)) != selection.record_id
    {
        return Err(BindingError::ArchiveUnavailable);
    }
    let recorded =
        hash_field(&fields, "content_hash", false)?.ok_or(BindingError::UnsupportedHash)?;
    let reviewed = hash_field(&fields, "reviewed_hash", true)?;
    let archival_review = interpret(&fields, stored.raw, selection.record_id.clone());
    let Assertion::DocumentSpan(span) = &archival_review.assertion else {
        return Err(BindingError::UnsupportedAssertion);
    };

    let current = reader.read_exact(&selection.current)?;
    if current.reference != selection.current {
        return Err(BindingError::ReferenceMismatch);
    }
    let current_span_hash = span_hash(&current.markdown, span.start_line, span.end_line)?;
    let recorded_current = compare(Some(&recorded), &current_span_hash);
    let reviewed_current = compare(reviewed.as_deref(), &current_span_hash);

    // Comparisons are not returned even when text differs if access/revision
    // changed during the read or hashing. The archive guard must also still hold.
    reader.revalidate_exact(&selection.current)?;
    check_owner(reader, selection)?;
    let final_archival = archive
        .get(&selection.record_id)
        .map_err(|_| BindingError::ArchiveUnavailable)?
        .ok_or(BindingError::ArchiveUnavailable)?;
    if final_archival.raw.archive != selection.archive
        || final_archival.raw.row_sha256 != archival_review.raw.row_sha256
    {
        return Err(BindingError::ArchiveUnavailable);
    }
    check_owner(reader, selection)?;
    Ok(BindingVerification {
        archival_review,
        selected_current: selection.current.clone(),
        observation: current.observation,
        scope: ComparisonScope::SelectedOwnerRevision,
        current_span_hash,
        recorded_current,
        reviewed_current,
    })
}

fn check_owner(
    reader: &impl CurrentDocumentReader,
    selection: &SelectedBinding,
) -> Result<(), BindingError> {
    if reader.owner_scope() != selection.current.owner_scope() {
        return Err(BindingError::OwnerMismatch);
    }
    Ok(())
}

fn hash_field(
    fields: &BTreeMap<String, SqlValue>,
    name: &str,
    optional: bool,
) -> Result<Option<String>, BindingError> {
    if optional && matches!(fields.get(name), None | Some(SqlValue::Null)) {
        return Ok(None);
    }
    let value = text(fields, name).ok_or(BindingError::UnsupportedHash)?;
    if !value.strip_prefix("blake3:").is_some_and(types::hex_digest) {
        return Err(BindingError::UnsupportedHash);
    }
    Ok(Some(value))
}

fn compare(expected: Option<&str>, actual: &str) -> CurrentHashComparison {
    match expected {
        None => CurrentHashComparison::Unknown,
        Some(expected) if expected.eq_ignore_ascii_case(actual) => CurrentHashComparison::Matches,
        Some(_) => CurrentHashComparison::Differs,
    }
}

/// Exactly the producer's str::lines + LF join, without allocating a vector or
/// joined span. Unlike historical extraction, absent current lines are rejected.
fn span_hash(markdown: &str, start: u32, end: u32) -> Result<String, BindingError> {
    let count = end
        .checked_sub(start)
        .and_then(|n| n.checked_add(1))
        .filter(|_| start > 0)
        .ok_or(BindingError::SpanUnavailable)? as usize;
    let mut hasher = blake3::Hasher::new();
    let mut observed = 0;
    for line in markdown.lines().skip((start - 1) as usize).take(count) {
        if observed > 0 {
            hasher.update(b"\n");
        }
        hasher.update(line.as_bytes());
        observed += 1;
    }
    if observed != count {
        return Err(BindingError::SpanUnavailable);
    }
    Ok(format!("blake3:{}", hasher.finalize().to_hex()))
}
