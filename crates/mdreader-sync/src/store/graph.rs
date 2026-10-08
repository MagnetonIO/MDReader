//! Immutable encrypted dependencies and exact scoped upload receipts.
use super::graph_read::dependencies_on;
use super::upload::ScopedReceipt;
use super::*;
use crate::outgoing::SealedGraph;

/// Stored ciphertext only; no plaintext metadata or key material.
pub struct StoredDependency {
    pub(crate) context: EncryptionContext,
    pub blob: String,
    pub envelope_utf8: Vec<u8>,
    pub uploaded: bool,
}
impl SyncStore<'_> {
    pub fn dependencies(&self, operation: &str) -> Result<Vec<StoredDependency>> {
        validate_uuid(operation)?;
        dependencies_on(self.connection, &self.scope, operation)
    }
    pub fn require_dependencies_uploaded(&self, operation: &str) -> Result<()> {
        require_uploaded_on(self.connection, &self.scope, operation)
    }
    /// Called only after the authenticated exact account/device request returns.
    /// No compatibility fallback to an unscoped v1 upload receipt.
    pub fn mark_dependency_uploaded(
        &mut self,
        scope: &SyncScope,
        operation: &str,
        blob: &str,
        status: u16,
        body: &[u8],
    ) -> Result<()> {
        self.response_scope(scope, status, &[200, 201])?;
        validate_uuid(operation)?;
        validate_digest(blob)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        graph_schema::require(&tx)?;
        let expected = super::graph_read::dependency_on(&tx, scope, operation, blob)?;
        validate_receipt(scope, operation, blob, expected.envelope_utf8.len(), body)?;
        let receipt = std::str::from_utf8(body).map_err(|_| SyncError::Invalid)?;
        let previous: Option<String> = tx.query_row("SELECT receipt FROM mdreader_sync_graph_dependencies WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4 AND blob=?5 AND (receipt IS NULL OR length(CAST(receipt AS BLOB))<=4096)",params![scope.account_digest,scope.library_id,scope.device_id,operation,blob], |r|r.get(0))?;
        if previous.as_deref().is_some_and(|old| old != receipt) {
            return Err(SyncError::Reused);
        }
        if tx.execute("UPDATE mdreader_sync_graph_dependencies SET receipt=?1 WHERE account=?2 AND library=?3 AND device=?4 AND operation_id=?5 AND blob=?6",params![receipt,scope.account_digest,scope.library_id,scope.device_id,operation,blob])?!=1 {return Err(SyncError::NotReady);}
        tx.commit()?;
        Ok(())
    }
}
/// Caller owns the transaction, source/selection fences and full rollback.
/// Root admission must happen in this same transaction; no network is allowed.
pub(super) fn insert_resources(
    tx: &Transaction<'_>,
    scope: &SyncScope,
    limits: &SyncLimits,
    operation: &str,
    graph: &SealedGraph,
) -> Result<()> {
    scope.validate()?;
    limits.validate()?;
    validate_uuid(operation)?;
    graph.validate()?;
    if graph.root.context.library_id != scope.library_id {
        return Err(SyncError::Scope);
    }
    graph_schema::initialize(tx)?;
    let existing:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM mdreader_sync_graph_roots WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4)",params![scope.account_digest,scope.library_id,scope.device_id,operation],|r|r.get(0))?;
    if existing {
        let current = dependencies_on(tx, scope, operation)?;
        let root = entry_on(tx, scope, operation)?.ok_or(SyncError::Schema)?;
        if root.envelope_utf8.as_deref() != Some(graph.root.envelope_utf8.as_slice())
            || current.len() != graph.resources.len()
            || current.iter().zip(&graph.resources).any(|(a, b)| {
                a.context != b.context || a.blob != b.blob_id || a.envelope_utf8 != b.envelope_utf8
            })
        {
            return Err(SyncError::Reused);
        }
        return Ok(());
    }
    let charged = charged_bytes(tx)?;
    let roots: u64 = tx.query_row(
        "SELECT coalesce(sum(bytes),0) FROM mdreader_sync_outbox",
        [],
        |r| r.get(0),
    )?;
    let extra: u64 = graph
        .resources
        .iter()
        .map(|b| (b.envelope_utf8.len() + MAX_RECEIPT_BYTES + 1024) as u64)
        .sum::<u64>()
        + (MAX_RECEIPT_BYTES + 1024) as u64;
    if charged.saturating_add(roots).saturating_add(extra) > limits.max_outbox_bytes {
        return Err(SyncError::Bounds);
    }
    let count: u64 = tx.query_row("SELECT count(*) FROM mdreader_sync_nonces", [], |r| {
        r.get(0)
    })?;
    if count.saturating_add(graph.resources.len() as u64 + 1) > limits.max_nonce_reservations {
        return Err(SyncError::Bounds);
    }
    for body in std::iter::once(&graph.root).chain(&graph.resources) {
        // Only already constructed canonical envelopes enter here; the stored
        // reservation still rejects collision with every prior same-key nonce.
        let value: serde_json::Value = parse(&body.envelope_utf8, 512 * 1024)?;
        let nonce = decode_canonical(
            value
                .get("nonce")
                .and_then(serde_json::Value::as_str)
                .ok_or(SyncError::Invalid)?,
        )?;
        if nonce.len() != 12
            || tx.execute(
                "INSERT OR IGNORE INTO mdreader_sync_nonces(fingerprint,nonce) VALUES(?1,?2)",
                params![graph.key_fingerprint, nonce],
            )? != 1
        {
            return Err(SyncError::Reused);
        }
    }
    tx.execute(
        "INSERT INTO mdreader_sync_graph_roots VALUES(?1,?2,?3,?4,?5,?6,NULL)",
        params![
            scope.account_digest,
            scope.library_id,
            scope.device_id,
            operation,
            graph.root.blob_id,
            graph.resources.len()
        ],
    )?;
    for (ordinal, body) in graph.resources.iter().enumerate() {
        tx.execute(
            "INSERT INTO mdreader_sync_graph_dependencies VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,NULL)",
            params![
                scope.account_digest,
                scope.library_id,
                scope.device_id,
                operation,
                ordinal,
                body.blob_id,
                serde_json::to_string(&body.context).map_err(|_| SyncError::Invalid)?,
                body.envelope_utf8,
                body.envelope_utf8.len()
            ],
        )?;
    }
    Ok(())
}
pub(super) fn validate_receipt(
    scope: &SyncScope,
    operation: &str,
    blob: &str,
    bytes: usize,
    body: &[u8],
) -> Result<()> {
    let receipt: ScopedReceipt = parse(body, MAX_RECEIPT_BYTES)?;
    if receipt.v != 2
        || receipt.account != scope.account_digest
        || receipt.library_id != scope.library_id
        || receipt.device != scope.device_id
        || receipt.operation_id != operation
        || receipt.blob != blob
        || receipt.bytes != bytes as u64
    {
        return Err(SyncError::Scope);
    }
    Ok(())
}

