use crate::*;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
macro_rules! opaque_text {
    ($name:ident,$max:expr) => {
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);
        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                if value.is_empty() || value.len() > $max || value.chars().any(char::is_control) {
                    return Err(ProviderError::Invalid);
                }
                Ok(Self(value))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                Self::new(String::deserialize(d)?).map_err(serde::de::Error::custom)
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($name))
                    .field("bytes", &self.0.len())
                    .finish()
            }
        }
    };
}
opaque_text!(SourceId, 256);
opaque_text!(LibraryId, 256);
opaque_text!(AccountBinding, 256);
opaque_text!(RecordKey, 4096);
opaque_text!(OpaqueValue, 4096);
opaque_text!(DisplayText, 4096);
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "BindingWire")]
pub struct SourceBinding {
    library_id: LibraryId,
    source_id: SourceId,
    account: AccountBinding,
    generation: uuid::Uuid,
    selection_digest: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingWire {
    library_id: LibraryId,
    source_id: SourceId,
    account: AccountBinding,
    generation: uuid::Uuid,
    selection_digest: String,
}
impl TryFrom<BindingWire> for SourceBinding {
    type Error = ProviderError;
    fn try_from(w: BindingWire) -> Result<Self> {
        Self::new(
            w.library_id,
            w.source_id,
            w.account,
            w.generation,
            w.selection_digest,
        )
    }
}
impl SourceBinding {
    pub fn new(
        library_id: LibraryId,
        source_id: SourceId,
        account: AccountBinding,
        generation: uuid::Uuid,
        selection_digest: String,
    ) -> Result<Self> {
        if generation.is_nil()
            || selection_digest.len() != 64
            || !selection_digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self {
            library_id,
            source_id,
            account,
            generation,
            selection_digest,
        })
    }
    pub fn library_id(&self) -> &LibraryId {
        &self.library_id
    }
    pub fn source_id(&self) -> &SourceId {
        &self.source_id
    }
    pub fn account(&self) -> &AccountBinding {
        &self.account
    }
    pub fn generation(&self) -> uuid::Uuid {
        self.generation
    }
    pub fn selection_digest(&self) -> &str {
        &self.selection_digest
    }
    pub fn digest(&self) -> [u8; 32] {
        body_hash(&serde_json::to_vec(self).expect("fixed serializable binding"))
    }
}
impl fmt::Debug for SourceBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceBinding")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    GoogleDrive,
    Dropbox,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SelectionWire", tag = "kind", rename_all = "snake_case")]
