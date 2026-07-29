//! Durable, local-only materializations for provider-neutral Markdown sources.
//!
//! Provider identifiers, revisions, and cursors are bound as SQLite BLOBs.
//! The store never formats them for diagnostics or logs.

use std::{
    fmt,
    path::Path,
    sync::{Mutex, MutexGuard},
};

use catdb_connectors::SourceId;
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use super::{
    MarkdownDocumentRevision, MarkdownMetadataPage, ProviderCursor, ProviderRevision,
    SourceRecordKey,
};

const MAX_IDENTIFIER_BYTES: usize = 4 * 1024;
const MAX_RECEIPT_BYTES: usize = 64 * 1024;
const MAX_WRITE_REPAIR_PAGE_ROWS: usize = 256;
const MAX_CHANGE_LOOKUP_KEYS: usize = 256;
const DEFAULT_CACHE_BYTES: u64 = 256 * 1024 * 1024;
const SCHEMA_SQL: &str = "
    CREATE TABLE mdreader_cloud_snapshot (
        source_id BLOB NOT NULL,
        generation BLOB NOT NULL,
        state TEXT NOT NULL CHECK (state IN ('staging','published','retired')),
        observed_at_ms INTEGER NOT NULL,
        schema_version_id BLOB NOT NULL,
        mapping_version_id BLOB NOT NULL,
        source_version_id BLOB NOT NULL,
        PRIMARY KEY (source_id, generation)
    );
    CREATE UNIQUE INDEX mdreader_cloud_one_published_snapshot
        ON mdreader_cloud_snapshot (source_id) WHERE state = 'published';
    CREATE TABLE mdreader_cloud_document (
        source_id BLOB NOT NULL,
        generation BLOB NOT NULL,
        record_key BLOB NOT NULL,
        name TEXT NOT NULL,
        display_path TEXT NOT NULL,
        media_type TEXT,
        modified_at_ms INTEGER,
        size_bytes INTEGER,
        revision BLOB NOT NULL,
        content_hash BLOB,
        deleted INTEGER NOT NULL CHECK (deleted IN (0,1)),
        schema_version_id BLOB NOT NULL,
        mapping_version_id BLOB NOT NULL,
        source_version_id BLOB NOT NULL,
        observed_at_ms INTEGER NOT NULL,
        PRIMARY KEY (source_id, generation, record_key),
        FOREIGN KEY (source_id, generation)
            REFERENCES mdreader_cloud_snapshot (source_id, generation)
            ON DELETE CASCADE
    );
    CREATE INDEX mdreader_cloud_document_published
        ON mdreader_cloud_document (source_id, generation, display_path, record_key);
    CREATE TABLE mdreader_cloud_sync_state (
        source_id BLOB PRIMARY KEY,
        change_cursor BLOB,
        sync_state TEXT NOT NULL,
        last_success_ms INTEGER,
        last_error_kind TEXT,
        retry_after_ms INTEGER
    );
    CREATE TABLE mdreader_cloud_body_cache (
        source_id BLOB NOT NULL,
        source_version_id BLOB NOT NULL,
        record_key BLOB NOT NULL,
        revision BLOB NOT NULL,
        body BLOB NOT NULL,
        body_hash BLOB NOT NULL,
        size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
        last_access_ms INTEGER NOT NULL,
        PRIMARY KEY (source_id, source_version_id, record_key, revision)
    );
    CREATE INDEX mdreader_cloud_body_cache_lru
        ON mdreader_cloud_body_cache (
            last_access_ms, source_id, source_version_id, record_key, revision
        );
    CREATE TABLE mdreader_cloud_write_repair (
        operation_id BLOB PRIMARY KEY,
        source_id BLOB NOT NULL,
        record_key BLOB NOT NULL,
        accepted_revision BLOB NOT NULL,
        body_hash BLOB NOT NULL,
        receipt_json BLOB NOT NULL,
        state TEXT NOT NULL CHECK (state IN ('provider-accepted','published')),
        created_at_ms INTEGER NOT NULL
    );";

/// Durable Cloud-document materialization errors. Variants deliberately do
/// not contain provider values, document bodies, or receipt contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloudDocumentStoreError {
    /// SQLite could not safely complete the requested durable operation.
    Persistence,
    /// A prior schema creation left controlled tables without a committed version.
    MigrationInterrupted,
    /// The expected store schema or a stored payload is malformed.
    CorruptStore,
    /// A cursor cannot safely be persisted as an opaque value.
    InvalidCursor,
    /// A provider revision cannot safely be persisted as an opaque value.
    InvalidRevision,
    /// An opaque source, record, generation, or version identifier is invalid.
    InvalidIdentifier,
    /// One staging generation received rows with incompatible semantic pins.
    VersionPinMismatch,
    /// The requested staged generation is absent or no longer staging.
    UnknownGeneration,
    /// The operation refers to a source that has no matching published state.
    ForeignSource,
    /// A body exceeds the per-item cache bound.
    BodyTooLarge,
    /// The requested write repair does not exist.
    WriteRepairNotFound,
    /// A reused operation identifier has different provider-authoritative data.
    WriteRepairCollision,
    /// A write-repair recovery page limit is zero or exceeds the fixed bound.
    InvalidWriteRepairPageLimit,
    /// A test-only failpoint aborted a transaction before commit.
    InjectedCommitFailure,
}

impl fmt::Display for CloudDocumentStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Persistence => "cloud-document-store-persistence-failed",
            Self::MigrationInterrupted => "cloud-document-store-migration-interrupted",
            Self::CorruptStore => "cloud-document-store-corrupt",
            Self::InvalidCursor => "cloud-document-store-invalid-cursor",
            Self::InvalidRevision => "cloud-document-store-invalid-revision",
            Self::InvalidIdentifier => "cloud-document-store-invalid-identifier",
            Self::VersionPinMismatch => "cloud-document-store-version-pin-mismatch",
            Self::UnknownGeneration => "cloud-document-store-generation-not-staging",
            Self::ForeignSource => "cloud-document-store-foreign-source",
            Self::BodyTooLarge => "cloud-document-store-body-too-large",
            Self::WriteRepairNotFound => "cloud-document-store-write-repair-not-found",
            Self::WriteRepairCollision => "cloud-document-store-write-repair-collision",
            Self::InvalidWriteRepairPageLimit => {
                "cloud-document-store-invalid-write-repair-page-limit"
            }
            Self::InjectedCommitFailure => "cloud-document-store-commit-failure",
        })
    }
}

impl std::error::Error for CloudDocumentStoreError {}

type StoreResult<T> = Result<T, CloudDocumentStoreError>;