/// Entire graph admission on the actual caller-owned transaction. No nested
/// transaction, commits or network. Caller rolls back the WHOLE transaction on
/// error and revalidates selected document/assets/authority immediately precommit.
/// All nonces, ciphertext and the caller's association become durable together.
pub fn enqueue_graph_in(
    tx: &Transaction<'_>,
    scope: &SyncScope,
    limits: &SyncLimits,
    operation: &str,
    parents: &[String],
    graph: &SealedGraph,
) -> Result<OutboxEntry> {
    scope.validate()?;
    limits.validate()?;
    graph.validate()?;
    validate_uuid(operation)?;
    validate_parents(parents, &graph.root.context.revision_id)?;
    let version: i64 = tx
        .query_row(
            "SELECT version FROM mdreader_sync_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .map_err(|_| SyncError::Schema)?;
    if version != VERSION {
        return Err(SyncError::Schema);
    }
    let synchronous: i64 = tx.query_row("PRAGMA synchronous", [], |r| r.get(0))?;
    let journal: String = tx.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    if synchronous < 2 || !matches!(journal.as_str(), "wal" | "delete" | "truncate" | "persist") {
        return Err(SyncError::NotReady);
    }
    if graph.root.context.library_id != scope.library_id {
        return Err(SyncError::Scope);
    }
    // Unsupported durable namespaces take precedence over replay fast paths.
    // Do not silently treat a future schema as an ordinary reused root.
    if graph_schema::present(tx)? {
        graph_schema::require(tx)?;
    }
    if entry_on(tx, scope, operation)?.is_some() && !has_root(tx, scope, operation)? {
        return Err(SyncError::Reused);
    }
    let entry = admission::enqueue_on(
        tx,
        scope,
        limits,
        CommitOperation {
            operation_id: operation.into(),
            item_id: graph.root.context.item_id.clone(),
            revision_id: graph.root.context.revision_id.clone(),
            parents: parents.to_vec(),
            blob: Some(graph.root.blob_id.clone()),
            deleted: false,
        },
        Some(&graph.root.envelope_utf8),
    )?;
    insert_resources(tx, scope, limits, operation, graph)?;
    Ok(entry)
}
pub(super) fn has_root(db: &Connection, scope: &SyncScope, operation: &str) -> Result<bool> {
    if !graph_schema::present(db)? {
        return Ok(false);
    }
    graph_schema::require(db)?;
    let (root, dependency): (bool, bool) = db.query_row("SELECT EXISTS(SELECT 1 FROM mdreader_sync_graph_roots WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4),EXISTS(SELECT 1 FROM mdreader_sync_graph_dependencies WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4)", params![scope.account_digest,scope.library_id,scope.device_id,operation], |r| Ok((r.get(0)?,r.get(1)?)))?;
    if dependency && !root {
        return Err(SyncError::Schema);
    }
    Ok(root)
}
pub(super) fn charged_bytes(db: &Connection) -> Result<u64> {
    if !graph_schema::present(db)? {
        return Ok(0);
    }
    graph_schema::require(db)?;
    Ok(db.query_row("SELECT (SELECT coalesce(sum(bytes+?1),0) FROM mdreader_sync_graph_dependencies)+(SELECT count(*)*?1 FROM mdreader_sync_graph_roots)", [MAX_RECEIPT_BYTES+1024], |r| r.get(0))?)
}
pub(super) fn require_uploaded_on(
    db: &Connection,
    scope: &SyncScope,
    operation: &str,
) -> Result<()> {
    if dependencies_on(db, scope, operation)?
        .iter()
        .any(|d| !d.uploaded)
    {
        return Err(SyncError::NotReady);
    }
    Ok(())
}
pub(super) fn save_root_receipt(
    tx: &Transaction<'_>,
    scope: &SyncScope,
    operation: &str,
    body: &[u8],
) -> Result<()> {
    if !has_root(tx, scope, operation)? {
        return Ok(());
    }
    require_uploaded_on(tx, scope, operation)?;
    let root = entry_on(tx, scope, operation)?.ok_or(SyncError::Schema)?;
    validate_receipt(
        scope,
        operation,
        root.operation.blob.as_deref().ok_or(SyncError::Schema)?,
        root.envelope_utf8.as_ref().ok_or(SyncError::Schema)?.len(),
        body,
    )?;
    let body = std::str::from_utf8(body).map_err(|_| SyncError::Invalid)?;
    let previous: Option<String> = tx.query_row("SELECT root_receipt FROM mdreader_sync_graph_roots WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4 AND (root_receipt IS NULL OR length(CAST(root_receipt AS BLOB))<=4096)", params![scope.account_digest,scope.library_id,scope.device_id,operation], |r| r.get(0))?;
    if previous.as_deref().is_some_and(|old| old != body) {
        return Err(SyncError::Reused);
    }
    tx.execute("UPDATE mdreader_sync_graph_roots SET root_receipt=?1 WHERE account=?2 AND library=?3 AND device=?4 AND operation_id=?5", params![body,scope.account_digest,scope.library_id,scope.device_id,operation])?;
    Ok(())
}

impl SyncStore<'_> {
    /// Only a graph's durable exact scoped receipt permits skipping root upload.
    /// Legacy uploaded state cannot provide that proof and returns false.
    pub fn graph_root_uploaded(&self, operation: &str) -> Result<bool> {
        validate_uuid(operation)?;
        root_uploaded_on(self.connection, &self.scope, operation)
    }
}
fn root_uploaded_on(db: &Connection, scope: &SyncScope, operation: &str) -> Result<bool> {
    if !has_root(db, scope, operation)? {
        return Ok(false);
    }
    let receipt: Option<String> = db.query_row("SELECT root_receipt FROM mdreader_sync_graph_roots WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4 AND (root_receipt IS NULL OR length(CAST(root_receipt AS BLOB))<=4096)", params![scope.account_digest,scope.library_id,scope.device_id,operation], |r| r.get(0))?;
    let entry = entry_on(db, scope, operation)?.ok_or(SyncError::Schema)?;
    if let Some(receipt) = receipt {
        if entry.state == OutboxState::Pending {
            return Err(SyncError::Schema);
        }
        validate_receipt(
            scope,
            operation,
            entry.operation.blob.as_deref().ok_or(SyncError::Schema)?,
            entry.envelope_utf8.as_ref().ok_or(SyncError::Schema)?.len(),
            receipt.as_bytes(),
        )?;
        require_uploaded_on(db, scope, operation)?;
        Ok(true)
    } else if entry.state != OutboxState::Pending {
        Err(SyncError::Schema)
    } else {
        Ok(false)
    }
}
pub(super) fn require_published_on(
    db: &Connection,
    scope: &SyncScope,
    operation: &str,
) -> Result<()> {
    if has_root(db, scope, operation)? && !root_uploaded_on(db, scope, operation)? {
        return Err(SyncError::NotReady);
    }
    // root_uploaded_on already checks every dependency once. An absent graph is
    // legacy, while has_root rejects orphan dependencies before this fast path.
    Ok(())
}
