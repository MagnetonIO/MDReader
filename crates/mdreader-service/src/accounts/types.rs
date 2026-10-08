use mdreader_sync::authority::{
    AuthorityError, CustodyLifecycle, CustodySummary, PairApprovalSummary,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Explicit Settings actions, not a generic renderer `confirmed` boolean.
/// No credentials, private keys, recovery secrets, paths or callback targets.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum AccountIntent {
    SignIn {},
    Bootstrap { remote_library: Uuid },
    JoinDevice { remote_library: Uuid },
    Recover { remote_library: Uuid },
    ReconcileEnrollment {},
    RenewEnrollment {},
    ShowPairRequest {},
    AcceptDeviceApproval { packet: String },
    ApproveDevice { packet: String },
    RetryDeviceApproval {},
    DeviceApprovalHandedOff {},
    ExportRecovery {},
    RetryCustodyWrite {},
    SyncNow {},
}
impl AccountIntent {
    pub(super) fn validate(&self) -> Result<()> {
        match self {
            Self::Bootstrap { remote_library }
            | Self::JoinDevice { remote_library }
            | Self::Recover { remote_library }
                if remote_library.get_version_num() != 4 =>
            {
                Err(AccountError::Invalid)
            }
            Self::AcceptDeviceApproval { packet } | Self::ApproveDevice { packet }
                if packet.is_empty() || packet.len() > 8192 =>
            {
                Err(AccountError::Limit)
            }
            _ => Ok(()),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountError {
    Unconfigured,
    Unsupported,
    Busy,
    Invalid,
    Schema,
    Storage,
    Conflict,
    CustodyUnavailable,
    EnrollmentDenied,
    IdentityOrScope,
    ReconnectRequired,
    Interrupted,
    Cancelled,
    Limit,
}
impl From<AuthorityError> for AccountError {
    fn from(e: AuthorityError) -> Self {
        match e {
            AuthorityError::Configuration => Self::Unconfigured,
            AuthorityError::Identity | AuthorityError::Scope => Self::IdentityOrScope,
            AuthorityError::Inactive => Self::ReconnectRequired,
            AuthorityError::Busy => Self::Busy,
            AuthorityError::Interrupted => Self::Interrupted,
            AuthorityError::Bounds => Self::Limit,
            AuthorityError::Custody => Self::CustodyUnavailable,
            AuthorityError::Generation => Self::Conflict,
            AuthorityError::Enrollment => Self::EnrollmentDenied,
            AuthorityError::LifecycleGated => Self::Unsupported,
            AuthorityError::Unsupported => Self::Unsupported,
        }
    }
}
impl From<rusqlite::Error> for AccountError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Storage
    }
}
impl From<crate::sync::incoming::Error> for AccountError {
    fn from(error: crate::sync::incoming::Error) -> Self {
        use crate::sync::incoming::Error;
        match error {
            Error::Busy => Self::Busy,
            Error::Inactive => Self::ReconnectRequired,
            Error::Scope => Self::IdentityOrScope,
            Error::Conflict => Self::Conflict,
            Error::Schema => Self::Schema,
            Error::Limit => Self::Limit,
            Error::Storage | Error::Publication(_) => Self::Storage,
        }
    }
}
impl From<mdreader_sync::transport::TransportError> for AccountError {
    fn from(error: mdreader_sync::transport::TransportError) -> Self {
        use mdreader_sync::transport::TransportError;
        match error {
            TransportError::Authority
            | TransportError::Authentication
            | TransportError::Forbidden => Self::ReconnectRequired,
            TransportError::Busy => Self::Busy,
            TransportError::Conflict => Self::Conflict,
            TransportError::Quota => Self::Limit,
            TransportError::Retryable => Self::Interrupted,
            TransportError::Protocol => Self::Invalid,
            TransportError::Local(_) => Self::Storage,
        }
    }
}
pub type Result<T> = std::result::Result<T, AccountError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Queued,
    SigningIn,
    PreparingEnrollment,
    WaitingForDeviceApproval,
    ReadingRecovery,
    ExportingRecovery,
    Syncing,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicPacketKind {
    Request,
    Approval,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    SelectedStatus {
        selection: super::SelectedOperation,
        disposition: crate::sync::outgoing::SelectedDisposition,
    },
    Connected,
    PublicPacket {
        packet_kind: PublicPacketKind,
        packet: String,
    },
    RecoveryExported,
    CustodyStored,
    HandoffComplete,
    Sync {
        committed: usize,
        conflicts: usize,
        received: u64,
        applied: u64,
        acknowledged: u64,
        remote_pending: bool,
        local_pending: bool,
        pending: Option<SyncPending>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JobState {
    Running { phase: Phase },
    Completed { outcome: Outcome },
    Failed { error: AccountError },
    Cancelled,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: Uuid,
    #[serde(default)]
    pub selected_sync: Option<super::SelectedOperation>,
    pub state: JobState,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustodyView {
    pub state: CustodyPhase,
    pub enrollment_uncertain: bool,
    pub enrollment_expired: bool,
    pub recovery_export_available: bool,
    pub material_staged: bool,
    pub device_approval: ApprovalPhase,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustodyPhase {
    LegacyPending,
    LegacyEnrolled,
    BootstrapPending,
    PairCandidate,
    RecoveryCandidate,
    Enrolled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPhase {
    None,
    Prepared,
    Uncertain,
    Confirmed,
}
impl From<CustodySummary> for CustodyView {
    fn from(s: CustodySummary) -> Self {
        let (state, export, staged) = match s.lifecycle {
            CustodyLifecycle::LegacyPending => (CustodyPhase::LegacyPending, false, false),
            CustodyLifecycle::LegacyEnrolled => (CustodyPhase::LegacyEnrolled, false, false),
            CustodyLifecycle::BootstrapPending => (CustodyPhase::BootstrapPending, false, false),
            CustodyLifecycle::PairCandidate {
                trusted_approval_staged,
            } => (CustodyPhase::PairCandidate, false, trusted_approval_staged),
            CustodyLifecycle::RecoveryCandidate { material_staged } => {
                (CustodyPhase::RecoveryCandidate, false, material_staged)
            }
            CustodyLifecycle::Enrolled {
                recovery_export_available,
            } => (CustodyPhase::Enrolled, recovery_export_available, false),
        };
        Self {
            state,
            enrollment_uncertain: s.enrollment.is_some_and(|e| e.uncertain),
            enrollment_expired: s.enrollment.is_some_and(|e| e.expired),
            recovery_export_available: export,
            material_staged: staged,
            device_approval: match s.pair_approval {
                PairApprovalSummary::None => ApprovalPhase::None,
                PairApprovalSummary::Prepared => ApprovalPhase::Prepared,
                PairApprovalSummary::Uncertain => ApprovalPhase::Uncertain,
                PairApprovalSummary::Confirmed => ApprovalPhase::Confirmed,
            },
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct AccountStatus {
    pub configured: bool,
    /// Native secure recovery file/input adapter exists. Export additionally
    /// requires the confirmed local anchor indicated by custody summary.
    pub recovery_available: bool,
    pub retained_library: bool,
    pub connected: bool,
    /// None means not inspected yet, not proof that OS material is absent.
    pub custody: Option<CustodyView>,
    pub latest_job: Option<Job>,
    pub selected_scope: Option<super::SelectedScope>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPending {
    pub sequence: u64,
    pub reason: crate::sync::incoming::PendingReason,
}