/// Immutable semantic versions attached to every materialized metadata row.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CloudDocumentVersionPins {
    schema_version_id: Vec<u8>,
    mapping_version_id: Vec<u8>,
    source_version_id: Vec<u8>,
}

impl CloudDocumentVersionPins {
    /// Builds a complete immutable version pin set from opaque bytes.
    pub(crate) fn new(
        schema_version_id: impl AsRef<[u8]>,
        mapping_version_id: impl AsRef<[u8]>,
        source_version_id: impl AsRef<[u8]>,
    ) -> StoreResult<Self> {
        let pins = Self {
            schema_version_id: schema_version_id.as_ref().to_vec(),
            mapping_version_id: mapping_version_id.as_ref().to_vec(),
            source_version_id: source_version_id.as_ref().to_vec(),
        };
        pins.validate()?;
        Ok(pins)
    }

    fn validate(&self) -> StoreResult<()> {
        validate_identifier(&self.schema_version_id)?;
        validate_identifier(&self.mapping_version_id)?;
        validate_identifier(&self.source_version_id)
    }
}

impl fmt::Debug for CloudDocumentVersionPins {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CloudDocumentVersionPins")
            .field("schema_version_bytes", &self.schema_version_id.len())
            .field("mapping_version_bytes", &self.mapping_version_id.len())
            .field("source_version_bytes", &self.source_version_id.len())
            .finish()
    }
}

/// Opaque generation identifier for one initial-sync staging snapshot.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CloudSnapshotGeneration {
    source_id: Vec<u8>,
    generation: [u8; 16],
}

impl CloudSnapshotGeneration {
    /// Returns the opaque generation as a UUID for in-process sync status only.
    pub(crate) fn id(&self) -> uuid::Uuid {
        uuid::Uuid::from_bytes(self.generation)
    }
}

impl fmt::Debug for CloudSnapshotGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CloudSnapshotGeneration")
            .field("source_bytes", &self.source_id.len())
            .field("generation_bytes", &self.generation.len())
            .finish()
    }
}

/// One metadata row from the sole published snapshot of a source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PublishedCloudDocument {
    /// The source-owned record key, never suitable for renderer diagnostics.
    pub(crate) key: SourceRecordKey,
    /// Provider display name retained as document metadata.
    pub(crate) name: String,
    /// Provider display path retained as document metadata.
    pub(crate) display_path: String,
    /// Optional provider media type.
    pub(crate) media_type: Option<String>,
    /// Optional provider modification timestamp.
    pub(crate) modified_at_ms: Option<i64>,
    /// Optional provider-reported size.
    pub(crate) size_bytes: Option<u64>,
    /// Exact provider revision that was materialized.
    pub(crate) revision: ProviderRevision,
    /// Optional provider content hash for the metadata revision.
    pub(crate) content_hash: Option<[u8; 32]>,
    /// Provider deletion tombstone state.
    pub(crate) deleted: bool,
    /// Semantic versions used to interpret this row.
    pub(crate) pins: CloudDocumentVersionPins,
    /// Materialization observation time.
    pub(crate) observed_at_ms: i64,
}

/// A provider-accepted write receipt that must outlive process restart until
/// derived metadata publication is durable.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CloudWriteRepair {
    operation_id: Vec<u8>,
    source_id: Vec<u8>,
    record_key: SourceRecordKey,
    accepted_revision: ProviderRevision,
    body_hash: [u8; 32],
    receipt_json: Vec<u8>,
    created_at_ms: i64,
}

impl CloudWriteRepair {
    /// Creates a repair journal entry from provider-confirmed opaque values.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        operation_id: impl AsRef<[u8]>,
        source_id: &SourceId,
        record_key: SourceRecordKey,
        accepted_revision: ProviderRevision,
        body_hash: [u8; 32],
        receipt_json: impl AsRef<[u8]>,
        created_at_ms: i64,
    ) -> StoreResult<Self> {
        let entry = Self {
            operation_id: operation_id.as_ref().to_vec(),
            source_id: source_bytes(source_id)?.to_vec(),
            record_key,
            accepted_revision,
            body_hash,
            receipt_json: receipt_json.as_ref().to_vec(),
            created_at_ms,
        };
        validate_identifier(&entry.operation_id)?;
        validate_record_key(&entry.record_key)?;
        validate_revision(&entry.accepted_revision)?;
        if entry.receipt_json.is_empty() || entry.receipt_json.len() > MAX_RECEIPT_BYTES {
            return Err(CloudDocumentStoreError::InvalidIdentifier);
        }
        Ok(entry)
    }

    /// Returns the operation identifier only for a subsequent store operation.
    pub(crate) fn operation_id(&self) -> &[u8] {
        &self.operation_id
    }
}

impl fmt::Debug for CloudWriteRepair {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CloudWriteRepair")
            .field("operation_bytes", &self.operation_id.len())
            .field("source_bytes", &self.source_id.len())
            .field("record_key_bytes", &self.record_key.0.len())
            .field("revision_bytes", &self.accepted_revision.opaque.len())
            .field("receipt_bytes", &self.receipt_json.len())
            .field("created_at_ms", &self.created_at_ms)
            .finish()
    }
}

/// Persisted state of a provider-accepted write repair.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PersistedCloudWriteRepair {
    operation_id: Vec<u8>,
    source_id: Vec<u8>,
    record_key: SourceRecordKey,
    accepted_revision: ProviderRevision,
    body_hash: [u8; 32],
    receipt_json: Vec<u8>,
    state: WriteRepairState,
    created_at_ms: i64,
}

impl PersistedCloudWriteRepair {
    /// Returns the opaque operation identifier for a source-scoped repair operation.
    pub(crate) fn operation_id(&self) -> &[u8] {
        &self.operation_id
    }

    /// Returns the opaque source identifier without formatting it for display.
    pub(crate) fn source_id_bytes(&self) -> &[u8] {
        &self.source_id
    }

    /// Returns the provider-owned record key for recovery publication.
    pub(crate) fn record_key(&self) -> &SourceRecordKey {
        &self.record_key
    }

    /// Returns the provider revision accepted by the source.
    pub(crate) fn accepted_revision(&self) -> &ProviderRevision {
        &self.accepted_revision
    }

    /// Returns the body hash accepted by the source.
    pub(crate) fn body_hash(&self) -> &[u8; 32] {
        &self.body_hash
    }

    /// Whether provider acceptance is durable but derived publication remains pending.
    pub(crate) fn is_provider_accepted(&self) -> bool {
        self.state == WriteRepairState::ProviderAccepted
    }

    /// Whether all derived publication was marked durable.
    pub(crate) fn is_published(&self) -> bool {
        self.state == WriteRepairState::Published
    }

    /// Returns opaque receipt bytes only to the recovery path that owns them.
    pub(crate) fn receipt_json(&self) -> &[u8] {
        &self.receipt_json
    }

