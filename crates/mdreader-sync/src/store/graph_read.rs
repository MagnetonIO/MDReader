//! Bounded durable graph reads. No malformed graph degrades to legacy admission.
use super::graph::{has_root, validate_receipt, StoredDependency};
use super::*;

pub(super) fn dependencies_on(
    db: &Connection,
    scope: &SyncScope,
    operation: &str,
) -> Result<Vec<StoredDependency>> {
    if !has_root(db, scope, operation)? {
        return Ok(Vec::new());
    }
    let (root_blob, count): (String, usize) = db.query_row(
        "SELECT root_blob,dependency_count FROM mdreader_sync_graph_roots WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4 AND length(CAST(root_blob AS BLOB))=64",
        params![scope.account_digest,scope.library_id,scope.device_id,operation],
        |r| Ok((r.get(0)?,r.get(1)?)),
    ).map_err(|_| SyncError::Schema)?;
    validate_digest(&root_blob)?;
    if count == 0 || count > crate::codec::MAX_RESOURCES {
        return Err(SyncError::Schema);
    }
    let root = entry_on(db, scope, operation)?.ok_or(SyncError::Schema)?;
    let root_body = root.envelope_utf8.as_deref().ok_or(SyncError::Schema)?;
    if root.operation.deleted
        || root.operation.blob.as_deref() != Some(root_blob.as_str())
        || root_body.len() > 512 * 1024
    {
        return Err(SyncError::Schema);
    }
    let key = crate::crypto::envelope_key_id(root_body)?;
    // Preflight aggregate and byte/type bounds while SQLite still owns values.
    // TEXT length() counts characters, so every text bound explicitly uses bytes.
    let (actual_count, bytes, valid): (usize, usize, bool) = db.query_row(
        "SELECT count(*),coalesce(sum(length(CAST(envelope AS BLOB))),0),coalesce(min(typeof(blob)='text' AND length(CAST(blob AS BLOB))=64 AND typeof(context)='text' AND length(CAST(context AS BLOB))<=1024 AND typeof(envelope)='blob' AND length(envelope)=bytes AND bytes BETWEEN 1 AND 524288 AND (receipt IS NULL OR (typeof(receipt)='text' AND length(CAST(receipt AS BLOB))<=4096))),1) FROM mdreader_sync_graph_dependencies WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4",
        params![scope.account_digest,scope.library_id,scope.device_id,operation],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    )?;
    if actual_count != count || !valid || bytes.saturating_add(root_body.len()) > 32 * 1024 * 1024 {
        return Err(SyncError::Schema);
    }
    let mut statement = db.prepare("SELECT ordinal,blob,context,envelope,bytes,receipt FROM mdreader_sync_graph_dependencies WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4 AND length(CAST(blob AS BLOB))=64 AND length(CAST(context AS BLOB))<=1024 AND length(CAST(envelope AS BLOB))<=524288 AND (receipt IS NULL OR length(CAST(receipt AS BLOB))<=4096) ORDER BY ordinal LIMIT 73")?;
    let rows = statement
        .query_map(
            params![
                scope.account_digest,
                scope.library_id,
                scope.device_id,
                operation
            ],
            |r| {
                Ok((
                    r.get::<_, usize>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Vec<u8>>(3)?,
                    r.get::<_, usize>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len() != count {
        return Err(SyncError::Schema);
    }
    let mut result = Vec::with_capacity(count);
    let mut items = std::collections::HashSet::from([root.operation.item_id]);
    let mut revisions = std::collections::HashSet::from([root.operation.revision_id]);
    for (expected, (ordinal, blob, context, envelope, bytes, receipt)) in
        rows.into_iter().enumerate()
    {
        if ordinal != expected || envelope.len() != bytes || crate::blob_digest(&envelope) != blob {
            return Err(SyncError::Schema);
        }
        validate_digest(&blob)?;
        let context: EncryptionContext = parse(context.as_bytes(), 1024)?;
        context.validate()?;
        if context.library_id != scope.library_id
            || context.key_id != key
            || crate::crypto::envelope_key_id(&envelope)? != key
        {
            return Err(SyncError::Scope);
        }
        if !items.insert(context.item_id.clone()) || !revisions.insert(context.revision_id.clone())
        {
            return Err(SyncError::Schema);
        }
        if let Some(receipt) = &receipt {
            validate_receipt(scope, operation, &blob, bytes, receipt.as_bytes())?;
        }
        result.push(StoredDependency {
            context,
            blob,
            envelope_utf8: envelope,
            uploaded: receipt.is_some(),
        });
    }
    Ok(result)
}

/// Receipt admission reads only this immutable row, not all sibling bodies. Full
/// graph integrity remains mandatory at root publication and commit. This avoids
/// quadratic body hashing/allocation while uploading a bounded multi-chunk graph.
pub(super) fn dependency_on(
    db: &Connection,
    scope: &SyncScope,
    operation: &str,
    blob: &str,
) -> Result<StoredDependency> {
    if !has_root(db, scope, operation)? {
        return Err(SyncError::NotReady);
    }
    let count:usize=db.query_row("SELECT dependency_count FROM mdreader_sync_graph_roots WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4 AND length(CAST(root_blob AS BLOB))=64",params![scope.account_digest,scope.library_id,scope.device_id,operation],|r|r.get(0)).map_err(|_|SyncError::Schema)?;
    if count == 0 || count > crate::codec::MAX_RESOURCES {
        return Err(SyncError::Schema);
    }
    let valid:Option<bool>=db.query_row("SELECT typeof(ordinal)='integer' AND ordinal BETWEEN 0 AND ?6-1 AND typeof(blob)='text' AND length(CAST(blob AS BLOB))=64 AND typeof(context)='text' AND length(CAST(context AS BLOB))<=1024 AND typeof(envelope)='blob' AND length(envelope)=bytes AND bytes BETWEEN 1 AND 524288 AND (receipt IS NULL OR (typeof(receipt)='text' AND length(CAST(receipt AS BLOB))<=4096)) FROM mdreader_sync_graph_dependencies WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4 AND blob=?5",params![scope.account_digest,scope.library_id,scope.device_id,operation,blob,count],|r|r.get(0)).optional()?;
    match valid {
        Some(true) => {}
        Some(false) => return Err(SyncError::Schema),
        None => return Err(SyncError::NotReady),
    }
    let (context,envelope,bytes,receipt):(String,Vec<u8>,usize,Option<String>)=db.query_row("SELECT context,envelope,bytes,receipt FROM mdreader_sync_graph_dependencies WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4 AND blob=?5 AND length(CAST(context AS BLOB))<=1024 AND length(CAST(envelope AS BLOB))<=524288 AND (receipt IS NULL OR length(CAST(receipt AS BLOB))<=4096)",params![scope.account_digest,scope.library_id,scope.device_id,operation,blob],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    if envelope.len() != bytes || crate::blob_digest(&envelope) != blob {
        return Err(SyncError::Schema);
    }
    let context: EncryptionContext = parse(context.as_bytes(), 1024)?;
    context.validate()?;
    if context.library_id != scope.library_id
        || crate::crypto::envelope_key_id(&envelope)? != context.key_id
    {
        return Err(SyncError::Scope);
    }
    if let Some(receipt) = &receipt {
        validate_receipt(scope, operation, blob, bytes, receipt.as_bytes())?;
    }
    Ok(StoredDependency {
        context,
        blob: blob.into(),
        envelope_utf8: envelope,
        uploaded: receipt.is_some(),
    })
}