pub enum SelectedScope {
    ExactDocuments { keys: Vec<RecordKey> },
    ScopedRoot { root: RecordKey, recursive: bool },
    DriveFolder { root: RecordKey, recursive: bool },
    ImportedLegacyBroad { configuration_digest: String },
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SelectionWire {
    ExactDocuments { keys: Vec<RecordKey> },
    ScopedRoot { root: RecordKey, recursive: bool },
    DriveFolder { root: RecordKey, recursive: bool },
    ImportedLegacyBroad { configuration_digest: String },
}
impl TryFrom<SelectionWire> for SelectedScope {
    type Error = ProviderError;
    fn try_from(w: SelectionWire) -> Result<Self> {
        match w {
            SelectionWire::ExactDocuments { keys } => Self::exact(keys),
            SelectionWire::ScopedRoot { root, recursive } => {
                Ok(Self::ScopedRoot { root, recursive })
            }
            SelectionWire::DriveFolder { root, recursive } => {
                let scope = Self::DriveFolder { root, recursive };
                scope.validate()?;
                Ok(scope)
            }
            SelectionWire::ImportedLegacyBroad {
                configuration_digest,
            } => Self::imported_legacy(configuration_digest),
        }
    }
}
impl SelectedScope {
    pub fn exact(keys: Vec<RecordKey>) -> Result<Self> {
        if keys.is_empty()
            || keys.len() > MAX_SELECTIONS
            || keys.iter().collect::<std::collections::BTreeSet<_>>().len() != keys.len()
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self::ExactDocuments { keys })
    }
    pub fn imported_legacy(configuration_digest: String) -> Result<Self> {
        if configuration_digest.len() != 64
            || !configuration_digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self::ImportedLegacyBroad {
            configuration_digest,
        })
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::ExactDocuments { keys } => Self::exact(keys.clone()).map(|_| ()),
            Self::ImportedLegacyBroad {
                configuration_digest,
            } => Self::imported_legacy(configuration_digest.clone()).map(|_| ()),
            Self::ScopedRoot { .. } => Ok(()),
            Self::DriveFolder { root, .. } => {
                if root.as_str().len() > 256
                    || !root
                        .as_str()
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                {
                    return Err(ProviderError::Invalid);
                }
                Ok(())
            }
        }
    }
    pub fn digest(&self) -> String {
        hex(&body_hash(
            &serde_json::to_vec(self).expect("fixed selection"),
        ))
    }
    pub fn contains_exact(&self, key: &RecordKey) -> bool {
        matches!(self,Self::ExactDocuments{keys}if keys.contains(key))
    }
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProviderCapabilities {
    pub read: bool,
    pub list: bool,
    pub changes: bool,
    pub assets: bool,
    pub guarded_write: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Coverage {
    NeverIndexed,
    Partial {
        reason: DisplayText,
    },
    /// Enumeration completed; bodies/indexing may still be absent or partial.
    MetadataCompleteSelected {
        observed_at_ms: i64,
    },
    /// Reserved for owner-confirmed complete body/index coverage.
    CompleteSelected {
        observed_at_ms: i64,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceHealth {
    Ready,
    Paused,
    ReconnectRequired,
    Unavailable,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "DescriptorWire")]
pub struct SourceDescriptor {
    binding: SourceBinding,
    kind: ProviderKind,
    name: DisplayText,
    scope: SelectedScope,
    capabilities: ProviderCapabilities,
    health: SourceHealth,
    coverage: Coverage,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DescriptorWire {
    binding: SourceBinding,
    kind: ProviderKind,
    name: DisplayText,
    scope: SelectedScope,
    capabilities: ProviderCapabilities,
    health: SourceHealth,
    coverage: Coverage,
}
impl TryFrom<DescriptorWire> for SourceDescriptor {
    type Error = ProviderError;
    fn try_from(w: DescriptorWire) -> Result<Self> {
        Self::new(
            w.binding,
            w.kind,
            w.name,
            w.scope,
            w.capabilities,
            w.health,
            w.coverage,
        )
    }
}
impl SourceDescriptor {
    pub fn new(
        binding: SourceBinding,
        kind: ProviderKind,
        name: DisplayText,
        scope: SelectedScope,
        capabilities: ProviderCapabilities,
        health: SourceHealth,
        coverage: Coverage,
    ) -> Result<Self> {
        scope.validate()?;
        if binding.selection_digest() != scope.digest() {
            return Err(ProviderError::IdentityMismatch);
        }
        if matches!(coverage,Coverage::CompleteSelected{observed_at_ms}|Coverage::MetadataCompleteSelected{observed_at_ms}if observed_at_ms<0)
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self {
            binding,
            kind,
            name,
            scope,
            capabilities,
            health,
            coverage,
        })
    }
    pub fn binding(&self) -> &SourceBinding {
        &self.binding
    }
    pub fn kind(&self) -> ProviderKind {
        self.kind
    }
    pub fn name(&self) -> &DisplayText {
        &self.name
    }
    pub fn health(&self) -> SourceHealth {
        self.health
    }
    pub fn scope(&self) -> &SelectedScope {
        &self.scope
    }
    pub fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities
    }
    pub fn coverage(&self) -> &Coverage {
        &self.coverage
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HashAlgorithm {
    Sha256,
    Md5,
    DropboxContentHash,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "HashWire")]
pub struct ContentHash {
    algorithm: HashAlgorithm,
    value: Vec<u8>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HashWire {
    algorithm: HashAlgorithm,
    value: Vec<u8>,
}
impl TryFrom<HashWire> for ContentHash {
    type Error = ProviderError;
    fn try_from(w: HashWire) -> Result<Self> {
        Self::new(w.algorithm, w.value)
    }
}
impl ContentHash {
    pub fn new(algorithm: HashAlgorithm, value: Vec<u8>) -> Result<Self> {
        if value.len()
            != if algorithm == HashAlgorithm::Md5 {
                16
            } else {
                32
            }
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self { algorithm, value })
    }
    pub fn algorithm(&self) -> HashAlgorithm {
        self.algorithm
    }
    pub fn value(&self) -> &[u8] {
        &self.value
    }
    pub fn from_hex(algorithm: HashAlgorithm, value: &str) -> Result<Self> {
        if !value.is_ascii() || !value.len().is_multiple_of(2) || value.len() > 64 {
            return Err(ProviderError::Invalid);
        }
        let bytes = (0..value.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&value[i..i + 2], 16).map_err(|_| ProviderError::Invalid))
            .collect::<Result<Vec<_>>>()?;
        Self::new(algorithm, bytes)
    }
}
impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContentHash")
            .field("algorithm", &self.algorithm)
            .finish_non_exhaustive()
    }
}
/// Three authorities with distinct meanings. A checksum must never become an
/// HTTP conditional validator or be presented as a provider revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentVersion {
    pub provider_revision: OpaqueValue,
    pub content_hash: Option<ContentHash>,
    pub conditional_write_token: Option<OpaqueValue>,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ProviderCursor(Vec<u8>);