    /// Returns the durable provider-acceptance observation time.
    pub(crate) fn created_at_ms(&self) -> i64 {
        self.created_at_ms
    }
}

impl fmt::Debug for PersistedCloudWriteRepair {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersistedCloudWriteRepair")
            .field("operation_bytes", &self.operation_id.len())
            .field("source_bytes", &self.source_id.len())
            .field("record_key_bytes", &self.record_key.0.len())
            .field("revision_bytes", &self.accepted_revision.opaque.len())
            .field("body_hash_bytes", &self.body_hash.len())
            .field("receipt_bytes", &self.receipt_json.len())
            .field("state", &self.state)
            .field("created_at_ms", &self.created_at_ms)
            .finish()
    }
}

/// Opaque position after one repair in source-scoped recovery ordering.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RepairContinuation {
    source_id: Vec<u8>,
    created_at_ms: i64,
    operation_id: Vec<u8>,
}

impl RepairContinuation {
    fn after(repair: &PersistedCloudWriteRepair) -> Self {
        Self {
            source_id: repair.source_id.clone(),
            created_at_ms: repair.created_at_ms,
            operation_id: repair.operation_id.clone(),
        }
    }
}

impl fmt::Debug for RepairContinuation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RepairContinuation")
            .field("source_bytes", &self.source_id.len())
            .field("created_at_ms", &self.created_at_ms)
            .field("operation_bytes", &self.operation_id.len())
            .finish()
    }
}

/// One bounded source-scoped page of pending write repairs.
pub(crate) struct RepairPage {
    repairs: Vec<PersistedCloudWriteRepair>,
    next: Option<RepairContinuation>,
}

impl RepairPage {
    /// Returns the repairs in deterministic recovery order.
    pub(crate) fn repairs(&self) -> &[PersistedCloudWriteRepair] {
        &self.repairs
    }

    /// Returns the opaque continuation after the final repair in this page.
    pub(crate) fn next_continuation(&self) -> Option<&RepairContinuation> {
        self.next.as_ref()
    }
}

impl fmt::Debug for RepairPage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RepairPage")
            .field("repair_count", &self.repairs.len())
            .field("has_next", &self.next.is_some())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WriteRepairState {
    ProviderAccepted,
    Published,
}

impl WriteRepairState {
    fn as_sql(self) -> &'static str {
        match self {
            Self::ProviderAccepted => "provider-accepted",
            Self::Published => "published",
        }
    }

    fn from_sql(value: &str) -> StoreResult<Self> {
        match value {
            "provider-accepted" => Ok(Self::ProviderAccepted),
            "published" => Ok(Self::Published),
            _ => Err(CloudDocumentStoreError::CorruptStore),
        }
    }
}

/// SQLite-backed materializations, cache, and write-repair journal for every
/// cloud source associated with one Library.
pub(crate) struct CloudDocumentStore {
    connection: Mutex<Connection>,
    cache_limit_bytes: u64,
    #[cfg(test)]
    fail_next_change_commit: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    fail_next_full_published_listing: std::sync::atomic::AtomicBool,
}

impl CloudDocumentStore {
    /// Maximum body size permitted for one cached Markdown document.
    pub(crate) const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

    /// Largest permitted pending write-repair recovery page.
    pub(crate) const MAX_WRITE_REPAIR_PAGE_ROWS: usize = MAX_WRITE_REPAIR_PAGE_ROWS;

    /// Largest bounded source-scoped lookup following one provider change page.
    pub(crate) const MAX_CHANGE_LOOKUP_ROWS: usize = MAX_CHANGE_LOOKUP_KEYS;

