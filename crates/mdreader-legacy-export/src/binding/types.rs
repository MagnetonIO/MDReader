use crate::reviews::{ArchiveIdentity, ArchivedReview};
use serde::Serialize;
use uuid::Uuid;

pub const MAX_CURRENT_DOCUMENT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CurrentReadError {
    #[error("current reader integration unavailable")]
    Unsupported,
    #[error("selected current document unavailable")]
    Unavailable,
    #[error("selected current document or authority changed")]
    Conflict,
    #[error("current document exceeds comparison bounds")]
    Bounds,
}

/// Errors never include locators, document bodies, reviewer values or OS errors.
#[derive(Debug, thiserror::Error)]
pub enum BindingError {
    #[error("archival comparison unsupported on this host")]
    UnsupportedHost,
    #[error("invalid explicit archival or current selection")]
    InvalidSelection,
    #[error("selected archival record unavailable or integrity check failed")]
    ArchiveUnavailable,
    #[error("selected record belongs to a different archive")]
    ArchiveMismatch,
    #[error("archived assertion is not supported for text comparison")]
    UnsupportedAssertion,
    #[error("archived hash algorithm or encoding is not supported")]
    UnsupportedHash,
    #[error("current reader belongs to a different selected owner")]
    OwnerMismatch,
    #[error("current reader returned a different selected reference")]
    ReferenceMismatch,
    #[error("archived inclusive line range is absent from current document")]
    SpanUnavailable,
    #[error(transparent)]
    Current(#[from] CurrentReadError),
}

/// Constructed from an explicit current owner selection, never an old locator.
/// Revision is an opaque exact owner revision, including provider revisions.
/// Not `Deserialize`: untrusted input must pass the bounded constructor.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentDocumentReference {
    owner_scope: Uuid,
    source_id: String,
    path: String,
    revision: String,
}
impl CurrentDocumentReference {
    pub fn new(
        owner_scope: Uuid,
        source_id: String,
        path: String,
        revision: String,
    ) -> Result<Self, BindingError> {
        if owner_scope.get_version_num() != 4
            || owner_scope.get_variant() != uuid::Variant::RFC4122
            || source_id.is_empty()
            || source_id.len() > 256
            || source_id.chars().any(char::is_control)
            || path.is_empty()
            || path.len() > 4096
            || path.contains(['\\', '\0'])
            || path.split('/').any(|part| matches!(part, "" | "." | ".."))
            || !hex_digest(&revision)
        {
            return Err(BindingError::InvalidSelection);
        }
        Ok(Self {
            owner_scope,
            source_id,
            path,
            revision,
        })
    }
    pub fn owner_scope(&self) -> Uuid {
        self.owner_scope
    }
    pub fn source_id(&self) -> &str {
        &self.source_id
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn revision(&self) -> &str {
        &self.revision
    }
}

#[derive(Clone)]
pub struct SelectedBinding {
    pub(super) archive: ArchiveIdentity,
    pub(super) record_id: String,
    pub(super) current: CurrentDocumentReference,
}
impl SelectedBinding {
    pub fn new(
        mut archive: ArchiveIdentity,
        mut record_id: String,
        current: CurrentDocumentReference,
    ) -> Result<Self, BindingError> {
        if archive.export_id.get_version_num() != 4
            || archive.export_id.get_variant() != uuid::Variant::RFC4122
            || !hex_digest(&archive.manifest_sha256)
            || !hex_digest(&record_id)
        {
            return Err(BindingError::InvalidSelection);
        }
        archive.manifest_sha256.make_ascii_lowercase();
        record_id.make_ascii_lowercase();
        Ok(Self {
            archive,
            record_id,
            current,
        })
    }
    pub fn current(&self) -> &CurrentDocumentReference {
        &self.current
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrentObservation {
    LocalCurrentRead,
    /// Exact admitted cache revision; does not assert the remote provider head.
    ProviderCachedRevision,
}

/// Trusted owner response. Body is deliberately not serializable or Debug.
pub struct CurrentDocument {
    pub(super) reference: CurrentDocumentReference,
    pub(super) markdown: String,
    pub(super) observation: CurrentObservation,
}
impl CurrentDocument {
    pub fn new(
        reference: CurrentDocumentReference,
        markdown: String,
        observation: CurrentObservation,
    ) -> Result<Self, CurrentReadError> {
        if markdown.len() > MAX_CURRENT_DOCUMENT_BYTES {
            return Err(CurrentReadError::Bounds);
        }
        if reference.source_id().starts_with("provider:")
            != (observation == CurrentObservation::ProviderCachedRevision)
        {
            return Err(CurrentReadError::Conflict);
        }
        if observation == CurrentObservation::LocalCurrentRead
            && blake3::hash(markdown.as_bytes()).to_hex().as_str() != reference.revision()
        {
            return Err(CurrentReadError::Conflict);
        }
        Ok(Self {
            reference,
            markdown,
            observation,
        })
    }
}

/// Injection boundary only; this crate implements no native owner adapter.
///
/// A later adapter must borrow the already selected concrete ReaderStore/profile
/// authority, enforce enabled/current source generation and exact revision on
/// read, then re-read/revalidate that authority and revision in `revalidate_exact`.
/// Neither method may discover/open a profile or infer a grant from the archive.
pub trait CurrentDocumentReader {
    fn owner_scope(&self) -> Uuid;
    fn read_exact(
        &self,
        _: &CurrentDocumentReference,
    ) -> Result<CurrentDocument, CurrentReadError> {
        Err(CurrentReadError::Unsupported)
    }
    fn revalidate_exact(&self, _: &CurrentDocumentReference) -> Result<(), CurrentReadError> {
        Err(CurrentReadError::Unsupported)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrentHashComparison {
    Matches,
    Differs,
    Unknown,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonScope {
    SelectedOwnerRevision,
}

/// A transient observation. Revalidate the current reference again before any
/// later evidence use. Normalized text equality does not re-run an extractor,
/// validate a relationship, migrate an approval or establish graph membership.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingVerification {
    /// Archival fields and source-binding-required label remain unchanged.
    pub archival_review: ArchivedReview,
    pub selected_current: CurrentDocumentReference,
    pub observation: CurrentObservation,
    pub scope: ComparisonScope,
    pub current_span_hash: String,
    pub recorded_current: CurrentHashComparison,
    pub reviewed_current: CurrentHashComparison,
}

pub(super) fn hex_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