impl ProviderCursor {
    pub fn new(bytes: Vec<u8>) -> Result<Self> {
        if bytes.is_empty() || bytes.len() > MAX_CURSOR_BYTES {
            return Err(ProviderError::Invalid);
        }
        Ok(Self(bytes))
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}
impl<'de> Deserialize<'de> for ProviderCursor {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::new(Vec::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Debug for ProviderCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderCursor")
            .field("bytes", &self.0.len())
            .finish()
    }
}
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct MarkdownBytes(Vec<u8>);
impl MarkdownBytes {
    pub fn new(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() > MAX_BODY_BYTES {
            return Err(ProviderError::TooLarge);
        }
        std::str::from_utf8(&bytes).map_err(|_| ProviderError::InvalidUtf8)?;
        Ok(Self(bytes))
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}
impl<'de> Deserialize<'de> for MarkdownBytes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::new(Vec::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Debug for MarkdownBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MarkdownBytes")
            .field("bytes", &self.0.len())
            .finish()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    /// Retained selected-root ancestry for bounded structural delta repair.
    /// Native provider observation only; never a grant or renderer path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub membership_ancestors: Option<Vec<RecordKey>>,
    pub key: RecordKey,
    pub name: DisplayText,
    pub display_path: DisplayText,
    pub media_type: Option<DisplayText>,
    pub modified_at_ms: Option<i64>,
    pub size_bytes: Option<u64>,
    pub version: DocumentVersion,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "PageWire")]
pub struct MetadataPage {
    rows: Vec<Metadata>,
    next_cursor: Option<ProviderCursor>,
    observed_at_ms: i64,
    coverage: Coverage,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageWire {
    rows: Vec<Metadata>,
    next_cursor: Option<ProviderCursor>,
    observed_at_ms: i64,
    coverage: Coverage,
}
impl TryFrom<PageWire> for MetadataPage {
    type Error = ProviderError;
    fn try_from(w: PageWire) -> Result<Self> {
        Self::new(w.rows, w.next_cursor, w.observed_at_ms, w.coverage)
    }
}
impl MetadataPage {
    pub fn new(
        rows: Vec<Metadata>,
        next_cursor: Option<ProviderCursor>,
        observed_at_ms: i64,
        coverage: Coverage,
    ) -> Result<Self> {
        if rows.len() > MAX_PAGE_ROWS
            || observed_at_ms < 0
            || rows.iter().any(|r| {
                r.membership_ancestors.as_ref().is_some_and(|path| {
                    path.is_empty()
                        || path.len() > 64
                        || path.contains(&r.key)
                        || path.iter().collect::<std::collections::BTreeSet<_>>().len()
                            != path.len()
                })
            })
            || rows
                .iter()
                .map(|r| &r.key)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != rows.len()
            || next_cursor.is_some()
                && matches!(
                    coverage,
                    Coverage::CompleteSelected { .. } | Coverage::MetadataCompleteSelected { .. }
                )
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self {
            rows,
            next_cursor,
            observed_at_ms,
            coverage,
        })
    }
    pub fn rows(&self) -> &[Metadata] {
        &self.rows
    }
    pub fn observed_at_ms(&self) -> i64 {
        self.observed_at_ms
    }
    pub fn next_cursor(&self) -> Option<&ProviderCursor> {
        self.next_cursor.as_ref()
    }
    pub fn coverage(&self) -> &Coverage {
        &self.coverage
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Change {
    Upsert {
        key: RecordKey,
    },
    Deleted {
        key: RecordKey,
        /// Path-based providers fence a tombstone to its published locator.
        /// Owners must ignore it when that stable ID now has a different path.
        expected_path: Option<RecordKey>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "ChangeWire", tag = "kind", rename_all = "snake_case")]
pub enum ChangeOutcome {
    Page {
        events: Vec<Change>,
        next_cursor: Option<ProviderCursor>,
        checkpoint: Option<ProviderCursor>,
    },
    RebuildRequired,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ChangeWire {
    Page {
        events: Vec<Change>,
        next_cursor: Option<ProviderCursor>,
        checkpoint: Option<ProviderCursor>,
    },
    RebuildRequired,
}
impl TryFrom<ChangeWire> for ChangeOutcome {
    type Error = ProviderError;
    fn try_from(w: ChangeWire) -> Result<Self> {
        match w {
            ChangeWire::RebuildRequired => Ok(Self::RebuildRequired),
            ChangeWire::Page {
                events,
                next_cursor,
                checkpoint,
            } => Self::page(events, next_cursor, checkpoint),
        }
    }
}
impl ChangeOutcome {
    pub fn page(
        events: Vec<Change>,
        next_cursor: Option<ProviderCursor>,
        checkpoint: Option<ProviderCursor>,
    ) -> Result<Self> {
        if events.len() > MAX_PAGE_ROWS || next_cursor.is_some() == checkpoint.is_some() {
            return Err(ProviderError::Invalid);
        }
        Ok(Self::Page {
            events,
            next_cursor,
            checkpoint,
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentBody {
    pub key: RecordKey,
    pub version: DocumentVersion,
    pub body: MarkdownBytes,
    pub observed_at_ms: i64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteReceipt {
    pub operation_id: uuid::Uuid,
    pub key: RecordKey,
    pub version: DocumentVersion,
    pub body_hash: [u8; 32],
    pub size_bytes: u64,
}
/// Assets remain unsupported until a provider implements explicit authority and
/// bounded retrieval; raw arbitrary URLs are never fetched by the contract.
#[derive(Clone)]
pub struct AssetBody {
    pub media_type: DisplayText,
    pub body: Vec<u8>,
}
impl fmt::Debug for AssetBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssetBody")
            .field("bytes", &self.body.len())
            .finish()
    }
}