    /// Opens (or creates) the durable materialization database at `path`.
    pub(crate) fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        Self::open_with_cache_limit(path, DEFAULT_CACHE_BYTES)
    }

    #[cfg(test)]
    pub(crate) fn open_with_cache_limit_for_tests(
        path: impl AsRef<Path>,
        cache_limit_bytes: u64,
    ) -> StoreResult<Self> {
        Self::open_with_cache_limit(path, cache_limit_bytes)
    }

    fn open_with_cache_limit(path: impl AsRef<Path>, cache_limit_bytes: u64) -> StoreResult<Self> {
        let mut connection =
            Connection::open(path).map_err(|_| CloudDocumentStoreError::Persistence)?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        initialize_schema(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
            cache_limit_bytes,
            #[cfg(test)]
            fail_next_change_commit: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            fail_next_full_published_listing: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Starts an isolated staging generation without replacing published data.
    pub(crate) fn begin_initial_sync(
        &self,
        source_id: &SourceId,
        cursor: &ProviderCursor,
        pins: &CloudDocumentVersionPins,
        observed_at_ms: i64,
    ) -> StoreResult<CloudSnapshotGeneration> {
        let source_id = source_bytes(source_id)?;
        validate_cursor(cursor)?;
        pins.validate()?;
        let generation = uuid::Uuid::new_v4().into_bytes();
        let connection = self.connection()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        transaction
            .execute(
                "DELETE FROM mdreader_cloud_snapshot
                 WHERE source_id = ?1 AND state = 'staging'",
                params![source_id],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        transaction
            .execute(
                "INSERT INTO mdreader_cloud_snapshot (
                    source_id, generation, state, observed_at_ms, schema_version_id,
                    mapping_version_id, source_version_id
                 ) VALUES (?1, ?2, 'staging', ?3, ?4, ?5, ?6)",
                params![
                    source_id,
                    generation,
                    observed_at_ms,
                    &pins.schema_version_id,
                    &pins.mapping_version_id,
                    &pins.source_version_id,
                ],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        transaction
            .execute(
                "INSERT INTO mdreader_cloud_sync_state
                    (source_id, change_cursor, sync_state, last_success_ms, last_error_kind, retry_after_ms)
                 VALUES (?1, NULL, 'initializing', NULL, NULL, NULL)
                 ON CONFLICT(source_id) DO UPDATE SET sync_state = 'initializing',
                     last_error_kind = NULL, retry_after_ms = NULL",
                params![source_id],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        transaction
            .commit()
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        Ok(CloudSnapshotGeneration {
            source_id: source_id.to_vec(),
            generation,
        })
    }

    /// Adds one provider page to a staging generation. A failure leaves the
    /// currently published generation untouched.
    pub(crate) fn stage_page(
        &self,
        generation: &CloudSnapshotGeneration,
        pins: &CloudDocumentVersionPins,
        page: &MarkdownMetadataPage,
    ) -> StoreResult<()> {
        pins.validate()?;
        validate_page(page)?;
        let connection = self.connection()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        ensure_staging_generation(&transaction, generation)?;
        ensure_generation_pins(&transaction, generation, pins)?;
        insert_metadata_rows(&transaction, generation, pins, page)?;
        transaction
            .commit()
            .map_err(|_| CloudDocumentStoreError::Persistence)
    }

    /// Atomically turns a complete staging generation into the source's only
    /// published snapshot and advances its opaque change cursor.
    pub(crate) fn publish_initial_sync(
        &self,
        generation: &CloudSnapshotGeneration,
        cursor: &ProviderCursor,
        observed_at_ms: i64,
    ) -> StoreResult<()> {
        validate_cursor(cursor)?;
        let connection = self.connection()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        ensure_staging_generation(&transaction, generation)?;
        transaction
            .execute(
                "UPDATE mdreader_cloud_snapshot SET state = 'retired'
                 WHERE source_id = ?1 AND state = 'published'",
                params![&generation.source_id],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        let published = transaction
            .execute(
                "UPDATE mdreader_cloud_snapshot SET state = 'published', observed_at_ms = ?3
                 WHERE source_id = ?1 AND generation = ?2 AND state = 'staging'",
                params![&generation.source_id, generation.generation, observed_at_ms],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        if published != 1 {
            return Err(CloudDocumentStoreError::UnknownGeneration);
        }
        upsert_sync_state(&transaction, &generation.source_id, cursor, observed_at_ms)?;
        transaction
            .execute(
                "DELETE FROM mdreader_cloud_snapshot
                 WHERE source_id = ?1 AND state = 'retired'",
                params![&generation.source_id],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        transaction
            .commit()
            .map_err(|_| CloudDocumentStoreError::Persistence)
    }

    /// Applies changed metadata and its continuation cursor in one SQLite
    /// transaction. The previous published snapshot remains visible on error.
    pub(crate) fn apply_change_page(
        &self,
        source_id: &SourceId,
        pins: &CloudDocumentVersionPins,
        page: &MarkdownMetadataPage,
        cursor: &ProviderCursor,
        observed_at_ms: i64,
    ) -> StoreResult<()> {
        let source_id = source_bytes(source_id)?;
        pins.validate()?;
        validate_page(page)?;
        validate_cursor(cursor)?;
        let connection = self.connection()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        let published: Option<(Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>)> = transaction
            .query_row(
                "SELECT generation, schema_version_id, mapping_version_id, source_version_id
                 FROM mdreader_cloud_snapshot
                 WHERE source_id = ?1 AND state = 'published'",
                params![source_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        let Some((published_generation, schema_version_id, mapping_version_id, source_version_id)) =
            published
        else {
            return Err(CloudDocumentStoreError::ForeignSource);
        };
        let generation: [u8; 16] = published_generation
            .as_slice()
            .try_into()
            .map_err(|_| CloudDocumentStoreError::CorruptStore)?;
        let published_pins = CloudDocumentVersionPins::new(
            schema_version_id,
            mapping_version_id,
            source_version_id,
        )?;
        if published_pins != *pins {
            return Err(CloudDocumentStoreError::VersionPinMismatch);
        }
        let generation = CloudSnapshotGeneration {
            source_id: source_id.to_vec(),
            generation,
        };
        insert_metadata_rows(&transaction, &generation, pins, page)?;
        #[cfg(test)]
        if self
            .fail_next_change_commit
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(CloudDocumentStoreError::InjectedCommitFailure);
        }
        let updated = transaction
            .execute(
                "UPDATE mdreader_cloud_snapshot SET observed_at_ms = ?3
                 WHERE source_id = ?1 AND generation = ?2 AND state = 'published'",
                params![source_id, generation.generation, observed_at_ms],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        if updated != 1 {
            return Err(CloudDocumentStoreError::CorruptStore);
        }
        upsert_sync_state(&transaction, source_id, cursor, observed_at_ms)?;
        transaction
            .commit()
            .map_err(|_| CloudDocumentStoreError::Persistence)
    }

    /// Lists metadata from the sole published snapshot, never staging rows.
    pub(crate) fn list_published(
        &self,
        source_id: &SourceId,
    ) -> StoreResult<Vec<PublishedCloudDocument>> {
        #[cfg(test)]
        if self
            .fail_next_full_published_listing
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(CloudDocumentStoreError::InjectedCommitFailure);
        }
        let source_id = source_bytes(source_id)?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT d.record_key, d.name, d.display_path, d.media_type, d.modified_at_ms,
                    d.size_bytes, d.revision, d.content_hash, d.deleted, d.schema_version_id,
                    d.mapping_version_id, d.source_version_id, d.observed_at_ms
                 FROM mdreader_cloud_document d
                 INNER JOIN mdreader_cloud_snapshot s
                   ON s.source_id = d.source_id AND s.generation = d.generation
                 WHERE d.source_id = ?1 AND s.state = 'published'
                 ORDER BY d.display_path, d.record_key",
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        let rows = statement
            .query_map(params![source_id], decode_published_document)
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| CloudDocumentStoreError::CorruptStore)
    }

    /// Resolves only the published rows named by one bounded provider event
    /// page. This remains local SQLite work and never spans provider I/O.
    pub(crate) fn published_for_keys(
        &self,
        source_id: &SourceId,
        keys: &[SourceRecordKey],
    ) -> StoreResult<Vec<PublishedCloudDocument>> {
        if keys.len() > Self::MAX_CHANGE_LOOKUP_ROWS {
            return Err(CloudDocumentStoreError::InvalidIdentifier);
        }
        let source_id = source_bytes(source_id)?;
        for key in keys {
            validate_record_key(key)?;
        }
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT d.record_key, d.name, d.display_path, d.media_type, d.modified_at_ms,
                    d.size_bytes, d.revision, d.content_hash, d.deleted, d.schema_version_id,
                    d.mapping_version_id, d.source_version_id, d.observed_at_ms
                 FROM mdreader_cloud_document d
                 INNER JOIN mdreader_cloud_snapshot s
                   ON s.source_id = d.source_id AND s.generation = d.generation
                 WHERE d.source_id = ?1 AND d.record_key = ?2 AND s.state = 'published'",
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        let mut documents = Vec::with_capacity(keys.len());
        for key in keys {
            let document = statement
                .query_row(
                    params![source_id, key.0.as_bytes()],
                    decode_published_document,
                )
                .optional()
                .map_err(|_| CloudDocumentStoreError::Persistence)?;
            if let Some(document) = document {
                documents.push(document);
            }
        }
        Ok(documents)
    }

    /// Loads the last atomically committed provider change cursor for `source_id`.
    pub(crate) fn load_change_cursor(
        &self,
        source_id: &SourceId,
    ) -> StoreResult<Option<ProviderCursor>> {
        let source_id = source_bytes(source_id)?;
        let connection = self.connection()?;
        let cursor: Option<Option<Vec<u8>>> = connection
            .query_row(
                "SELECT change_cursor FROM mdreader_cloud_sync_state WHERE source_id = ?1",
                params![source_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        cursor
            .flatten()
            .map(|value| {
                let cursor = ProviderCursor(value);
                validate_cursor(&cursor)?;
                Ok(cursor)
            })
            .transpose()
    }

    /// Discards incomplete staging snapshots for exactly one source without
    /// touching its published snapshot, cursor, cache, or repair journal.
    pub(crate) fn discard_staging(&self, source_id: &SourceId) -> StoreResult<()> {
        let source_id = source_bytes(source_id)?;
        let connection = self.connection()?;
        connection
            .execute(
                "DELETE FROM mdreader_cloud_snapshot
                 WHERE source_id = ?1 AND state = 'staging'",
                params![source_id],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        Ok(())
    }

    /// Removes staging generations left by a process that stopped before
    /// publication. Published snapshots and all source state remain intact.
    pub(crate) fn discard_unreferenced_staging(&self) -> StoreResult<()> {
        let connection = self.connection()?;
        connection
            .execute(
                "DELETE FROM mdreader_cloud_snapshot WHERE state = 'staging'",
                [],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        Ok(())
    }

    /// Retires durable metadata, cached bodies, cursors, and pending write
    /// repairs for exactly one disconnected source. Callers must cancel that
    /// source's owner before invoking this effect.
    pub(crate) fn retire_source(&self, source_id: &SourceId) -> StoreResult<()> {
        let source_id = source_bytes(source_id)?;
        let connection = self.connection()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        transaction
            .execute(
                "DELETE FROM mdreader_cloud_snapshot WHERE source_id = ?1",
                params![source_id],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        transaction
            .execute(
                "DELETE FROM mdreader_cloud_body_cache WHERE source_id = ?1",
                params![source_id],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        transaction
            .execute(
                "DELETE FROM mdreader_cloud_write_repair WHERE source_id = ?1",
                params![source_id],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        transaction
            .execute(
                "DELETE FROM mdreader_cloud_sync_state WHERE source_id = ?1",
                params![source_id],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        transaction
            .commit()
            .map_err(|_| CloudDocumentStoreError::Persistence)
    }

    /// Caches one validated body by exact source version, record key, and
    /// provider revision, evicting the deterministic least-recently-used rows.
    pub(crate) fn put_body(
        &self,
        source_id: &SourceId,
        pins: &CloudDocumentVersionPins,
        document: &MarkdownDocumentRevision,
    ) -> StoreResult<()> {
        let source_id = source_bytes(source_id)?;
        pins.validate()?;
        validate_record_key(&document.key)?;
        validate_revision(&document.revision)?;
        if document.body.len() > Self::MAX_BODY_BYTES {
            return Err(CloudDocumentStoreError::BodyTooLarge);
        }
        let size_bytes = i64::try_from(document.body.len())
            .map_err(|_| CloudDocumentStoreError::BodyTooLarge)?;
        let body_hash = blake3::hash(&document.body);
        let connection = self.connection()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        let known_record: Option<i64> = transaction
            .query_row(
                "SELECT 1 FROM mdreader_cloud_document d
                 INNER JOIN mdreader_cloud_snapshot s
                   ON s.source_id = d.source_id AND s.generation = d.generation
                 WHERE d.source_id = ?1 AND d.source_version_id = ?2 AND d.record_key = ?3
                   AND d.deleted = 0 AND s.state = 'published'",
                params![
                    source_id,
                    &pins.source_version_id,
                    document.key.0.as_bytes()
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        if known_record.is_none() {
            return Err(CloudDocumentStoreError::ForeignSource);
        }
        transaction
            .execute(
                "INSERT INTO mdreader_cloud_body_cache (
                    source_id, source_version_id, record_key, revision, body, body_hash,
                    size_bytes, last_access_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(source_id, source_version_id, record_key, revision) DO UPDATE SET
                    body = excluded.body, body_hash = excluded.body_hash,
                    size_bytes = excluded.size_bytes, last_access_ms = excluded.last_access_ms",
                params![
                    source_id,
                    &pins.source_version_id,
                    document.key.0.as_bytes(),
                    document.revision.opaque.as_bytes(),
                    &document.body,
                    body_hash.as_bytes(),
                    size_bytes,
                    document.observed_at_ms,
                ],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        evict_bodies(&transaction, self.cache_limit_bytes)?;
        transaction
            .commit()
            .map_err(|_| CloudDocumentStoreError::Persistence)
    }

    /// Returns a body only if it matches the presently published metadata
    /// revision, then records the access time for deterministic LRU eviction.
    pub(crate) fn get_body(
        &self,
        source_id: &SourceId,
        pins: &CloudDocumentVersionPins,
        record_key: &SourceRecordKey,
        revision: &ProviderRevision,
        accessed_at_ms: i64,
    ) -> StoreResult<Option<Vec<u8>>> {
        let source_id = source_bytes(source_id)?;
        pins.validate()?;
        validate_record_key(record_key)?;
        validate_revision(revision)?;
        let connection = self.connection()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        let body: Option<(Vec<u8>, Vec<u8>, i64)> = transaction
            .query_row(
                "SELECT b.body, b.body_hash, b.size_bytes
                 FROM mdreader_cloud_body_cache b
                 INNER JOIN mdreader_cloud_document d
                   ON d.source_id = b.source_id
                  AND d.source_version_id = b.source_version_id
                  AND d.record_key = b.record_key
                  AND d.revision = b.revision
                 INNER JOIN mdreader_cloud_snapshot s
                   ON s.source_id = d.source_id AND s.generation = d.generation
                 WHERE b.source_id = ?1 AND b.source_version_id = ?2 AND b.record_key = ?3
                   AND b.revision = ?4 AND d.deleted = 0 AND s.state = 'published'",
                params![
                    source_id,
                    &pins.source_version_id,
                    record_key.0.as_bytes(),
                    revision.opaque.as_bytes(),
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        let Some((body, stored_hash, stored_size)) = body else {
            transaction
                .commit()
                .map_err(|_| CloudDocumentStoreError::Persistence)?;
            return Ok(None);
        };
        let body_size =
            i64::try_from(body.len()).map_err(|_| CloudDocumentStoreError::CorruptStore)?;
        let stored_hash: [u8; 32] = stored_hash
            .as_slice()
            .try_into()
            .map_err(|_| CloudDocumentStoreError::CorruptStore)?;
        if stored_size < 0
            || stored_size != body_size
            || stored_hash != *blake3::hash(&body).as_bytes()
        {
            return Err(CloudDocumentStoreError::CorruptStore);
        }
        transaction
            .execute(
                "UPDATE mdreader_cloud_body_cache SET last_access_ms = ?5
                 WHERE source_id = ?1 AND source_version_id = ?2 AND record_key = ?3
                   AND revision = ?4",
                params![
                    source_id,
                    &pins.source_version_id,
                    record_key.0.as_bytes(),
                    revision.opaque.as_bytes(),
                    accessed_at_ms,
                ],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        transaction
            .commit()
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        Ok(Some(body))
    }

    /// Journals a provider-accepted write before any derived materialization is
    /// attempted. No receipt is removed by this store operation.
    pub(crate) fn record_provider_acceptance(&self, repair: &CloudWriteRepair) -> StoreResult<()> {
        let connection = self.connection()?;
        let existing = connection
            .query_row(
                "SELECT operation_id, source_id, record_key, accepted_revision, body_hash,
                    receipt_json, state, created_at_ms
                 FROM mdreader_cloud_write_repair WHERE operation_id = ?1",
                params![&repair.operation_id],
                decode_write_repair_row,
            )
            .optional()
            .map_err(|_| CloudDocumentStoreError::CorruptStore)?;
        if let Some(existing) = existing {
            if existing.source_id == repair.source_id
                && existing.record_key.0.as_bytes() == repair.record_key.0.as_bytes()
                && existing.accepted_revision.opaque.as_bytes()
                    == repair.accepted_revision.opaque.as_bytes()
                && existing.body_hash == repair.body_hash
                && existing.receipt_json == repair.receipt_json
                && existing.created_at_ms == repair.created_at_ms
            {
                return Ok(());
            }
            return Err(CloudDocumentStoreError::WriteRepairCollision);
        }
        connection
            .execute(
                "INSERT INTO mdreader_cloud_write_repair (
                    operation_id, source_id, record_key, accepted_revision, body_hash,
                    receipt_json, state, created_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    &repair.operation_id,
                    &repair.source_id,
                    repair.record_key.0.as_bytes(),
                    repair.accepted_revision.opaque.as_bytes(),
                    repair.body_hash.as_slice(),
                    &repair.receipt_json,
                    WriteRepairState::ProviderAccepted.as_sql(),
                    repair.created_at_ms,
                ],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        Ok(())
    }

    /// Records that all derived publication is durable without deleting the
    /// provider receipt. Repeating a successful mark is idempotent.
    pub(crate) fn mark_write_published(
        &self,
        source_id: &SourceId,
        operation_id: &[u8],
    ) -> StoreResult<()> {
        let source_id = source_bytes(source_id)?;
        validate_identifier(operation_id)?;
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "UPDATE mdreader_cloud_write_repair SET state = 'published'
                 WHERE source_id = ?1 AND operation_id = ?2 AND state = 'provider-accepted'",
                params![source_id, operation_id],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        if changed == 1 {
            return Ok(());
        }
        let exists: Option<i64> = connection
            .query_row(
                "SELECT 1 FROM mdreader_cloud_write_repair
                 WHERE source_id = ?1 AND operation_id = ?2",
                params![source_id, operation_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        if exists.is_some() {
            Ok(())
        } else {
            Err(CloudDocumentStoreError::WriteRepairNotFound)
        }
    }

    /// Loads a retained repair receipt for restart recovery.
    pub(crate) fn load_write_repair(
        &self,
        source_id: &SourceId,
        operation_id: &[u8],
    ) -> StoreResult<Option<PersistedCloudWriteRepair>> {
        let source_id = source_bytes(source_id)?;
        validate_identifier(operation_id)?;
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT operation_id, source_id, record_key, accepted_revision, body_hash,
                    receipt_json, state, created_at_ms
                 FROM mdreader_cloud_write_repair WHERE source_id = ?1 AND operation_id = ?2",
                params![source_id, operation_id],
                decode_write_repair_row,
            )
            .optional()
            .map_err(|_| CloudDocumentStoreError::CorruptStore)
    }

    /// Loads one bounded provider-accepted repair page for exactly one source.
    ///
    /// The continuation is exclusive and includes both deterministic ordering
    /// fields, so rows sharing an observation time are neither repeated nor
    /// skipped across pages.
    pub(crate) fn pending_write_repairs_page(
        &self,
        source_id: &SourceId,
        after: Option<&RepairContinuation>,
        limit: usize,
    ) -> StoreResult<RepairPage> {
        if limit == 0 || limit > Self::MAX_WRITE_REPAIR_PAGE_ROWS {
            return Err(CloudDocumentStoreError::InvalidWriteRepairPageLimit);
        }
        let source_id = source_bytes(source_id)?;
        if after.is_some_and(|continuation| continuation.source_id.as_slice() != source_id) {
            return Err(CloudDocumentStoreError::ForeignSource);
        }
        let query_limit = i64::try_from(limit + 1)
            .map_err(|_| CloudDocumentStoreError::InvalidWriteRepairPageLimit)?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT operation_id, source_id, record_key, accepted_revision, body_hash,
                    receipt_json, state, created_at_ms
                 FROM mdreader_cloud_write_repair
                 WHERE source_id = ?1 AND state = 'provider-accepted'
                   AND (?2 IS NULL OR created_at_ms > ?2
                        OR (created_at_ms = ?2 AND operation_id > ?3))
                 ORDER BY created_at_ms, operation_id
                 LIMIT ?4",
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        let repairs = statement
            .query_map(
                params![
                    source_id,
                    after.map(|continuation| continuation.created_at_ms),
                    after.map(|continuation| continuation.operation_id.as_slice()),
                    query_limit,
                ],
                decode_write_repair_row,
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        let mut repairs = repairs
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| CloudDocumentStoreError::CorruptStore)?;
        let next = if repairs.len() > limit {
            repairs.pop();
            repairs.last().map(RepairContinuation::after)
        } else {
            None
        };
        Ok(RepairPage { repairs, next })
    }

    #[cfg(test)]
    pub(crate) fn inject_commit_failure_after_rows(&self) {
        self.fail_next_change_commit
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn reject_next_full_published_listing(&self) {
        self.fail_next_full_published_listing
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn connection(&self) -> StoreResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| CloudDocumentStoreError::Persistence)
    }
}

fn initialize_schema(connection: &mut Connection) -> StoreResult<()> {
    let controlled_tables = controlled_table_count(connection)?;
    match controlled_tables {
        0 => {
            let transaction = connection
                .transaction()
                .map_err(|_| CloudDocumentStoreError::Persistence)?;
            transaction
                .execute_batch(SCHEMA_SQL)
                .map_err(|_| CloudDocumentStoreError::Persistence)?;
            transaction
                .commit()
                .map_err(|_| CloudDocumentStoreError::Persistence)
        }
        1..=4 => Err(CloudDocumentStoreError::MigrationInterrupted),
        _ => validate_schema(connection),
    }
}

fn controlled_table_count(connection: &Connection) -> StoreResult<i64> {
    connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name GLOB 'mdreader_cloud_*'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| CloudDocumentStoreError::Persistence)
}

fn validate_schema(connection: &Connection) -> StoreResult<()> {
    let expected =
        Connection::open_in_memory().map_err(|_| CloudDocumentStoreError::Persistence)?;
    expected
        .execute_batch(SCHEMA_SQL)
        .map_err(|_| CloudDocumentStoreError::Persistence)?;
    if controlled_schema_signature(connection)? != controlled_schema_signature(&expected)? {
        return Err(CloudDocumentStoreError::CorruptStore);
    }
    Ok(())
}

fn controlled_schema_signature(
    connection: &Connection,
) -> StoreResult<Vec<(String, String, String)>> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, sql FROM sqlite_master
             WHERE name GLOB 'mdreader_cloud_*' AND type IN ('table', 'index')
             ORDER BY type, name",
        )
        .map_err(|_| CloudDocumentStoreError::Persistence)?;
    let signature = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|_| CloudDocumentStoreError::Persistence)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CloudDocumentStoreError::Persistence)?;
    Ok(signature)
}

fn source_bytes(source_id: &SourceId) -> StoreResult<&[u8]> {
    let value = source_id.as_str().as_bytes();
    validate_identifier(value)?;
    Ok(value)
}

fn validate_identifier(value: &[u8]) -> StoreResult<()> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES {
        Err(CloudDocumentStoreError::InvalidIdentifier)
    } else {
        Ok(())
    }
}

fn validate_cursor(cursor: &ProviderCursor) -> StoreResult<()> {
    if cursor.0.is_empty() || cursor.0.len() > MAX_IDENTIFIER_BYTES {
        Err(CloudDocumentStoreError::InvalidCursor)
    } else {
        Ok(())
    }
}

fn validate_record_key(record_key: &SourceRecordKey) -> StoreResult<()> {
    validate_identifier(record_key.0.as_bytes())
}

fn validate_revision(revision: &ProviderRevision) -> StoreResult<()> {
    if revision.opaque.is_empty() || revision.opaque.len() > MAX_IDENTIFIER_BYTES {
        Err(CloudDocumentStoreError::InvalidRevision)
    } else {
        Ok(())
    }
}

fn decode_write_repair_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PersistedCloudWriteRepair> {
    let operation_id: Vec<u8> = row.get(0)?;
    let source_id: Vec<u8> = row.get(1)?;
    let record_key =
        String::from_utf8(row.get::<_, Vec<u8>>(2)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let revision =
        String::from_utf8(row.get::<_, Vec<u8>>(3)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let body_hash: [u8; 32] = row
        .get::<_, Vec<u8>>(4)?
        .as_slice()
        .try_into()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let receipt_json: Vec<u8> = row.get(5)?;
    let state: String = row.get(6)?;
    let state = WriteRepairState::from_sql(&state).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let repair = PersistedCloudWriteRepair {
        operation_id,
        source_id,
        record_key: SourceRecordKey::new(record_key),
        accepted_revision: ProviderRevision {
            opaque: revision,
            content_hash: None,
        },
        body_hash,
        receipt_json,
        state,
        created_at_ms: row.get(7)?,
    };
    validate_identifier(&repair.operation_id).map_err(|_| rusqlite::Error::InvalidQuery)?;
    validate_identifier(&repair.source_id).map_err(|_| rusqlite::Error::InvalidQuery)?;
    validate_record_key(&repair.record_key).map_err(|_| rusqlite::Error::InvalidQuery)?;
    validate_revision(&repair.accepted_revision).map_err(|_| rusqlite::Error::InvalidQuery)?;
    if repair.receipt_json.is_empty() || repair.receipt_json.len() > MAX_RECEIPT_BYTES {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(repair)
}

fn validate_page(page: &MarkdownMetadataPage) -> StoreResult<()> {
    if let Some(cursor) = &page.next_cursor {
        validate_cursor(cursor)?;
    }
    for metadata in &page.rows {
        validate_record_key(&metadata.key)?;
        validate_revision(&metadata.revision)?;
        if metadata.name.is_empty() || metadata.display_path.is_empty() {
            return Err(CloudDocumentStoreError::InvalidIdentifier);
        }
        if metadata
            .size_bytes
            .is_some_and(|size| size > i64::MAX as u64)
        {
            return Err(CloudDocumentStoreError::InvalidIdentifier);
        }
    }
    Ok(())
}

fn ensure_staging_generation(
    transaction: &Transaction<'_>,
    generation: &CloudSnapshotGeneration,
) -> StoreResult<()> {
    let exists: Option<i64> = transaction
        .query_row(
            "SELECT 1 FROM mdreader_cloud_snapshot
             WHERE source_id = ?1 AND generation = ?2 AND state = 'staging'",
            params![&generation.source_id, generation.generation],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| CloudDocumentStoreError::Persistence)?;
    if exists.is_some() {
        Ok(())
    } else {
        Err(CloudDocumentStoreError::UnknownGeneration)
    }
}

fn insert_metadata_rows(
    transaction: &Transaction<'_>,
    generation: &CloudSnapshotGeneration,
    pins: &CloudDocumentVersionPins,
    page: &MarkdownMetadataPage,
) -> StoreResult<()> {
    for metadata in &page.rows {
        let size_bytes = metadata
            .size_bytes
            .map(|size| i64::try_from(size).map_err(|_| CloudDocumentStoreError::InvalidIdentifier))
            .transpose()?;
        let content_hash = metadata
            .revision
            .content_hash
            .as_ref()
            .map(|hash| hash.as_slice());
        transaction
            .execute(
                "INSERT INTO mdreader_cloud_document (
                    source_id, generation, record_key, name, display_path, media_type,
                    modified_at_ms, size_bytes, revision, content_hash, deleted,
                    schema_version_id, mapping_version_id, source_version_id, observed_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                 ON CONFLICT(source_id, generation, record_key) DO UPDATE SET
                    name = excluded.name, display_path = excluded.display_path,
                    media_type = excluded.media_type, modified_at_ms = excluded.modified_at_ms,
                    size_bytes = excluded.size_bytes, revision = excluded.revision,
                    content_hash = excluded.content_hash, deleted = excluded.deleted,
                    schema_version_id = excluded.schema_version_id,
                    mapping_version_id = excluded.mapping_version_id,
                    source_version_id = excluded.source_version_id,
                    observed_at_ms = excluded.observed_at_ms",
                params![
                    &generation.source_id,
                    generation.generation,
                    metadata.key.0.as_bytes(),
                    &metadata.name,
                    &metadata.display_path,
                    &metadata.media_type,
                    metadata.modified_at_ms,
                    size_bytes,
                    metadata.revision.opaque.as_bytes(),
                    content_hash,
                    i64::from(metadata.deleted),
                    &pins.schema_version_id,
                    &pins.mapping_version_id,
                    &pins.source_version_id,
                    page.observed_at_ms,
                ],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
    }
    Ok(())
}

fn load_generation_pins(
    transaction: &Transaction<'_>,
    generation: &CloudSnapshotGeneration,
) -> StoreResult<CloudDocumentVersionPins> {
    let values: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = transaction
        .query_row(
            "SELECT schema_version_id, mapping_version_id, source_version_id
             FROM mdreader_cloud_snapshot WHERE source_id = ?1 AND generation = ?2",
            params![&generation.source_id, generation.generation],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| CloudDocumentStoreError::Persistence)?;
    if let Some((schema_version_id, mapping_version_id, source_version_id)) = values {
        return CloudDocumentVersionPins::new(
            schema_version_id,
            mapping_version_id,
            source_version_id,
        );
    }
    Err(CloudDocumentStoreError::UnknownGeneration)
}

fn ensure_generation_pins(
    transaction: &Transaction<'_>,
    generation: &CloudSnapshotGeneration,
    requested_pins: &CloudDocumentVersionPins,
) -> StoreResult<()> {
    let existing_pins = load_generation_pins(transaction, generation)?;
    if existing_pins != *requested_pins {
        return Err(CloudDocumentStoreError::VersionPinMismatch);
    }
    Ok(())
}

fn upsert_sync_state(
    transaction: &Transaction<'_>,
    source_id: &[u8],
    cursor: &ProviderCursor,
    observed_at_ms: i64,
) -> StoreResult<()> {
    transaction
        .execute(
            "INSERT INTO mdreader_cloud_sync_state
                (source_id, change_cursor, sync_state, last_success_ms, last_error_kind, retry_after_ms)
             VALUES (?1, ?2, 'published', ?3, NULL, NULL)
             ON CONFLICT(source_id) DO UPDATE SET change_cursor = excluded.change_cursor,
                sync_state = 'published', last_success_ms = excluded.last_success_ms,
                last_error_kind = NULL, retry_after_ms = NULL",
            params![source_id, &cursor.0, observed_at_ms],
        )
        .map_err(|_| CloudDocumentStoreError::Persistence)?;
    Ok(())
}

fn evict_bodies(transaction: &Transaction<'_>, cache_limit_bytes: u64) -> StoreResult<()> {
    let mut total_bytes = 0_i64;
    let mut statement = transaction
        .prepare("SELECT body, body_hash, size_bytes FROM mdreader_cloud_body_cache")
        .map_err(|_| CloudDocumentStoreError::Persistence)?;
    let mut rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|_| CloudDocumentStoreError::Persistence)?;
    for row in &mut rows {
        let (body, body_hash, stored_size) =
            row.map_err(|_| CloudDocumentStoreError::Persistence)?;
        let body_hash: [u8; 32] = body_hash
            .as_slice()
            .try_into()
            .map_err(|_| CloudDocumentStoreError::CorruptStore)?;
        let physical_size =
            i64::try_from(body.len()).map_err(|_| CloudDocumentStoreError::CorruptStore)?;
        if stored_size < 0
            || stored_size != physical_size
            || body_hash != *blake3::hash(&body).as_bytes()
        {
            return Err(CloudDocumentStoreError::CorruptStore);
        }
        total_bytes = total_bytes
            .checked_add(physical_size)
            .ok_or(CloudDocumentStoreError::CorruptStore)?;
    }
    drop(rows);
    drop(statement);
    let cache_limit =
        i64::try_from(cache_limit_bytes).map_err(|_| CloudDocumentStoreError::Persistence)?;
    while total_bytes > cache_limit {
        let oldest: Option<(Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, i64)> = transaction
            .query_row(
                "SELECT source_id, source_version_id, record_key, revision, size_bytes
                 FROM mdreader_cloud_body_cache
                 ORDER BY last_access_ms, source_id, source_version_id, record_key, revision
                 LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        let Some((source_id, source_version_id, record_key, revision, size_bytes)) = oldest else {
            return Err(CloudDocumentStoreError::CorruptStore);
        };
        if size_bytes < 0 {
            return Err(CloudDocumentStoreError::CorruptStore);
        }
        let deleted = transaction
            .execute(
                "DELETE FROM mdreader_cloud_body_cache
                 WHERE source_id = ?1 AND source_version_id = ?2 AND record_key = ?3 AND revision = ?4",
                params![source_id, source_version_id, record_key, revision],
            )
            .map_err(|_| CloudDocumentStoreError::Persistence)?;
        if deleted != 1 {
            return Err(CloudDocumentStoreError::CorruptStore);
        }
        total_bytes -= size_bytes;
    }
    Ok(())
}

fn decode_published_document(row: &rusqlite::Row<'_>) -> rusqlite::Result<PublishedCloudDocument> {
    let record_key: Vec<u8> = row.get(0)?;
    let revision: Vec<u8> = row.get(6)?;
    let content_hash: Option<Vec<u8>> = row.get(7)?;
    let size_bytes: Option<i64> = row.get(5)?;
    let deleted: i64 = row.get(8)?;
    let schema_version_id: Vec<u8> = row.get(9)?;
    let mapping_version_id: Vec<u8> = row.get(10)?;
    let source_version_id: Vec<u8> = row.get(11)?;
    let record_key = String::from_utf8(record_key).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let revision = String::from_utf8(revision).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let content_hash = content_hash
        .map(|hash| {
            hash.as_slice()
                .try_into()
                .map_err(|_| rusqlite::Error::InvalidQuery)
        })
        .transpose()?;
    let size_bytes = size_bytes
        .map(|size| u64::try_from(size).map_err(|_| rusqlite::Error::InvalidQuery))
        .transpose()?;
    if !matches!(deleted, 0 | 1) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let pins =
        CloudDocumentVersionPins::new(schema_version_id, mapping_version_id, source_version_id)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let revision = ProviderRevision {
        opaque: revision,
        content_hash,
    };
    validate_record_key(&SourceRecordKey::new(record_key.clone()))
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    validate_revision(&revision).map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(PublishedCloudDocument {
        key: SourceRecordKey::new(record_key),
        name: row.get(1)?,
        display_path: row.get(2)?,
        media_type: row.get(3)?,
        modified_at_ms: row.get(4)?,
        size_bytes,
        revision,
        content_hash,
        deleted: deleted == 1,
        pins,
        observed_at_ms: row.get(12)?,
    })
}
