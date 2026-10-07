use crate::{
    crypto::{decode_canonical, encrypt_with_nonce, random_nonce, validate_digest, validate_uuid},
    *,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
const VERSION: i64 = 1;
const MAX_RECEIPT_BYTES: usize = 4096;
const MAX_PAGE_BYTES: usize = 256 * 1024;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncScope {
    account_digest: String,
    library_id: String,
    device_id: String,
}
impl SyncScope {
    pub fn new(account_digest: String, library_id: String, device_id: String) -> Result<Self> {
        validate_digest(&account_digest)?;
        validate_uuid(&library_id)?;
        validate_device(&device_id)?;
        Ok(Self {
            account_digest,
            library_id,
            device_id,
        })
    }
    pub fn account_digest(&self) -> &str {
        &self.account_digest
    }
    pub fn library_id(&self) -> &str {
        &self.library_id
    }
    pub fn device_id(&self) -> &str {
        &self.device_id
    }
    fn validate(&self) -> Result<()> {
        validate_digest(&self.account_digest)?;
        validate_uuid(&self.library_id)?;
        validate_device(&self.device_id)
    }
}
fn validate_device(value: &str) -> Result<()> {
    if value.len() != 43 || decode_canonical(value)?.len() != 32 {
        return Err(SyncError::Invalid);
    }
    Ok(())
}
#[derive(Debug, Clone)]
pub struct SyncLimits {
    pub max_outbox_entries: u64,
    pub max_outbox_bytes: u64,
    pub max_remote_revisions: u64,
    pub max_remote_bytes: u64,
    pub max_nonce_reservations: u64,
}
impl Default for SyncLimits {
    fn default() -> Self {
        Self {
            max_outbox_entries: 10_000,
            max_outbox_bytes: 64 * 1024 * 1024,
            max_remote_revisions: 100_000,
            max_remote_bytes: 256 * 1024 * 1024,
            max_nonce_reservations: 100_000,
        }
    }
}
impl SyncLimits {
    fn validate(&self) -> Result<()> {
        if self.max_outbox_entries == 0
            || self.max_outbox_entries > 10_000
            || self.max_outbox_bytes == 0
            || self.max_outbox_bytes > 256 * 1024 * 1024
            || self.max_remote_revisions == 0
            || self.max_remote_revisions > 100_000
            || self.max_remote_bytes == 0
            || self.max_remote_bytes > 1024 * 1024 * 1024
            || self.max_nonce_reservations == 0
            || self.max_nonce_reservations > 1_000_000
        {
            return Err(SyncError::Bounds);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommitOperation {
    pub operation_id: String,
    pub item_id: String,
    pub revision_id: String,
    pub parents: Vec<String>,
    #[serde(deserialize_with = "required_blob")]
    pub blob: Option<String>,
    pub deleted: bool,
}
impl CommitOperation {
    fn validate(&self) -> Result<()> {
        validate_uuid(&self.operation_id)?;
        validate_uuid(&self.item_id)?;
        validate_uuid(&self.revision_id)?;
        validate_parents(&self.parents, &self.revision_id)?;
        match (&self.blob, self.deleted) {
            (Some(blob), false) => validate_digest(blob)?,
            (None, true) => {}
            _ => return Err(SyncError::Invalid),
        }
        Ok(())
    }
    pub fn wire_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        encode(self)
    }
}
fn validate_parents(parents: &[String], revision: &str) -> Result<()> {
    if parents.len() > 8 {
        return Err(SyncError::Bounds);
    }
    let mut unique = std::collections::HashSet::new();
    for p in parents {
        validate_uuid(p)?;
        if p == revision || !unique.insert(p) {
            return Err(SyncError::Invalid);
        }
    }
    Ok(())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxState {
    Pending,
    Uploaded,
    Acknowledged,
}
impl OutboxState {
    fn sql(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Uploaded => "uploaded",
            Self::Acknowledged => "acknowledged",
        }
    }
    fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "uploaded" => Ok(Self::Uploaded),
            "acknowledged" => Ok(Self::Acknowledged),
            _ => Err(SyncError::Schema),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommitReceipt {
    pub operation_id: String,
    pub revision_id: String,
    pub sequence: u64,
    pub conflict: bool,
    pub heads: Vec<String>,
}
impl CommitReceipt {
    fn validate(&mut self) -> Result<()> {
        validate_uuid(&self.operation_id)?;
        validate_uuid(&self.revision_id)?;
        validate_sequence(self.sequence, false)?;
        if self.heads.is_empty()
            || self.heads.len() > 8
            || self.conflict != (self.heads.len() > 1)
            || !self.heads.contains(&self.revision_id)
        {
            return Err(SyncError::Invalid);
        }
        let mut ids = std::collections::HashSet::new();
        for h in &self.heads {
            validate_uuid(h)?;
            if !ids.insert(h) {
                return Err(SyncError::Invalid);
            }
        }
        self.heads.sort();
        Ok(())
    }
}
#[derive(Debug, Clone)]
pub struct OutboxEntry {
    pub operation: CommitOperation,
    pub envelope_utf8: Option<Vec<u8>>,
    pub state: OutboxState,
    pub receipt: Option<CommitReceipt>,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorState {
    pub received: u64,
    pub applied: u64,
    pub server_acknowledged: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadReceipt {
    blob: String,
    bytes: u64,
}

/// Separate migration namespace: never changes the reader's PRAGMA user_version.
/// The supplied handle is the service owner's connection; this crate never opens a database.
pub fn migrate(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing: u64 = transaction.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name GLOB 'mdreader_sync_*'",
        [],
        |r| r.get(0),
    )?;
    if existing > 0 {
        let version = transaction
            .query_row(
                "SELECT version FROM mdreader_sync_meta WHERE singleton=1",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|_| SyncError::Schema)?;
        if version != VERSION {
            return Err(SyncError::Schema);
        }
        for sql in ["SELECT account,library,device,operation_id,operation,envelope,state,receipt,bytes FROM mdreader_sync_outbox LIMIT 0","SELECT fingerprint,nonce FROM mdreader_sync_nonces LIMIT 0","SELECT account,library,device,received,applied,acked FROM mdreader_sync_cursors LIMIT 0","SELECT account,library,device,sequence,revision,item,blob,deleted,parents,source_device,envelope,bytes FROM mdreader_sync_remote LIMIT 0","SELECT account,library,device,item,revision FROM mdreader_sync_heads LIMIT 0"] {transaction.prepare(sql).map_err(|_|SyncError::Schema)?;}
        transaction.commit()?;
        return Ok(());
    }
    transaction.execute_batch("CREATE TABLE mdreader_sync_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL);
      CREATE TABLE mdreader_sync_outbox(sequence INTEGER PRIMARY KEY AUTOINCREMENT,account TEXT NOT NULL,library TEXT NOT NULL,device TEXT NOT NULL,operation_id TEXT NOT NULL,revision_id TEXT NOT NULL,operation TEXT NOT NULL,envelope BLOB,state TEXT NOT NULL CHECK(state IN ('pending','uploaded','acknowledged')),receipt TEXT,bytes INTEGER NOT NULL CHECK(bytes>=0),UNIQUE(account,operation_id),UNIQUE(account,revision_id));
      CREATE INDEX mdreader_sync_outbox_scope ON mdreader_sync_outbox(account,library,device,state,sequence);
      CREATE TABLE mdreader_sync_nonces(fingerprint TEXT NOT NULL,nonce BLOB NOT NULL CHECK(length(nonce)=12),PRIMARY KEY(fingerprint,nonce));
      CREATE TABLE mdreader_sync_cursors(account TEXT NOT NULL,library TEXT NOT NULL,device TEXT NOT NULL,received INTEGER NOT NULL DEFAULT 0,applied INTEGER NOT NULL DEFAULT 0,acked INTEGER NOT NULL DEFAULT 0,CHECK(acked>=0 AND acked<=applied AND applied<=received),PRIMARY KEY(account,library,device));
      CREATE TABLE mdreader_sync_remote(account TEXT NOT NULL,library TEXT NOT NULL,device TEXT NOT NULL,sequence INTEGER NOT NULL,revision TEXT NOT NULL,item TEXT NOT NULL,blob TEXT,deleted INTEGER NOT NULL CHECK(deleted IN (0,1)),parents TEXT NOT NULL,source_device TEXT NOT NULL,envelope BLOB,bytes INTEGER NOT NULL CHECK(bytes>=0),PRIMARY KEY(account,library,device,revision),UNIQUE(account,library,device,sequence));
      CREATE INDEX mdreader_sync_remote_item ON mdreader_sync_remote(account,library,device,item,sequence);
      CREATE TABLE mdreader_sync_heads(account TEXT NOT NULL,library TEXT NOT NULL,device TEXT NOT NULL,item TEXT NOT NULL,revision TEXT NOT NULL,PRIMARY KEY(account,library,device,item,revision));
      INSERT INTO mdreader_sync_meta VALUES(1,1);")?;
    transaction.commit()?;
    Ok(())
}

pub struct SyncStore<'a> {
    connection: &'a mut Connection,
    scope: SyncScope,
    limits: SyncLimits,
}
impl<'a> SyncStore<'a> {
    pub fn attach(connection: &'a mut Connection, scope: SyncScope) -> Result<Self> {
        Self::with_limits(connection, scope, SyncLimits::default())
    }
    pub fn with_limits(
        connection: &'a mut Connection,
        scope: SyncScope,
        limits: SyncLimits,
    ) -> Result<Self> {
        scope.validate()?;
        limits.validate()?;
        let version = connection
            .query_row(
                "SELECT version FROM mdreader_sync_meta WHERE singleton=1",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|_| SyncError::Schema)?;
        if version != VERSION {
            return Err(SyncError::Schema);
        }
        let synchronous: i64 = connection.query_row("PRAGMA synchronous", [], |r| r.get(0))?;
        let journal: String = connection.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        if synchronous < 2 || !matches!(journal.as_str(), "wal" | "delete" | "truncate" | "persist")
        {
            return Err(SyncError::NotReady);
        }
        Ok(Self {
            connection,
            scope,
            limits,
        })
    }
    pub fn scope(&self) -> &SyncScope {
        &self.scope
    }
    fn response_scope(&self, scope: &SyncScope, status: u16, allowed: &[u16]) -> Result<()> {
        scope.validate()?;
        if scope != &self.scope {
            return Err(SyncError::Scope);
        }
        if !allowed.contains(&status) {
            return Err(SyncError::NotReady);
        }
        Ok(())
    }
    pub fn entry(&self, operation_id: &str) -> Result<Option<OutboxEntry>> {
        validate_uuid(operation_id)?;
        entry_on(self.connection, &self.scope, operation_id)
    }
    pub fn pending(&self, limit: usize) -> Result<Vec<OutboxEntry>> {
        if limit == 0 || limit > 100 {
            return Err(SyncError::Bounds);
        }
        let mut statement=self.connection.prepare("SELECT operation_id FROM mdreader_sync_outbox WHERE account=?1 AND library=?2 AND device=?3 AND state<>'acknowledged' ORDER BY sequence LIMIT ?4")?;
        let ids = statement
            .query_map(
                params![
                    self.scope.account_digest,
                    self.scope.library_id,
                    self.scope.device_id,
                    limit
                ],
                |r| r.get::<_, String>(0),
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| entry_on(self.connection, &self.scope, &id)?.ok_or(SyncError::Schema))
            .collect()
    }
    /// Retrying an operation reuses its durable ciphertext rather than generating a nonce again.
    pub fn seal_enqueue(
        &mut self,
        operation_id: &str,
        parents: &[String],
        key: &LibraryKey,
        context: &EncryptionContext,
        plaintext: &[u8],
    ) -> Result<OutboxEntry> {
        context.validate()?;
        validate_uuid(operation_id)?;
        validate_parents(parents, &context.revision_id)?;
        if context.library_id != self.scope.library_id || context.key_id != key.key_id() {
            return Err(SyncError::Scope);
        }
        if plaintext.len() > MAX_PLAINTEXT_BYTES {
            return Err(SyncError::Bounds);
        }
        let mut canonical_parents = parents.to_vec();
        canonical_parents.sort();
        if let Some(existing) = self.entry(operation_id)? {
            if existing.operation.deleted
                || existing.operation.item_id != context.item_id
                || existing.operation.revision_id != context.revision_id
                || existing.operation.parents != canonical_parents
            {
                return Err(SyncError::Reused);
            }
            let body = decrypt(
                key,
                context,
                existing.envelope_utf8.as_ref().ok_or(SyncError::Schema)?,
            )?;
            if body.as_slice() != plaintext {
                return Err(SyncError::Reused);
            }
            return Ok(existing);
        }
        // Reservation commits before encryption. Failed enqueue consumes the nonce safely.
        let nonce = self.reserve_nonce(key)?;
        let prepared = encrypt_with_nonce(key, context, plaintext, nonce)?;
        self.enqueue_prepared(operation_id, parents, &prepared)
    }
    fn reserve_nonce(&mut self, key: &LibraryKey) -> Result<[u8; 12]> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count: u64 =
            transaction.query_row("SELECT count(*) FROM mdreader_sync_nonces", [], |r| {
                r.get(0)
            })?;
        if count >= self.limits.max_nonce_reservations {
            return Err(SyncError::Bounds);
        }
        let fingerprint = key.fingerprint();
        for _ in 0..32 {
            let nonce = random_nonce()?;
            let inserted = transaction.execute(
                "INSERT OR IGNORE INTO mdreader_sync_nonces(fingerprint,nonce) VALUES(?1,?2)",
                params![fingerprint, nonce.as_slice()],
            )?;
            if inserted == 1 {
                transaction.commit()?;
                return Ok(nonce);
            }
        }
        Err(SyncError::Entropy)
    }
    pub fn enqueue_prepared(
        &mut self,
        operation_id: &str,
        parents: &[String],
        prepared: &PreparedRevision,
    ) -> Result<OutboxEntry> {
        if prepared.context.library_id != self.scope.library_id {
            return Err(SyncError::Scope);
        }
        prepared.context.validate()?;
        let operation = CommitOperation {
            operation_id: operation_id.into(),
            item_id: prepared.context.item_id.clone(),
            revision_id: prepared.context.revision_id.clone(),
            parents: parents.to_vec(),
            blob: Some(prepared.blob_id.clone()),
            deleted: false,
        };
        self.enqueue(operation, Some(&prepared.envelope_utf8))
    }
    pub fn enqueue_tombstone(
        &mut self,
        operation_id: &str,
        item_id: &str,
        revision_id: &str,
        parents: &[String],
    ) -> Result<OutboxEntry> {
        self.enqueue(
            CommitOperation {
                operation_id: operation_id.into(),
                item_id: item_id.into(),
                revision_id: revision_id.into(),
                parents: parents.to_vec(),
                blob: None,
                deleted: true,
            },
            None,
        )
    }
    fn enqueue(
        &mut self,
        mut operation: CommitOperation,
        envelope: Option<&[u8]>,
    ) -> Result<OutboxEntry> {
        operation.validate()?;
        operation.parents.sort();
        validate_encrypted_body(&operation, envelope)?;
        let canonical =
            String::from_utf8(operation.wire_bytes()?).map_err(|_| SyncError::Invalid)?;
        let bytes = (canonical.len() + envelope.map_or(0, <[u8]>::len) + MAX_RECEIPT_BYTES) as u64;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(old) = entry_on(&transaction, &self.scope, &operation.operation_id)? {
            if old.operation != operation || old.envelope_utf8.as_deref() != envelope {
                return Err(SyncError::Reused);
            }
            transaction.commit()?;
            return Ok(old);
        }
        let reused:u64=transaction.query_row("SELECT count(*) FROM mdreader_sync_outbox WHERE account=?1 AND (operation_id=?2 OR revision_id=?3)",params![self.scope.account_digest,operation.operation_id,operation.revision_id],|r|r.get(0))?;
        if reused > 0 {
            return Err(SyncError::Reused);
        }
        let (count, total): (u64, u64) = transaction.query_row(
            "SELECT count(*),coalesce(sum(bytes),0) FROM mdreader_sync_outbox",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if count >= self.limits.max_outbox_entries
            || total.saturating_add(bytes) > self.limits.max_outbox_bytes
        {
            return Err(SyncError::Bounds);
        }
        let state = if operation.deleted {
            OutboxState::Uploaded
        } else {
            OutboxState::Pending
        };
        transaction.execute("INSERT INTO mdreader_sync_outbox(account,library,device,operation_id,revision_id,operation,envelope,state,receipt,bytes) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL,?9)",params![self.scope.account_digest,self.scope.library_id,self.scope.device_id,operation.operation_id,operation.revision_id,canonical,envelope,state.sql(),bytes])?;
        transaction.commit()?;
        Ok(OutboxEntry {
            operation,
            envelope_utf8: envelope.map(<[u8]>::to_vec),
            state,
            receipt: None,
        })
    }
    /// Caller must bind response_scope to its authenticated account/device HTTP channel.
    /// This validates the protocol receipt; it does not authenticate a network connection.
    pub fn mark_uploaded(
        &mut self,
        response_scope: &SyncScope,
        operation_id: &str,
        status: u16,
        bytes: &[u8],
    ) -> Result<()> {
        self.response_scope(response_scope, status, &[200, 201])?;
        validate_uuid(operation_id)?;
        let receipt: UploadReceipt = parse(bytes, MAX_RECEIPT_BYTES)?;
        validate_digest(&receipt.blob)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let entry =
            entry_on(&transaction, &self.scope, operation_id)?.ok_or(SyncError::NotReady)?;
        if entry.operation.deleted
            || entry.operation.blob.as_deref() != Some(receipt.blob.as_str())
            || entry.envelope_utf8.as_ref().map(|b| b.len() as u64) != Some(receipt.bytes)
        {
            return Err(SyncError::Authentication);
        }
        transaction.execute("UPDATE mdreader_sync_outbox SET state='uploaded' WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4 AND state='pending'",params![self.scope.account_digest,self.scope.library_id,self.scope.device_id,operation_id])?;
        transaction.commit()?;
        Ok(())
    }
    pub fn commit_receipt(
        &mut self,
        response_scope: &SyncScope,
        status: u16,
        bytes: &[u8],
    ) -> Result<CommitReceipt> {
        self.response_scope(response_scope, status, &[200, 201])?;
        let mut receipt: CommitReceipt = parse(bytes, MAX_RECEIPT_BYTES)?;
        receipt.validate()?;
        let canonical = String::from_utf8(encode(&receipt)?).map_err(|_| SyncError::Invalid)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let entry = entry_on(&transaction, &self.scope, &receipt.operation_id)?
            .ok_or(SyncError::NotReady)?;
        if entry.state == OutboxState::Pending {
            return Err(SyncError::NotReady);
        }
        if entry.operation.revision_id != receipt.revision_id {
            return Err(SyncError::Authentication);
        }
        if entry.receipt.as_ref().is_some_and(|old| old != &receipt) {
            return Err(SyncError::Reused);
        }
        transaction.execute("UPDATE mdreader_sync_outbox SET state='acknowledged',receipt=?1 WHERE account=?2 AND library=?3 AND device=?4 AND operation_id=?5",params![canonical,self.scope.account_digest,self.scope.library_id,self.scope.device_id,receipt.operation_id])?;
        transaction.commit()?;
        Ok(receipt)
    }
    pub fn cursors(&self) -> Result<CursorState> {
        cursors_on(self.connection, &self.scope)
    }
    /// Durably stage authenticated encrypted bodies and opaque revision metadata.
    /// This advances received only. Application and server acknowledgment are separate.
    pub fn stage_changes(
        &mut self,
        response_scope: &SyncScope,
        status: u16,
        after: u64,
        bytes: &[u8],
        prepared: &[PreparedRevision],
    ) -> Result<CursorState> {
        self.response_scope(response_scope, status, &[200])?;
        validate_sequence(after, true)?;
        let mut page: ChangesPage = parse(bytes, MAX_PAGE_BYTES)?;
        page.validate(after)?;
        if prepared.len() > 100 {
            return Err(SyncError::Bounds);
        }
        let mut bodies = std::collections::HashMap::new();
        for body in prepared {
            body.context.validate()?;
            if body.context.library_id != self.scope.library_id
                || bodies
                    .insert(body.context.revision_id.as_str(), body)
                    .is_some()
            {
                return Err(SyncError::Scope);
            }
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = cursors_on(&transaction, &self.scope)?;
        if after > current.received || page.high_water < current.received {
            return Err(SyncError::NotReady);
        }
        let (mut count, mut total): (u64, u64) = transaction.query_row(
            "SELECT count(*),coalesce(sum(bytes),0) FROM mdreader_sync_remote",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        for change in &page.changes {
            if let Some(old) = remote_on(&transaction, &self.scope, &change.revision_id)? {
                if old.change != *change {
                    return Err(SyncError::Reused);
                }
                // Replayed history never re-adds a head consumed by a later revision.
                continue;
            }
            if change.sequence <= current.received {
                return Err(SyncError::Schema);
            }
            let envelope = if change.deleted {
                None
            } else {
                let body = bodies
                    .get(change.revision_id.as_str())
                    .ok_or(SyncError::NotReady)?;
                if body.context.item_id != change.item_id
                    || body.blob_id.as_str() != change.blob.as_deref().ok_or(SyncError::Invalid)?
                {
                    return Err(SyncError::Authentication);
                }
                Some(body.envelope_utf8.as_slice())
            };
            let parents =
                String::from_utf8(encode(&change.parents)?).map_err(|_| SyncError::Invalid)?;
            let charged = (encode(change)?.len() + envelope.map_or(0, <[u8]>::len)) as u64;
            if count >= self.limits.max_remote_revisions
                || total.saturating_add(charged) > self.limits.max_remote_bytes
            {
                return Err(SyncError::Bounds);
            }
            let item_count:u64=transaction.query_row("SELECT count(*) FROM mdreader_sync_remote WHERE account=?1 AND library=?2 AND device=?3 AND item=?4",params![self.scope.account_digest,self.scope.library_id,self.scope.device_id,change.item_id],|r|r.get(0))?;
            if item_count >= 256 {
                return Err(SyncError::Bounds);
            }
            for parent in &change.parents {
                let item:Option<String>=transaction.query_row("SELECT item FROM mdreader_sync_remote WHERE account=?1 AND library=?2 AND device=?3 AND revision=?4",params![self.scope.account_digest,self.scope.library_id,self.scope.device_id,parent],|r|r.get(0)).optional()?;
                if item.as_deref() != Some(change.item_id.as_str()) {
                    return Err(SyncError::Invalid);
                }
            }
            transaction.execute("INSERT INTO mdreader_sync_remote(account,library,device,sequence,revision,item,blob,deleted,parents,source_device,envelope,bytes) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![self.scope.account_digest,self.scope.library_id,self.scope.device_id,change.sequence,change.revision_id,change.item_id,change.blob,change.deleted,parents,change.device,envelope,charged])?;
            for parent in &change.parents {
                transaction.execute("DELETE FROM mdreader_sync_heads WHERE account=?1 AND library=?2 AND device=?3 AND item=?4 AND revision=?5",params![self.scope.account_digest,self.scope.library_id,self.scope.device_id,change.item_id,parent])?;
            }
            transaction.execute("INSERT INTO mdreader_sync_heads(account,library,device,item,revision) VALUES(?1,?2,?3,?4,?5)",params![self.scope.account_digest,self.scope.library_id,self.scope.device_id,change.item_id,change.revision_id])?;
            let heads:u64=transaction.query_row("SELECT count(*) FROM mdreader_sync_heads WHERE account=?1 AND library=?2 AND device=?3 AND item=?4",params![self.scope.account_digest,self.scope.library_id,self.scope.device_id,change.item_id],|r|r.get(0))?;
            if heads > 8 {
                return Err(SyncError::Bounds);
            }
            count += 1;
            total += charged;
        }
        let received = current.received.max(page.cursor);
        transaction.execute("INSERT INTO mdreader_sync_cursors(account,library,device,received,applied,acked) VALUES(?1,?2,?3,?4,0,0) ON CONFLICT(account,library,device) DO UPDATE SET received=max(received,excluded.received)",params![self.scope.account_digest,self.scope.library_id,self.scope.device_id,received])?;
        transaction.commit()?;
        self.cursors()
    }
    /// Concurrent branches and tombstones remain separate readable revision heads.
    pub fn heads(&self, item: &str) -> Result<Vec<RemoteRevision>> {
        validate_uuid(item)?;
        let mut statement=self.connection.prepare("SELECT r.sequence,r.revision,r.item,r.blob,r.deleted,r.parents,r.source_device,r.envelope FROM mdreader_sync_heads h JOIN mdreader_sync_remote r ON r.account=h.account AND r.library=h.library AND r.device=h.device AND r.revision=h.revision WHERE h.account=?1 AND h.library=?2 AND h.device=?3 AND h.item=?4 ORDER BY h.revision")?;
        let rows = statement
            .query_map(
                params![
                    self.scope.account_digest,
                    self.scope.library_id,
                    self.scope.device_id,
                    item
                ],
                raw_remote,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if rows.len() > 8 {
            return Err(SyncError::Schema);
        }
        rows.into_iter().map(parse_remote).collect()
    }
    /// Bounded durable history for the owner to apply/export, including consumed revisions.
    pub fn staged(&self, after: u64, limit: usize) -> Result<Vec<RemoteRevision>> {
        validate_sequence(after, true)?;
        if limit == 0 || limit > 100 {
            return Err(SyncError::Bounds);
        }
        let cursor = self.cursors()?;
        if after > cursor.received {
            return Err(SyncError::NotReady);
        }
        let mut statement=self.connection.prepare("SELECT sequence,revision,item,blob,deleted,parents,source_device,envelope FROM mdreader_sync_remote WHERE account=?1 AND library=?2 AND device=?3 AND sequence>?4 AND sequence<=?5 ORDER BY sequence LIMIT ?6")?;
        let rows = statement
            .query_map(
                params![
                    self.scope.account_digest,
                    self.scope.library_id,
                    self.scope.device_id,
                    after,
                    cursor.received,
                    limit
                ],
                raw_remote,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter().map(parse_remote).collect()
    }
    /// The owner applies document/reading state with this same SQLite transaction.
    /// Callback failure rolls back application writes and the applied cursor together.
    pub fn mark_applied(
        &mut self,
        through: u64,
        apply: impl FnOnce(&Transaction<'_>) -> Result<()>,
    ) -> Result<CursorState> {
        validate_sequence(through, true)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = cursors_on(&transaction, &self.scope)?;
        if through > current.received {
            return Err(SyncError::NotReady);
        }
        if through <= current.applied {
            transaction.commit()?;
            return Ok(current);
        }
        apply(&transaction)?;
        transaction.execute("UPDATE mdreader_sync_cursors SET applied=?1 WHERE account=?2 AND library=?3 AND device=?4",params![through,self.scope.account_digest,self.scope.library_id,self.scope.device_id])?;
        transaction.commit()?;
        self.cursors()
    }
    pub fn confirm_cursor_ack(
        &mut self,
        response_scope: &SyncScope,
        status: u16,
        requested: u64,
        bytes: &[u8],
    ) -> Result<CursorState> {
        self.response_scope(response_scope, status, &[200])?;
        validate_sequence(requested, true)?;
        let response: CursorAck = parse(bytes, MAX_RECEIPT_BYTES)?;
        validate_sequence(response.cursor, true)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = cursors_on(&transaction, &self.scope)?;
        if requested > current.applied
            || response.cursor < requested
            || response.cursor > current.applied
        {
            return Err(SyncError::Authentication);
        }
        transaction.execute("INSERT INTO mdreader_sync_cursors(account,library,device,received,applied,acked) VALUES(?1,?2,?3,0,0,0) ON CONFLICT(account,library,device) DO NOTHING",params![self.scope.account_digest,self.scope.library_id,self.scope.device_id])?;
        transaction.execute("UPDATE mdreader_sync_cursors SET acked=max(acked,?1) WHERE account=?2 AND library=?3 AND device=?4",params![response.cursor,self.scope.account_digest,self.scope.library_id,self.scope.device_id])?;
        transaction.commit()?;
        self.cursors()
    }
}

type RawOutbox = (String, Option<Vec<u8>>, String, Option<String>);
fn entry_on(
    connection: &Connection,
    scope: &SyncScope,
    operation_id: &str,
) -> Result<Option<OutboxEntry>> {
    let row:Option<RawOutbox>=connection.query_row("SELECT operation,envelope,state,receipt FROM mdreader_sync_outbox WHERE account=?1 AND library=?2 AND device=?3 AND operation_id=?4",params![scope.account_digest,scope.library_id,scope.device_id,operation_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    row.map(|(op, envelope, state, receipt)| {
        let operation: CommitOperation = parse(op.as_bytes(), 4096)?;
        operation.validate()?;
        validate_encrypted_body(&operation, envelope.as_deref())?;
        let state = OutboxState::parse(&state)?;
        let receipt = receipt
            .map(|r| {
                let mut receipt: CommitReceipt = parse(r.as_bytes(), MAX_RECEIPT_BYTES)?;
                receipt.validate()?;
                if receipt.operation_id != operation.operation_id
                    || receipt.revision_id != operation.revision_id
                {
                    return Err(SyncError::Schema);
                }
                Ok(receipt)
            })
            .transpose()?;
        if (state == OutboxState::Acknowledged) != receipt.is_some() {
            return Err(SyncError::Schema);
        }
        Ok(OutboxEntry {
            operation,
            envelope_utf8: envelope,
            state,
            receipt,
        })
    })
    .transpose()
}
fn validate_encrypted_body(operation: &CommitOperation, envelope: Option<&[u8]>) -> Result<()> {
    if operation.deleted {
        if envelope.is_some() {
            return Err(SyncError::Invalid);
        }
        return Ok(());
    }
    let envelope = envelope.ok_or(SyncError::Invalid)?;
    crate::crypto::validate_envelope(envelope)?;
    if operation.blob.as_deref() != Some(blob_digest(envelope).as_str()) {
        return Err(SyncError::Authentication);
    }
    Ok(())
}
fn cursors_on(connection: &Connection, scope: &SyncScope) -> Result<CursorState> {
    Ok(connection.query_row("SELECT received,applied,acked FROM mdreader_sync_cursors WHERE account=?1 AND library=?2 AND device=?3",params![scope.account_digest,scope.library_id,scope.device_id],|r|Ok(CursorState{received:r.get(0)?,applied:r.get(1)?,server_acknowledged:r.get(2)?})).optional()?.unwrap_or_default())
}
fn validate_sequence(sequence: u64, zero: bool) -> Result<()> {
    if sequence > MAX_SAFE_INTEGER || (!zero && sequence == 0) {
        return Err(SyncError::Invalid);
    }
    Ok(())
}
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| SyncError::Invalid)
}
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8], max: usize) -> Result<T> {
    if bytes.len() > max {
        return Err(SyncError::Bounds);
    }
    serde_json::from_slice(bytes).map_err(|_| SyncError::Invalid)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteChange {
    pub sequence: u64,
    pub revision_id: String,
    pub item_id: String,
    #[serde(deserialize_with = "required_blob")]
    pub blob: Option<String>,
    pub deleted: bool,
    pub parents: Vec<String>,
    pub device: String,
}
#[derive(Debug, Clone)]
pub struct RemoteRevision {
    pub change: RemoteChange,
    pub envelope_utf8: Option<Vec<u8>>,
}

fn required_blob<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}
impl RemoteChange {
    fn validate(&mut self) -> Result<()> {
        validate_sequence(self.sequence, false)?;
        validate_uuid(&self.revision_id)?;
        validate_uuid(&self.item_id)?;
        validate_device(&self.device)?;
        validate_parents(&self.parents, &self.revision_id)?;
        match (&self.blob, self.deleted) {
            (Some(blob), false) => validate_digest(blob)?,
            (None, true) => {}
            _ => return Err(SyncError::Invalid),
        }
        self.parents.sort();
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChangesPage {
    changes: Vec<RemoteChange>,
    cursor: u64,
    high_water: u64,
    has_more: bool,
    checkpoint: u64,
}
impl ChangesPage {
    fn validate(&mut self, after: u64) -> Result<()> {
        validate_sequence(self.cursor, true)?;
        validate_sequence(self.high_water, true)?;
        if self.changes.len() > 100
            || self.checkpoint != 0
            || self.cursor > self.high_water
            || self.has_more != (self.cursor < self.high_water)
            || (self.has_more && self.changes.is_empty())
        {
            return Err(SyncError::Invalid);
        }
        for (index, change) in self.changes.iter_mut().enumerate() {
            change.validate()?;
            if change.sequence
                != after
                    .checked_add(index as u64 + 1)
                    .ok_or(SyncError::Invalid)?
            {
                return Err(SyncError::Invalid);
            }
        }
        if self.cursor != self.changes.last().map_or(after, |change| change.sequence) {
            return Err(SyncError::Invalid);
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CursorAck {
    cursor: u64,
}
struct RawRemote {
    sequence: u64,
    revision: String,
    item: String,
    blob: Option<String>,
    deleted: bool,
    parents: String,
    source_device: String,
    envelope: Option<Vec<u8>>,
}
fn raw_remote(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRemote> {
    Ok(RawRemote {
        sequence: row.get(0)?,
        revision: row.get(1)?,
        item: row.get(2)?,
        blob: row.get(3)?,
        deleted: row.get(4)?,
        parents: row.get(5)?,
        source_device: row.get(6)?,
        envelope: row.get(7)?,
    })
}
fn parse_remote(raw: RawRemote) -> Result<RemoteRevision> {
    let mut change = RemoteChange {
        sequence: raw.sequence,
        revision_id: raw.revision,
        item_id: raw.item,
        blob: raw.blob,
        deleted: raw.deleted,
        parents: parse(raw.parents.as_bytes(), 1024)?,
        device: raw.source_device,
    };
    change.validate()?;
    if change.deleted {
        if raw.envelope.is_some() {
            return Err(SyncError::Schema);
        }
    } else {
        let envelope = raw.envelope.as_deref().ok_or(SyncError::Schema)?;
        crate::crypto::validate_envelope(envelope)?;
        if change.blob.as_deref() != Some(blob_digest(envelope).as_str()) {
            return Err(SyncError::Authentication);
        }
    }
    Ok(RemoteRevision {
        change,
        envelope_utf8: raw.envelope,
    })
}
fn remote_on(
    connection: &Connection,
    scope: &SyncScope,
    revision: &str,
) -> Result<Option<RemoteRevision>> {
    let raw=connection.query_row("SELECT sequence,revision,item,blob,deleted,parents,source_device,envelope FROM mdreader_sync_remote WHERE account=?1 AND library=?2 AND device=?3 AND revision=?4",params![scope.account_digest,scope.library_id,scope.device_id,revision],raw_remote).optional()?;
    raw.map(parse_remote).transpose()
}
