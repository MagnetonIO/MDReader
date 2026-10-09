use super::*;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
type SnapshotRow = (String, String, Option<Vec<u8>>, Option<Vec<u8>>);
type CachedRow = (Vec<u8>, Vec<u8>, Vec<u8>, i64, Option<String>);
const MAX_METADATA: usize = 32 * 1024;
const MAX_SOURCES: u64 = 128;
const MAX_ADMISSION_RECEIPTS: u64 = 4096;
const MAX_ROWS: u64 = 16_384;
const MAX_METADATA_STORAGE: u64 = 64 * 1024 * 1024;
const MAX_CACHE_STORAGE: u64 = 256 * 1024 * 1024;
// Bound durable replay history separately from metadata rows and per-call I/O.
const MAX_PAGES: u64 = 16_384;
const MAX_HISTORY_PAGES: u64 = 65_536;
pub(super) fn db<T>(r: std::result::Result<T, rusqlite::Error>) -> Result<T> {
    r.map_err(|_| ProviderError::Persistence)
}
pub(super) fn encode<T: Serialize>(v: &T, limit: usize) -> Result<String> {
    let s = serde_json::to_string(v).map_err(|_| ProviderError::Invalid)?;
    if s.len() > limit {
        return Err(ProviderError::TooLarge);
    }
    Ok(s)
}
pub(super) fn decode<T: serde::de::DeserializeOwned>(s: &str, limit: usize) -> Result<T> {
    if s.len() > limit {
        return Err(ProviderError::Persistence);
    }
    serde_json::from_str(s).map_err(|_| ProviderError::Persistence)
}
pub(super) fn identity(binding: &SourceBinding) -> String {
    hex(&body_hash(
        &serde_json::to_vec(&(binding.library_id(), binding.source_id())).expect("fixed IDs"),
    ))
}
pub(super) fn version_digest(version: &DocumentVersion) -> [u8; 32] {
    body_hash(&serde_json::to_vec(version).expect("fixed version"))
}
/// Own migration guard; preserves global user_version and durability policy.
pub fn initialize(connection: &mut Connection) -> Result<()> {
    if db(connection.query_row("PRAGMA synchronous", [], |r| r.get::<_, u8>(0)))? < 2 {
        return Err(ProviderError::Persistence);
    }
    let tx = db(connection.transaction())?;
    let count:u64=db(tx.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('mdreader_provider_registry_meta','mdreader_provider_registry_sources','mdreader_provider_registry_snapshots','mdreader_provider_registry_rows','mdreader_provider_registry_seen','mdreader_provider_registry_state','mdreader_provider_registry_cache','mdreader_provider_registry_admissions')",[],|r|r.get(0)))?;
    if count == 0 {
        db(tx.execute_batch("CREATE TABLE mdreader_provider_registry_meta(id INTEGER PRIMARY KEY CHECK(id=1),version INTEGER NOT NULL,library_id TEXT NOT NULL,legacy_reference_fence INTEGER NOT NULL CHECK(legacy_reference_fence IN (0,1)));
CREATE TABLE mdreader_provider_registry_sources(identity TEXT PRIMARY KEY,binding BLOB NOT NULL,descriptor TEXT NOT NULL,digest BLOB NOT NULL,credential_reference TEXT NOT NULL,configuration_digest BLOB NOT NULL,enabled INTEGER NOT NULL,coverage TEXT NOT NULL,projection_bound INTEGER NOT NULL);
CREATE TABLE mdreader_provider_registry_snapshots(id TEXT PRIMARY KEY,source TEXT NOT NULL,binding BLOB NOT NULL,phase TEXT NOT NULL,list_cursor BLOB,change_cursor BLOB,observed INTEGER NOT NULL,coverage TEXT NOT NULL,base_epoch INTEGER NOT NULL);
CREATE UNIQUE INDEX mdreader_provider_registry_one_staging ON mdreader_provider_registry_snapshots(source) WHERE phase!='published';
CREATE TABLE mdreader_provider_registry_rows(snapshot TEXT NOT NULL,key TEXT NOT NULL,metadata TEXT NOT NULL,digest BLOB NOT NULL,PRIMARY KEY(snapshot,key));
CREATE TABLE mdreader_provider_registry_seen(snapshot TEXT NOT NULL,kind TEXT NOT NULL,digest BLOB NOT NULL,PRIMARY KEY(snapshot,kind,digest));
CREATE TABLE mdreader_provider_registry_state(source TEXT PRIMARY KEY,published TEXT,checkpoint BLOB,epoch INTEGER NOT NULL);
CREATE TABLE mdreader_provider_registry_cache(source TEXT NOT NULL,binding BLOB NOT NULL,key TEXT NOT NULL,version BLOB NOT NULL,body BLOB NOT NULL,digest BLOB NOT NULL,observed INTEGER NOT NULL,job TEXT,PRIMARY KEY(source,binding,key));
CREATE TABLE mdreader_provider_registry_admissions(reference_digest BLOB PRIMARY KEY CHECK(length(reference_digest)=32),configuration_digest BLOB NOT NULL CHECK(length(configuration_digest)=32),source_id TEXT NOT NULL,binding BLOB NOT NULL CHECK(length(binding)=32));"))?;
        db(tx.execute(
            "INSERT INTO mdreader_provider_registry_meta VALUES(1,2,?1,0)",
            [Uuid::new_v4().to_string()],
        ))?;
    } else if !matches!(count, 7 | 8) {
        return Err(ProviderError::Persistence);
    }
    let version: u64 = db(tx.query_row(
        "SELECT version FROM mdreader_provider_registry_meta WHERE id=1",
        [],
        |r| r.get(0),
    ))?;
    if count == 7 && version == 1 {
        // Prototype v1 stored only current refs. Preserve those; unknown refs
        // cannot safely be distinguished from retired pre-migration grants.
        let records = {
            let mut statement=db(tx.prepare("SELECT descriptor,digest,credential_reference,configuration_digest,binding FROM mdreader_provider_registry_sources LIMIT 129"))?;
            let rows = db(statement.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Vec<u8>>(3)?,
                    r.get::<_, Vec<u8>>(4)?,
                ))
            }))?;
            rows.map(db).collect::<Result<Vec<_>>>()?
        };
        if records.len() > MAX_SOURCES as usize {
            return Err(ProviderError::Persistence);
        }
        db(tx.execute_batch("ALTER TABLE mdreader_provider_registry_meta ADD COLUMN legacy_reference_fence INTEGER NOT NULL DEFAULT 0 CHECK(legacy_reference_fence IN (0,1));CREATE TABLE mdreader_provider_registry_admissions(reference_digest BLOB PRIMARY KEY CHECK(length(reference_digest)=32),configuration_digest BLOB NOT NULL CHECK(length(configuration_digest)=32),source_id TEXT NOT NULL,binding BLOB NOT NULL CHECK(length(binding)=32));"))?;
        let fence = !records.is_empty();
        for (text, digest, reference, configuration, binding) in records {
            if body_hash(text.as_bytes()).as_slice() != digest || configuration.len() != 32 {
                return Err(ProviderError::Persistence);
            }
            let descriptor: SourceDescriptor = decode(&text, 65536)?;
            if descriptor.binding().digest().as_slice() != binding {
                return Err(ProviderError::Persistence);
            }
            let reference = OpaqueValue::new(reference).map_err(|_| ProviderError::Persistence)?;
            db(tx.execute(
                "INSERT INTO mdreader_provider_registry_admissions VALUES(?1,?2,?3,?4)",
                params![
                    body_hash(reference.as_str().as_bytes()).as_slice(),
                    configuration,
                    descriptor.binding().source_id().as_str(),
                    binding
                ],
            ))?;
        }
        db(tx.execute("UPDATE mdreader_provider_registry_meta SET version=2,legacy_reference_fence=?1 WHERE id=1",[fence]))?;
    } else if version != 2 || !matches!(count, 0 | 8) {
        return Err(ProviderError::Persistence);
    }
    let fence: u8 = db(tx.query_row(
        "SELECT legacy_reference_fence FROM mdreader_provider_registry_meta WHERE id=1",
        [],
        |r| r.get(0),
    ))?;
    if fence > 1 {
        return Err(ProviderError::Persistence);
    }
    db(tx.prepare("SELECT reference_digest,configuration_digest,source_id,binding FROM mdreader_provider_registry_admissions LIMIT 0"))?;
    db(tx.commit())
}
pub(super) struct AdmissionReceipt {
    pub configuration_digest: [u8; 32],
    pub source_id: SourceId,
    pub binding: [u8; 32],
}
pub(super) fn admission_receipt(
    c: &Connection,
    reference: &OpaqueValue,
) -> Result<Option<AdmissionReceipt>> {
    type Row = (Vec<u8>, String, Vec<u8>);
    let row:Option<Row>=db(c.query_row("SELECT configuration_digest,source_id,binding FROM mdreader_provider_registry_admissions WHERE reference_digest=?1",[body_hash(reference.as_str().as_bytes()).as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional())?;
    row.map(|(configuration, source, binding)| {
        Ok(AdmissionReceipt {
            configuration_digest: configuration
                .try_into()
                .map_err(|_| ProviderError::Persistence)?,
            source_id: SourceId::new(source).map_err(|_| ProviderError::Persistence)?,
            binding: binding.try_into().map_err(|_| ProviderError::Persistence)?,
        })
    })
    .transpose()
}
fn record_admission(
    tx: &Transaction<'_>,
    descriptor: &SourceDescriptor,
    reference: &OpaqueValue,
    configuration: [u8; 32],
) -> Result<()> {
    if admission_receipt(tx, reference)?.is_some() {
        return Err(ProviderError::Conflict);
    }
    let fence: bool = db(tx.query_row(
        "SELECT legacy_reference_fence FROM mdreader_provider_registry_meta WHERE id=1",
        [],
        |r| r.get(0),
    ))?;
    if fence {
        return Err(ProviderError::Unsupported);
    }
    let count: u64 = db(tx.query_row(
        "SELECT count(*) FROM mdreader_provider_registry_admissions",
        [],
        |r| r.get(0),
    ))?;
    if count >= MAX_ADMISSION_RECEIPTS {
        return Err(ProviderError::TooLarge);
    }
    db(tx.execute(
        "INSERT INTO mdreader_provider_registry_admissions VALUES(?1,?2,?3,?4)",
        params![
            body_hash(reference.as_str().as_bytes()).as_slice(),
            configuration.as_slice(),
            descriptor.binding().source_id().as_str(),
            descriptor.binding().digest().as_slice()
        ],
    ))?;
    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSummary {
    pub source_id: SourceId,
    pub reader_source_id: Option<SourceId>,
    pub name: DisplayText,
    pub kind: ProviderKind,
    pub enabled: bool,
    pub selected_scope: SelectionSummary,
    pub capabilities: ProviderCapabilities,
    pub health: SourceHealth,
    pub coverage: Coverage,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectionSummary {
    ExactDocuments {
        count: usize,
    },
    ScopedRoot {
        recursive: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        root: Option<RecordKey>,
    },
    DriveFolder {
        root: RecordKey,
        recursive: bool,
    },
    ImportedLegacyBroad,
}
impl From<&SelectedScope> for SelectionSummary {
    fn from(scope: &SelectedScope) -> Self {
        match scope {
            SelectedScope::ExactDocuments { keys } => Self::ExactDocuments { count: keys.len() },
            SelectedScope::ScopedRoot { recursive, root } => Self::ScopedRoot {
                recursive: *recursive,
                root: Some(root.clone()),
            },
            SelectedScope::DriveFolder { root, recursive } => Self::DriveFolder {
                root: root.clone(),
                recursive: *recursive,
            },
            SelectedScope::ImportedLegacyBroad { .. } => Self::ImportedLegacyBroad,
        }
    }
}
/// Initial explicit admission only. Existing IDs cannot be implicitly regranted.
/// Descriptor coverage is untrusted and is always reset to NeverIndexed.
pub fn admit(
    connection: &mut Connection,
    descriptor: &SourceDescriptor,
    credential_reference: &OpaqueValue,
    configuration_digest: [u8; 32],
) -> Result<()> {
    initialize(connection)?;
    if descriptor.binding().library_id() != &library_id(connection)? {
        return Err(ProviderError::Unauthorized);
    }
    let text = encode(&normalized(descriptor)?, 64 * 1024)?;
    let tx = db(connection.transaction())?;
    let n: u64 = db(tx.query_row(
        "SELECT count(*) FROM mdreader_provider_registry_sources",
        [],
        |r| r.get(0),
    ))?;
    if n >= MAX_SOURCES {
        return Err(ProviderError::TooLarge);
    }
    let exists: bool = db(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM mdreader_provider_registry_sources WHERE identity=?1)",
        [identity(descriptor.binding())],
        |r| r.get(0),
    ))?;
    if exists {
        return Err(ProviderError::Conflict);
    }
    record_admission(&tx, descriptor, credential_reference, configuration_digest)?;
    db(tx.execute(
        "INSERT INTO mdreader_provider_registry_sources VALUES(?1,?2,?3,?4,?5,?6,1,?7,0)",
        params![
            identity(descriptor.binding()),
            descriptor.binding().digest().as_slice(),
            text,
            body_hash(text.as_bytes()).as_slice(),
            credential_reference.as_str(),
            configuration_digest.as_slice(),
            encode(&Coverage::NeverIndexed, MAX_METADATA)?
        ],
    ))?;
    db(tx.execute(
        "INSERT INTO mdreader_provider_registry_state VALUES(?1,NULL,NULL,0)",
        [identity(descriptor.binding())],
    ))?;
    db(tx.commit())
}
pub fn list(connection: &mut Connection, library: &LibraryId) -> Result<Vec<SourceSummary>> {
    initialize(connection)?;
    let mut statement=db(connection.prepare("SELECT descriptor,digest,enabled,coverage,projection_bound FROM mdreader_provider_registry_sources ORDER BY rowid LIMIT 129"))?;
    let records = db(statement.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Vec<u8>>(1)?,
            r.get::<_, bool>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, bool>(4)?,
        ))
    }))?;
    let mut result = Vec::new();
    for record in records {
        let (text, digest, enabled, coverage, bound) = db(record)?;
        if body_hash(text.as_bytes()).as_slice() != digest {
            return Err(ProviderError::Persistence);
        }
        let d: SourceDescriptor = decode(&text, 64 * 1024)?;
        if d.binding().library_id() != library {
            continue;
        }
        result.push(SourceSummary {
            source_id: d.binding().source_id().clone(),
            reader_source_id: if bound {
                Some(SourceId::new(format!(
                    "provider:{}",
                    hex(&d.binding().digest())
                ))?)
            } else {
                None
            },
            name: d.name().clone(),
            kind: d.kind(),
            enabled,
            selected_scope: d.scope().into(),
            capabilities: d.capabilities(),
            health: if enabled {
                d.health()
            } else {
                SourceHealth::Paused
            },
            coverage: decode(&coverage, MAX_METADATA)?,
        });
    }
    if result.len() > MAX_SOURCES as usize {
        return Err(ProviderError::Persistence);
    }
    Ok(result)
}
pub(super) fn descriptor_at(
    connection: &Connection,
    binding: &SourceBinding,
    enabled: bool,
) -> Result<SourceDescriptor> {
    let value:Option<(Vec<u8>,String,Vec<u8>,bool)>=db(connection.query_row("SELECT binding,descriptor,digest,enabled FROM mdreader_provider_registry_sources WHERE identity=?1",[identity(binding)],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional())?;
    let (bound, text, digest, is_enabled) = value.ok_or(ProviderError::Unavailable)?;
    if bound != binding.digest() || enabled && !is_enabled {
        return Err(ProviderError::Unauthorized);
    }
    if body_hash(text.as_bytes()).as_slice() != digest {
        return Err(ProviderError::Persistence);
    }
    let d: SourceDescriptor = decode(&text, 64 * 1024)?;
    if d.binding() != binding {
        return Err(ProviderError::Persistence);
    }
    Ok(d)
}
pub fn revalidate(connection: &mut Connection, binding: &SourceBinding) -> Result<()> {
    initialize(connection)?;
    descriptor_at(connection, binding, true).map(|_| ())
}
/// Core's authoritative revoke and registry disable commit atomically. Physical
/// cleanup is separate and cannot serve as the access gate. Never reports a
/// successful revocation while old core read/search/job authority remains live.
pub fn disable(
    connection: &mut Connection,
    binding: &SourceBinding,
    projection: &dyn ProjectionAdmission,
) -> Result<()> {
    initialize(connection)?;
    descriptor_at(connection, binding, false)?;
    let tx = db(connection.transaction())?;
    let bound: bool = db(tx.query_row(
        "SELECT projection_bound FROM mdreader_provider_registry_sources WHERE identity=?1",
        [identity(binding)],
        |r| r.get(0),
    ))?;
    if bound {
        projection.revoke(&tx, binding)?;
    }
    db(tx.execute(
        "UPDATE mdreader_provider_registry_sources SET enabled=0 WHERE identity=?1",
        [identity(binding)],
    ))?;
    db(tx.commit())
}
/// Explicit reconnect/account switch only, after old authority is disabled and
/// core cleanup has succeeded. A fresh generation is mandatory; old journal
/// evidence is retained under its original binding.
pub fn reconnect(
    connection: &mut Connection,
    descriptor: &SourceDescriptor,
    credential_reference: &OpaqueValue,
    configuration_digest: [u8; 32],
) -> Result<()> {
    initialize(connection)?;
    if descriptor.binding().library_id() != &library_id(connection)? {
        return Err(ProviderError::Unauthorized);
    }
    let id = identity(descriptor.binding());
    let (old, enabled): (String, bool) = db(connection.query_row(
        "SELECT descriptor,enabled FROM mdreader_provider_registry_sources WHERE identity=?1",
        [&id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ))?;
    let old: SourceDescriptor = decode(&old, 64 * 1024)?;
    if enabled || old.binding().generation() == descriptor.binding().generation() {
        return Err(ProviderError::Conflict);
    }
    let text = encode(&normalized(descriptor)?, 64 * 1024)?;
    let tx = db(connection.transaction())?;
    record_admission(&tx, descriptor, credential_reference, configuration_digest)?;
    clear_snapshots(&tx, &id)?;
    db(tx.execute(
        "DELETE FROM mdreader_provider_registry_cache WHERE source=?1",
        [&id],
    ))?;
    db(tx.execute("UPDATE mdreader_provider_registry_sources SET binding=?2,descriptor=?3,digest=?4,credential_reference=?5,configuration_digest=?6,enabled=1,projection_bound=0,coverage=?7 WHERE identity=?1",params![id,descriptor.binding().digest().as_slice(),text,body_hash(text.as_bytes()).as_slice(),credential_reference.as_str(),configuration_digest.as_slice(),encode(&Coverage::NeverIndexed,MAX_METADATA)?]))?;
    db(tx.execute("UPDATE mdreader_provider_registry_state SET published=NULL,checkpoint=NULL,epoch=epoch+1 WHERE source=?1",[id]))?;
    db(tx.commit())
}
fn clear_snapshots(tx: &Transaction<'_>, source: &str) -> Result<()> {
    db(tx.execute("DELETE FROM mdreader_provider_registry_rows WHERE snapshot IN (SELECT id FROM mdreader_provider_registry_snapshots WHERE source=?1)",[source]))?;
    db(tx.execute("DELETE FROM mdreader_provider_registry_seen WHERE snapshot IN (SELECT id FROM mdreader_provider_registry_snapshots WHERE source=?1)",[source]))?;
    db(tx.execute(
        "DELETE FROM mdreader_provider_registry_snapshots WHERE source=?1",
        [source],
    ))?;
    Ok(())
}
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub(super) id: Uuid,
    pub(super) binding: SourceBinding,
}
impl Snapshot {
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn binding(&self) -> &SourceBinding {
        &self.binding
    }
}
#[derive(Debug, Clone)]
pub struct SnapshotProgress {
    pub snapshot: Snapshot,
    pub enumerating: bool,
    pub list_cursor: Option<ProviderCursor>,
    pub change_cursor: Option<ProviderCursor>,
}
fn cursor(bytes: Option<Vec<u8>>) -> Result<Option<ProviderCursor>> {
    bytes.map(ProviderCursor::new).transpose()
}
pub fn resume(
    connection: &mut Connection,
    binding: &SourceBinding,
) -> Result<Option<SnapshotProgress>> {
    initialize(connection)?;
    descriptor_at(connection, binding, true)?;
    let record:Option<SnapshotRow>=db(connection.query_row("SELECT id,phase,list_cursor,change_cursor FROM mdreader_provider_registry_snapshots WHERE source=?1 AND binding=?2 AND phase!='published'",params![identity(binding),binding.digest().as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional())?;
    record
        .map(|(id, phase, list, changes)| {
            Ok(SnapshotProgress {
                snapshot: Snapshot {
                    id: Uuid::parse_str(&id).map_err(|_| ProviderError::Persistence)?,
                    binding: binding.clone(),
                },
                enumerating: phase == "enumerating",
                list_cursor: cursor(list)?,
                change_cursor: cursor(changes)?,
            })
        })
        .transpose()
}
pub fn checkpoint(
    connection: &mut Connection,
    binding: &SourceBinding,
) -> Result<Option<ProviderCursor>> {
    initialize(connection)?;
    descriptor_at(connection, binding, true)?;
    cursor(db(connection.query_row(
        "SELECT checkpoint FROM mdreader_provider_registry_state WHERE source=?1",
        [identity(binding)],
        |r| r.get(0),
    ))?)
}
pub fn begin_snapshot(
    connection: &mut Connection,
    binding: &SourceBinding,
    start_changes: Option<&ProviderCursor>,
) -> Result<Snapshot> {
    initialize(connection)?;
    let d = descriptor_at(connection, binding, true)?;
    if d.capabilities().changes != start_changes.is_some() {
        return Err(ProviderError::Invalid);
    }
    begin(connection, binding, "enumerating", start_changes, false)
}
pub fn begin_delta(connection: &mut Connection, binding: &SourceBinding) -> Result<Snapshot> {
    let checkpoint = checkpoint(connection, binding)?.ok_or(ProviderError::Invalid)?;
    begin(connection, binding, "reconciling", Some(&checkpoint), true)
}
fn begin(
    connection: &mut Connection,
    binding: &SourceBinding,
    phase: &str,
    start: Option<&ProviderCursor>,
    copy: bool,
) -> Result<Snapshot> {
    let snapshot = Snapshot {
        id: Uuid::new_v4(),
        binding: binding.clone(),
    };
    let tx = db(connection.transaction())?;
    let exists:bool=db(tx.query_row("SELECT EXISTS(SELECT 1 FROM mdreader_provider_registry_snapshots WHERE source=?1 AND phase!='published')",[identity(binding)],|r|r.get(0)))?;
    if exists {
        return Err(ProviderError::Conflict);
    }
    let epoch: u64 = db(tx.query_row(
        "SELECT epoch FROM mdreader_provider_registry_state WHERE source=?1",
        [identity(binding)],
        |r| r.get(0),
    ))?;
    db(tx.execute(
        "INSERT INTO mdreader_provider_registry_snapshots VALUES(?1,?2,?3,?4,NULL,?5,0,?6,?7)",
        params![
            snapshot.id.to_string(),
            identity(binding),
            binding.digest().as_slice(),
            phase,
            start.map(ProviderCursor::as_bytes),
            encode(&Coverage::NeverIndexed, MAX_METADATA)?,
            epoch
        ],
    ))?;
    if copy {
        db(tx.execute("INSERT INTO mdreader_provider_registry_rows SELECT ?1,key,metadata,digest FROM mdreader_provider_registry_rows WHERE snapshot=(SELECT published FROM mdreader_provider_registry_state WHERE source=?2)",params![snapshot.id.to_string(),identity(binding)]))?;
        db(tx.execute("UPDATE mdreader_provider_registry_snapshots SET coverage=(SELECT coverage FROM mdreader_provider_registry_sources WHERE identity=?2) WHERE id=?1",params![snapshot.id.to_string(),identity(binding)]))?;
    }
    let bytes: u64 = db(tx.query_row(
        "SELECT COALESCE(sum(length(metadata)),0) FROM mdreader_provider_registry_rows",
        [],
        |r| r.get(0),
    ))?;
    if bytes > MAX_METADATA_STORAGE {
        return Err(ProviderError::TooLarge);
    }
    db(tx.commit())?;
    Ok(snapshot)
}
fn ensure_snapshot(connection: &Connection, snapshot: &Snapshot, phase: &str) -> Result<()> {
    descriptor_at(connection, &snapshot.binding, true)?;
    let found:bool=db(connection.query_row("SELECT EXISTS(SELECT 1 FROM mdreader_provider_registry_snapshots WHERE id=?1 AND source=?2 AND binding=?3 AND phase=?4)",params![snapshot.id.to_string(),identity(&snapshot.binding),snapshot.binding.digest().as_slice(),phase],|r|r.get(0)))?;
    if !found {
        return Err(ProviderError::Conflict);
    }
    Ok(())
}
fn advance(
    tx: &Transaction<'_>,
    snapshot: &Snapshot,
    kind: &str,
    expected: Option<&ProviderCursor>,
    next: Option<&ProviderCursor>,
    terminal: bool,
) -> Result<()> {
    let column = if kind == "list" {
        "list_cursor"
    } else {
        "change_cursor"
    };
    let sql = format!("SELECT {column} FROM mdreader_provider_registry_snapshots WHERE id=?1");
    let current: Option<Vec<u8>> = db(tx.query_row(&sql, [snapshot.id.to_string()], |r| r.get(0)))?;
    if current.as_deref() != expected.map(ProviderCursor::as_bytes) {
        return Err(ProviderError::Conflict);
    }
    let pages: u64 = db(tx.query_row(
        "SELECT count(*) FROM mdreader_provider_registry_seen WHERE snapshot=?1 AND kind=?2",
        params![snapshot.id.to_string(), kind],
        |r| r.get(0),
    ))?;
    if pages >= MAX_PAGES {
        return Err(ProviderError::TooLarge);
    }
    let total_pages: u64 = db(tx.query_row(
        "SELECT count(*) FROM mdreader_provider_registry_seen",
        [],
        |r| r.get(0),
    ))?;
    if total_pages >= MAX_HISTORY_PAGES {
        return Err(ProviderError::TooLarge);
    }
    // Record the consumed cursor, including the initial empty cursor. A repeated
    // continuation cannot loop or reapply a page after a committed restart.
    let consumed = body_hash(expected.map(ProviderCursor::as_bytes).unwrap_or_default());
    let seen:bool=db(tx.query_row("SELECT EXISTS(SELECT 1 FROM mdreader_provider_registry_seen WHERE snapshot=?1 AND kind=?2 AND digest=?3)",params![snapshot.id.to_string(),kind,consumed.as_slice()],|r|r.get(0)))?;
    if seen || !terminal && next.is_some() && next == expected {
        return Err(ProviderError::Protocol);
    }
    db(tx.execute(
        "INSERT INTO mdreader_provider_registry_seen VALUES(?1,?2,?3)",
        params![snapshot.id.to_string(), kind, consumed.as_slice()],
    ))?;
    if let Some(next) = next {
        let seen:bool=db(tx.query_row("SELECT EXISTS(SELECT 1 FROM mdreader_provider_registry_seen WHERE snapshot=?1 AND kind=?2 AND digest=?3)",params![snapshot.id.to_string(),kind,body_hash(next.as_bytes()).as_slice()],|r|r.get(0)))?;
        if seen && !(terminal && Some(next) == expected) {
            return Err(ProviderError::Protocol);
        }
    }
    db(tx.execute(
        &format!("UPDATE mdreader_provider_registry_snapshots SET {column}=?2 WHERE id=?1"),
        params![snapshot.id.to_string(), next.map(ProviderCursor::as_bytes)],
    ))?;
    Ok(())
}
fn insert_row(tx: &Transaction<'_>, snapshot: &Snapshot, row: &Metadata) -> Result<()> {
    let d = descriptor_at(tx, &snapshot.binding, true)?;
    if matches!(d.scope(), SelectedScope::ExactDocuments { .. })
        && !d.scope().contains_exact(&row.key)
    {
        return Err(ProviderError::Unauthorized);
    }
    let text = encode(row, MAX_METADATA)?;
    db(tx.execute("INSERT INTO mdreader_provider_registry_rows VALUES(?1,?2,?3,?4) ON CONFLICT(snapshot,key) DO UPDATE SET metadata=excluded.metadata,digest=excluded.digest",params![snapshot.id.to_string(),row.key.as_str(),text,body_hash(text.as_bytes()).as_slice()]))?;
    let rows: u64 = db(tx.query_row(
        "SELECT count(*) FROM mdreader_provider_registry_rows WHERE snapshot=?1",
        [snapshot.id.to_string()],
        |r| r.get(0),
    ))?;
    let bytes: u64 = db(tx.query_row(
        "SELECT COALESCE(sum(length(metadata)),0) FROM mdreader_provider_registry_rows",
        [],
        |r| r.get(0),
    ))?;
    if rows > MAX_ROWS || bytes > MAX_METADATA_STORAGE {
        return Err(ProviderError::TooLarge);
    }
    Ok(())
}
pub fn stage_page(
    connection: &mut Connection,
    snapshot: &Snapshot,
    expected_cursor: Option<&ProviderCursor>,
    page: &MetadataPage,
) -> Result<()> {
    initialize(connection)?;
    ensure_snapshot(connection, snapshot, "enumerating")?;
    let tx = db(connection.transaction())?;
    advance(
        &tx,
        snapshot,
        "list",
        expected_cursor,
        page.next_cursor(),
        false,
    )?;
    for row in page.rows() {
        insert_row(&tx, snapshot, row)?
    }
    let coverage = match page.coverage() {
        Coverage::CompleteSelected { observed_at_ms } => Coverage::MetadataCompleteSelected {
            observed_at_ms: *observed_at_ms,
        },
        value => value.clone(),
    };
    db(tx.execute("UPDATE mdreader_provider_registry_snapshots SET phase=?2,observed=?3,coverage=?4 WHERE id=?1",params![snapshot.id.to_string(),if page.next_cursor().is_some(){"enumerating"}else{"reconciling"},page.observed_at_ms(),encode(&coverage,MAX_METADATA)?]))?;
    db(tx.commit())
}
#[derive(Debug, Clone)]
pub enum ResolvedChange {
    Upsert(Metadata),
    Deleted {
        key: RecordKey,
        expected_path: Option<RecordKey>,
    },
}
pub fn stage_changes(
    connection: &mut Connection,
    snapshot: &Snapshot,
    expected_cursor: &ProviderCursor,
    changes: &[ResolvedChange],
    next_cursor: Option<&ProviderCursor>,
    final_checkpoint: Option<&ProviderCursor>,
    observed_at_ms: i64,
) -> Result<()> {
    initialize(connection)?;
    ensure_snapshot(connection, snapshot, "reconciling")?;
    if changes.len() > MAX_PAGE_ROWS
        || observed_at_ms < 0
        || next_cursor.is_some() == final_checkpoint.is_some()
    {
        return Err(ProviderError::Invalid);
    }
    let tx = db(connection.transaction())?;
    advance(
        &tx,
        snapshot,
        "changes",
        Some(expected_cursor),
        next_cursor.or(final_checkpoint),
        final_checkpoint.is_some(),
    )?;
    for change in changes {
        match change {
            ResolvedChange::Upsert(row) => insert_row(&tx, snapshot, row)?,
            ResolvedChange::Deleted { key, expected_path } => {
                if let Some(path) = expected_path {
                    let current = metadata_in(&tx, &snapshot.id.to_string(), key)?;
                    if current
                        .as_ref()
                        .is_none_or(|m| m.display_path.as_str() != path.as_str())
                    {
                        continue;
                    }
                }
                db(tx.execute(
                    "DELETE FROM mdreader_provider_registry_rows WHERE snapshot=?1 AND key=?2",
                    params![snapshot.id.to_string(), key.as_str()],
                ))?;
            }
        }
    }
    let previous: String = db(tx.query_row(
        "SELECT coverage FROM mdreader_provider_registry_snapshots WHERE id=?1",
        [snapshot.id.to_string()],
        |r| r.get(0),
    ))?;
    let coverage = match decode::<Coverage>(&previous, MAX_METADATA)? {
        Coverage::CompleteSelected { .. } | Coverage::MetadataCompleteSelected { .. } => {
            Coverage::MetadataCompleteSelected { observed_at_ms }
        }
        partial => partial,
    };
    db(tx.execute("UPDATE mdreader_provider_registry_snapshots SET phase=?2,observed=MAX(observed,?3),coverage=?4 WHERE id=?1",params![snapshot.id.to_string(),if final_checkpoint.is_some(){"ready"}else{"reconciling"},observed_at_ms,encode(&coverage,MAX_METADATA)?]))?;
    db(tx.commit())
}
pub(super) fn metadata_in(
    connection: &Connection,
    snapshot: &str,
    key: &RecordKey,
) -> Result<Option<Metadata>> {
    let record:Option<(String,Vec<u8>)>=db(connection.query_row("SELECT metadata,digest FROM mdreader_provider_registry_rows WHERE snapshot=?1 AND key=?2",params![snapshot,key.as_str()],|r|Ok((r.get(0)?,r.get(1)?))).optional())?;
    record
        .map(|(text, digest)| {
            if body_hash(text.as_bytes()).as_slice() != digest {
                return Err(ProviderError::Persistence);
            }
            decode(&text, MAX_METADATA)
        })
        .transpose()
}
pub fn metadata(
    connection: &mut Connection,
    binding: &SourceBinding,
    key: &RecordKey,
) -> Result<Metadata> {
    initialize(connection)?;
    descriptor_at(connection, binding, true)?;
    published_metadata(connection, binding, key)
}
fn published_metadata(
    connection: &Connection,
    binding: &SourceBinding,
    key: &RecordKey,
) -> Result<Metadata> {
    let snapshot: Option<String> = db(connection.query_row(
        "SELECT published FROM mdreader_provider_registry_state WHERE source=?1",
        [identity(binding)],
        |r| r.get(0),
    ))?;
    metadata_in(
        connection,
        &snapshot.ok_or(ProviderError::Unavailable)?,
        key,
    )?
    .ok_or(ProviderError::NotFound)
}
/// Atomic metadata/checkpoint publication. Existing admitted bodies whose exact
/// version disappeared/changed are fenced by core in this same transaction.
pub fn publish_snapshot(
    connection: &mut Connection,
    snapshot: &Snapshot,
    projection: &dyn ProjectionAdmission,
) -> Result<()> {
    initialize(connection)?;
    let d = descriptor_at(connection, &snapshot.binding, true)?;
    ensure_snapshot(
        connection,
        snapshot,
        if d.capabilities().changes {
            "ready"
        } else {
            "reconciling"
        },
    )?;
    let tx = db(connection.transaction())?;
    let current:bool=db(tx.query_row("SELECT s.base_epoch=r.epoch FROM mdreader_provider_registry_snapshots s JOIN mdreader_provider_registry_state r ON r.source=s.source WHERE s.id=?1",[snapshot.id.to_string()],|r|r.get(0)))?;
    if !current {
        return Err(ProviderError::Conflict);
    }
    let admitted: Vec<(String, Vec<u8>)> = {
        let mut s=db(tx.prepare("SELECT key,version FROM mdreader_provider_registry_cache WHERE source=?1 AND binding=?2 AND job IS NOT NULL"))?;
        let rows = db(s.query_map(
            params![
                identity(&snapshot.binding),
                snapshot.binding.digest().as_slice()
            ],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ))?;
        rows.map(db).collect::<Result<_>>()?
    };
    for (key, version) in admitted {
        let key = RecordKey::new(key)?;
        let new = metadata_in(&tx, &snapshot.id.to_string(), &key)?;
        if new
            .as_ref()
            .is_none_or(|m| version_digest(&m.version).as_slice() != version)
        {
            projection.remove(&tx, &snapshot.binding, &key)?;
            db(tx.execute("UPDATE mdreader_provider_registry_cache SET job=NULL WHERE source=?1 AND binding=?2 AND key=?3",params![identity(&snapshot.binding),snapshot.binding.digest().as_slice(),key.as_str()]))?;
        }
    }
    let (checkpoint, coverage): (Option<Vec<u8>>, String) = db(tx.query_row(
        "SELECT change_cursor,coverage FROM mdreader_provider_registry_snapshots WHERE id=?1",
        [snapshot.id.to_string()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ))?;
    db(tx.execute("UPDATE mdreader_provider_registry_state SET published=?2,checkpoint=?3,epoch=epoch+1 WHERE source=?1",params![identity(&snapshot.binding),snapshot.id.to_string(),checkpoint]))?;
    db(tx.execute(
        "UPDATE mdreader_provider_registry_sources SET coverage=?2 WHERE identity=?1",
        params![identity(&snapshot.binding), coverage],
    ))?;
    db(tx.execute("DELETE FROM mdreader_provider_registry_rows WHERE snapshot IN (SELECT id FROM mdreader_provider_registry_snapshots WHERE source=?1 AND id!=?2)",params![identity(&snapshot.binding),snapshot.id.to_string()]))?;
    db(tx.execute("DELETE FROM mdreader_provider_registry_seen WHERE snapshot IN (SELECT id FROM mdreader_provider_registry_snapshots WHERE source=?1)",[identity(&snapshot.binding)]))?;
    db(tx.execute(
        "DELETE FROM mdreader_provider_registry_snapshots WHERE source=?1 AND id!=?2",
        params![identity(&snapshot.binding), snapshot.id.to_string()],
    ))?;
    db(tx.execute(
        "UPDATE mdreader_provider_registry_snapshots SET phase='published' WHERE id=?1",
        [snapshot.id.to_string()],
    ))?;
    db(tx.commit())
}
pub fn discard_staging(connection: &mut Connection, binding: &SourceBinding) -> Result<()> {
    initialize(connection)?;
    descriptor_at(connection, binding, true)?;
    let tx = db(connection.transaction())?;
    db(tx.execute("DELETE FROM mdreader_provider_registry_rows WHERE snapshot IN (SELECT id FROM mdreader_provider_registry_snapshots WHERE source=?1 AND phase!='published')",[identity(binding)]))?;
    db(tx.execute("DELETE FROM mdreader_provider_registry_seen WHERE snapshot IN (SELECT id FROM mdreader_provider_registry_snapshots WHERE source=?1 AND phase!='published')",[identity(binding)]))?;
    db(tx.execute(
        "DELETE FROM mdreader_provider_registry_snapshots WHERE source=?1 AND phase!='published'",
        [identity(binding)],
    ))?;
    db(tx.commit())
}
#[derive(Debug, Clone)]
pub struct CachedDocument {
    pub metadata: Metadata,
    pub document: DocumentBody,
    pub projection: Option<ProjectionReceipt>,
}
/// Private service cache; returns only the exact currently observed revision.
/// Observation time is explicit. A cache hit is never proof of current remote head.
pub fn cached(
    connection: &mut Connection,
    binding: &SourceBinding,
    key: &RecordKey,
) -> Result<Option<CachedDocument>> {
    initialize(connection)?;
    descriptor_at(connection, binding, true)?;
    let metadata = super::metadata_observation::metadata(connection, binding, key)?;
    let record:Option<CachedRow>=db(connection.query_row("SELECT version,body,digest,observed,job FROM mdreader_provider_registry_cache WHERE source=?1 AND binding=?2 AND key=?3",params![identity(binding),binding.digest().as_slice(),key.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional())?;
    let Some((version, body, digest, observed, job)) = record else {
        return Ok(None);
    };
    if version_digest(&metadata.version).as_slice() != version {
        return Ok(None);
    }
    if body_hash(&body).as_slice() != digest || observed < 0 {
        return Err(ProviderError::Persistence);
    }
    let body = MarkdownBytes::new(body)?;
    if let Some(hash) = &metadata.version.content_hash {
        verify_content_hash(body.as_bytes(), hash)?
    }
    Ok(Some(CachedDocument {
        document: DocumentBody {
            key: key.clone(),
            version: metadata.version.clone(),
            body,
            observed_at_ms: observed,
        },
        metadata,
        projection: job.map(|s| decode(&s, MAX_METADATA)).transpose()?,
    }))
}
fn validate_body(metadata: &Metadata, document: &DocumentBody) -> Result<()> {
    if metadata.key != document.key
        || metadata.version != document.version
        || document.observed_at_ms < 0
        || metadata
            .size_bytes
            .is_some_and(|n| n != document.body.as_bytes().len() as u64)
    {
        return Err(ProviderError::Conflict);
    }
    if let Some(hash) = &document.version.content_hash {
        verify_content_hash(document.body.as_bytes(), hash)?
    }
    Ok(())
}
fn store_body(
    tx: &Transaction<'_>,
    binding: &SourceBinding,
    metadata: &Metadata,
    document: &DocumentBody,
    projection: Option<&dyn ProjectionAdmission>,
) -> Result<Option<ProjectionReceipt>> {
    validate_body(metadata, document)?;
    let prior:Option<(Vec<u8>,Vec<u8>,Option<String>)>=db(tx.query_row("SELECT version,digest,job FROM mdreader_provider_registry_cache WHERE source=?1 AND binding=?2 AND key=?3",params![identity(binding),binding.digest().as_slice(),document.key.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional())?;
    if prior.as_ref().is_some_and(|(version, digest, _)| {
        version_digest(&document.version).as_slice() == version
            && body_hash(document.body.as_bytes()).as_slice() != digest
    }) {
        return Err(ProviderError::Conflict);
    }
    let receipt = if let Some(projection) = projection {
        Some(projection.admit(tx, binding, metadata, document)?)
    } else {
        prior
            .and_then(|(version, _, job)| {
                if version_digest(&document.version).as_slice() == version {
                    job
                } else {
                    None
                }
            })
            .map(|s| decode(&s, MAX_METADATA))
            .transpose()?
    };
    if receipt.as_ref().is_some_and(|r| {
        r.job_id.is_nil()
            || r.reader_source_id.as_str() != format!("provider:{}", hex(&binding.digest()))
            || r.path.as_str().starts_with('/')
            || r.path
                .as_str()
                .split('/')
                .any(|part| part.is_empty() || part == ".." || part == ".")
            || r.path.as_str().contains('\\')
    }) {
        return Err(ProviderError::Protocol);
    }
    db(tx.execute("INSERT INTO mdreader_provider_registry_cache VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(source,binding,key) DO UPDATE SET version=excluded.version,body=excluded.body,digest=excluded.digest,observed=excluded.observed,job=excluded.job",params![identity(binding),binding.digest().as_slice(),document.key.as_str(),version_digest(&document.version).as_slice(),document.body.as_bytes(),body_hash(document.body.as_bytes()).as_slice(),document.observed_at_ms,receipt.as_ref().map(|r|encode(r,MAX_METADATA)).transpose()?]))?;
    if receipt.is_some() {
        db(tx.execute(
            "UPDATE mdreader_provider_registry_sources SET projection_bound=1 WHERE identity=?1",
            [identity(binding)],
        ))?;
    }
    let bytes: u64 = db(tx.query_row(
        "SELECT COALESCE(sum(length(body)),0) FROM mdreader_provider_registry_cache",
        [],
        |r| r.get(0),
    ))?;
    if bytes > MAX_CACHE_STORAGE {
        return Err(ProviderError::TooLarge);
    }
    Ok(receipt)
}
/// Durable cache only; no searchable/body-complete coverage is claimed.
pub fn cache_body(
    connection: &mut Connection,
    binding: &SourceBinding,
    document: &DocumentBody,
) -> Result<()> {
    initialize(connection)?;
    descriptor_at(connection, binding, true)?;
    let metadata = super::metadata_observation::metadata(connection, binding, &document.key)?;
    let tx = db(connection.transaction())?;
    store_body(&tx, binding, &metadata, document, None)?;
    db(tx.commit())
}
pub fn admit_body(
    connection: &mut Connection,
    binding: &SourceBinding,
    document: &DocumentBody,
    projection: &dyn ProjectionAdmission,
) -> Result<ProjectionReceipt> {
    initialize(connection)?;
    descriptor_at(connection, binding, true)?;
    let metadata = super::metadata_observation::metadata(connection, binding, &document.key)?;
    let tx = db(connection.transaction())?;
    let receipt = store_body(&tx, binding, &metadata, document, Some(projection))?
        .ok_or(ProviderError::Unsupported)?;
    db(tx.commit())?;
    Ok(receipt)
}
pub fn prepare_write(
    connection: &mut Connection,
    binding: &SourceBinding,
    key: &RecordKey,
    expected: &DocumentVersion,
    body: &MarkdownBytes,
) -> Result<Uuid> {
    initialize(connection)?;
    let d = descriptor_at(connection, binding, true)?;
    if !d.capabilities().guarded_write {
        return Err(ProviderError::Unsupported);
    }
    if published_metadata(connection, binding, key)?.version != *expected {
        return Err(ProviderError::Conflict);
    }
    journal::prepare(connection, binding, key, expected, body)
}
/// Repair publication after a verified accepted receipt. Cache+metadata+core job
/// commit precedes the journal's published marker. Replays are idempotent and
/// third-party metadata heads are never overwritten by a stale accepted write.
pub fn publish_mutation(
    connection: &mut Connection,
    binding: &SourceBinding,
    operation: Uuid,
    projection: &dyn ProjectionAdmission,
    observed_at_ms: i64,
) -> Result<ProjectionReceipt> {
    initialize(connection)?;
    descriptor_at(connection, binding, true)?;
    let pending = journal::load(connection, binding, operation)?;
    if !matches!(
        pending.state,
        journal::MutationState::Accepted | journal::MutationState::Published
    ) {
        return Err(ProviderError::LocalPublicationPending);
    }
    if let Some(cache) = cached(connection, binding, &pending.key)? {
        if let Some(receipt) = cache.projection {
            if pending
                .receipt
                .as_ref()
                .is_some_and(|accepted| accepted.version == cache.document.version)
                && cache.document.body == pending.body
            {
                journal::mark_published(connection, binding, operation)?;
                return Ok(receipt);
            }
        }
    }
    let receipt = pending.receipt.ok_or(ProviderError::Persistence)?;
    let mut metadata = published_metadata(connection, binding, &pending.key)?;
    if metadata.version != pending.expected && metadata.version != receipt.version {
        return Err(ProviderError::LocalPublicationPending);
    }
    metadata.version = receipt.version.clone();
    metadata.size_bytes = Some(receipt.size_bytes);
    let document = DocumentBody {
        key: pending.key,
        version: receipt.version,
        body: pending.body,
        observed_at_ms,
    };
    let tx = db(connection.transaction())?;
    let published: String = db(tx.query_row(
        "SELECT published FROM mdreader_provider_registry_state WHERE source=?1",
        [identity(binding)],
        |r| r.get(0),
    ))?;
    insert_row(
        &tx,
        &Snapshot {
            id: Uuid::parse_str(&published).map_err(|_| ProviderError::Persistence)?,
            binding: binding.clone(),
        },
        &metadata,
    )?;
    let admission = store_body(&tx, binding, &metadata, &document, Some(projection))?
        .ok_or(ProviderError::Unsupported)?;
    db(tx.execute(
        "UPDATE mdreader_provider_registry_state SET epoch=epoch+1 WHERE source=?1",
        [identity(binding)],
    ))?;
    db(tx.commit())?;
    journal::mark_published(connection, binding, operation)?;
    Ok(admission)
}

/// Stable private profile/library identity. Generated once by the service owner;
/// renderer source descriptors cannot choose a different library.
pub fn library_id(connection: &mut Connection) -> Result<LibraryId> {
    initialize(connection)?;
    LibraryId::new(db(connection.query_row(
        "SELECT library_id FROM mdreader_provider_registry_meta WHERE id=1",
        [],
        |r| r.get::<_, String>(0),
    ))?)
    .map_err(|_| ProviderError::Persistence)
}
pub(super) fn ready(connection: &mut Connection, snapshot: &Snapshot) -> Result<bool> {
    initialize(connection)?;
    descriptor_at(connection, &snapshot.binding, true)?;
    db(connection.query_row(
        "SELECT phase='ready' FROM mdreader_provider_registry_snapshots WHERE id=?1 AND binding=?2",
        params![
            snapshot.id.to_string(),
            snapshot.binding.digest().as_slice()
        ],
        |r| r.get(0),
    ))
}

/// Forget derived metadata/body/index materialization after authoritative disable.
/// Source selection/account authority stays paused for explicit reconnect; pending
/// mutation intent/accepted receipts/conflicts are deliberately retained.
pub fn forget_materialization(
    connection: &mut Connection,
    binding: &SourceBinding,
    projection: &dyn ProjectionAdmission,
) -> Result<()> {
    disable(connection, binding, projection)?;
    let tx = db(connection.transaction())?;
    clear_snapshots(&tx, &identity(binding))?;
    db(tx.execute(
        "DELETE FROM mdreader_provider_registry_cache WHERE source=?1",
        [identity(binding)],
    ))?;
    db(tx.execute("UPDATE mdreader_provider_registry_state SET published=NULL,checkpoint=NULL,epoch=epoch+1 WHERE source=?1",[identity(binding)]))?;
    db(tx.execute(
        "UPDATE mdreader_provider_registry_sources SET coverage=?2 WHERE identity=?1",
        params![
            identity(binding),
            encode(&Coverage::NeverIndexed, MAX_METADATA)?
        ],
    ))?;
    db(tx.commit())
}

pub(super) fn snapshot_is_current(
    connection: &mut Connection,
    snapshot: &Snapshot,
) -> Result<bool> {
    initialize(connection)?;
    descriptor_at(connection, &snapshot.binding, true)?;
    db(connection.query_row("SELECT s.base_epoch=r.epoch FROM mdreader_provider_registry_snapshots s JOIN mdreader_provider_registry_state r ON r.source=s.source WHERE s.id=?1 AND s.binding=?2",params![snapshot.id.to_string(),snapshot.binding.digest().as_slice()],|r|r.get(0)))
}
/// Expired/reset cursors require a new enumeration, not repeated polling of the
/// rejected checkpoint. Old published/cache observations remain explicitly
/// partial until a replacement snapshot is durably published.
pub fn reset_for_rebuild(connection: &mut Connection, binding: &SourceBinding) -> Result<()> {
    discard_staging(connection, binding)?;
    let tx = db(connection.transaction())?;
    db(tx.execute(
        "UPDATE mdreader_provider_registry_state SET checkpoint=NULL,epoch=epoch+1 WHERE source=?1",
        [identity(binding)],
    ))?;
    db(tx.execute(
        "UPDATE mdreader_provider_registry_sources SET coverage=?2 WHERE identity=?1",
        params![
            identity(binding),
            encode(
                &Coverage::Partial {
                    reason: DisplayText::new(
                        "Provider cursor reset; replacement enumeration required"
                    )?
                },
                MAX_METADATA
            )?
        ],
    ))?;
    db(tx.commit())
}

fn normalized(descriptor: &SourceDescriptor) -> Result<SourceDescriptor> {
    SourceDescriptor::new(
        descriptor.binding().clone(),
        descriptor.kind(),
        descriptor.name().clone(),
        descriptor.scope().clone(),
        descriptor.capabilities(),
        SourceHealth::Ready,
        Coverage::NeverIndexed,
    )
}
/// Private owner-only factory input. Not serializable into renderer responses;
/// carries a credential reference, never a credential/token.
pub struct SourceConfiguration {
    pub descriptor: SourceDescriptor,
    pub credential_reference: OpaqueValue,
    pub configuration_digest: [u8; 32],
}
pub fn configuration(
    connection: &mut Connection,
    source: &SourceId,
) -> Result<SourceConfiguration> {
    let library = library_id(connection)?;
    let id = hex(&body_hash(
        &serde_json::to_vec(&(&library, source)).map_err(|_| ProviderError::Invalid)?,
    ));
    let (text,digest,credential,configuration):(String,Vec<u8>,String,Vec<u8>)=db(connection.query_row("SELECT descriptor,digest,credential_reference,configuration_digest FROM mdreader_provider_registry_sources WHERE identity=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))))?;
    if body_hash(text.as_bytes()).as_slice() != digest {
        return Err(ProviderError::Persistence);
    }
    Ok(SourceConfiguration {
        descriptor: decode(&text, 64 * 1024)?,
        credential_reference: OpaqueValue::new(credential)
            .map_err(|_| ProviderError::Persistence)?,
        configuration_digest: configuration
            .try_into()
            .map_err(|_| ProviderError::Persistence)?,
    })
}
