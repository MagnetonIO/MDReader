use crate::{model::valid_hash, Error, Result, RootBinding, SnapshotIdentity};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_GENERATION: u64 = 9_007_199_254_740_991;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateReceipt {
    pub id: Uuid,
    pub operation: Uuid,
    pub original: RootBinding,
    pub candidate: RootBinding,
    pub snapshot: SnapshotIdentity,
    pub imported_records: u64,
    pub import_digest: String,
}
impl CandidateReceipt {
    pub(crate) fn validate(&self) -> Result<()> {
        self.original.validate()?;
        self.candidate.validate()?;
        if self.original == self.candidate
            || self.original.locator == self.candidate.locator
            || self.imported_records > 1_000_000
            || !valid_hash(&self.import_digest)
            || !valid_hash(&self.snapshot.manifest_sha256)
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    pub root: RootBinding,
    pub mode: SelectionMode,
    pub generation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Prepared,
    Preserved,
    Staged,
    Active,
    RolledBack,
}
impl State {
    pub(crate) fn sql(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Preserved => "preserved",
            Self::Staged => "staged",
            Self::Active => "active",
            Self::RolledBack => "rolled_back",
        }
    }
    pub(crate) fn from_sql(s: &str) -> Result<Self> {
        match s {
            "prepared" => Ok(Self::Prepared),
            "preserved" => Ok(Self::Preserved),
            "staged" => Ok(Self::Staged),
            "active" => Ok(Self::Active),
            "rolled_back" => Ok(Self::RolledBack),
            _ => Err(Error::Invalid),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Transition {
    pub id: Uuid,
    pub original: RootBinding,
    pub expected_generation: u64,
    pub inventory_hash: String,
    pub state: State,
    pub snapshot: Option<SnapshotIdentity>,
    pub candidate: Option<CandidateReceipt>,
    pub activation_generation: Option<u64>,
    pub rollback_generation: Option<u64>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivationReceipt {
    pub id: Uuid,
    pub operation: Uuid,
    pub kind: ReceiptKind,
    pub from_generation: u64,
    pub to_generation: u64,
    pub selected_root: RootBinding,
    pub selected_mode: SelectionMode,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptKind {
    Activation,
    Rollback,
}
impl ReceiptKind {
    pub(crate) fn sql(self) -> &'static str {
        match self {
            Self::Activation => "activation",
            Self::Rollback => "rollback",
        }
    }
}
/// A real native owner freezes both original/candidate and all registered writers.
/// It must invoke no callback after a failed/partial freeze, and keep new effects
/// denied until the callback exits. No renderer constructs this implementation.
pub trait ActivationOwner {
    fn with_frozen<T>(
        &mut self,
        transition: &Transition,
        action: impl FnOnce(&mut dyn FrozenActivation) -> Result<T>,
    ) -> Result<T>;
}
pub trait FrozenActivation {
    /// Short final authority/owner/session/root check; no network, hashing or waits.
    fn revalidate(&mut self) -> Result<()>;
    /// Verify exact receipt on candidate owner's committed connection; it must
    /// be in autocommit and match immutable imported records, schema and roots.
    /// Owner-open DB identity must still match; a pathname from SQLite alone
    /// does not detect an old connection whose file was subsequently replaced.
    fn verify_candidate(&mut self, receipt: &CandidateReceipt) -> Result<()>;
    /// Rollback requires a newly verified export/reconciliation receipt covering
    /// any writes since activation; accepted/unknown remote writes are retained.
    /// This is owner-derived evidence, never a caller's "no changes" boolean.
    fn verify_rollback_preservation(&mut self, activation: &ActivationReceipt) -> Result<()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    LegacyCurrent,
    ReaderOnly,
}
impl SelectionMode {
    pub(crate) fn sql(self) -> &'static str {
        match self {
            Self::LegacyCurrent => "legacy_current",
            Self::ReaderOnly => "reader_only",
        }
    }
    pub(crate) fn from_sql(value: &str) -> Result<Self> {
        match value {
            "legacy_current" => Ok(Self::LegacyCurrent),
            "reader_only" => Ok(Self::ReaderOnly),
            _ => Err(Error::Invalid),
        }
    }
}
