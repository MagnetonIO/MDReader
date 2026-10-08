//! Renderer-safe account command DTOs. No native credential references or paths.
use mdreader_service::accounts as service;
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Deserialize, Type)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AccountAction {
    SignIn {},
    Bootstrap { remote_library: String },
    JoinDevice { remote_library: String },
    Recover { remote_library: String },
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
#[derive(Deserialize, Type)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AccountRequest {
    Status {},
    Begin { intent: AccountAction },
    Poll { job_id: String },
    Cancel { job_id: String },
    Disconnect {},
}
#[derive(Serialize, Type)]
pub struct AccountError {
    pub kind: String,
}
#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub configured: bool,
    pub retained_library: bool,
    pub recovery_available: bool,
    pub connected: bool,
    pub custody: Option<CustodyStatus>,
    pub latest_job: Option<AccountJob>,
}
#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CustodyStatus {
    pub state: String,
    pub enrollment_uncertain: bool,
    pub enrollment_expired: bool,
    pub recovery_export_available: bool,
    pub material_staged: bool,
    pub device_approval: String,
}
#[derive(Serialize, Type)]
pub struct AccountJob {
    pub id: String,
    pub state: JobState,
}
#[derive(Serialize, Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JobState {
    Running { phase: String },
    Completed { outcome: Outcome },
    Failed { error: AccountError },
    Cancelled,
}
#[derive(Serialize, Type)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Outcome {
    Connected,
    PublicPacket {
        packet_kind: String,
        packet: String,
    },
    RecoveryExported,
    CustodyStored,
    HandoffComplete,
    Sync {
        committed: String,
        conflicts: String,
        received: String,
        applied: String,
        acknowledged: String,
        remote_pending: bool,
        local_pending: bool,
        pending: Option<AccountPending>,
    },
}
#[derive(Serialize, Type)]
pub struct AccountPending {
    pub sequence: String,
    pub reason: String,
}
#[derive(Serialize, Type)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AccountReply {
    Status { status: AccountStatus },
    Begun { job_id: String },
    Job { job: Option<AccountJob> },
    Complete,
}

pub(super) fn error(kind: &str) -> AccountError {
    AccountError { kind: kind.into() }
}
impl From<service::AccountError> for AccountError {
    fn from(value: service::AccountError) -> Self {
        // All service errors are fixed enums; never expose transport/native text.
        let kind = match value {
            service::AccountError::Unconfigured => "unconfigured",
            service::AccountError::Unsupported => "unsupported",
            service::AccountError::Busy => "busy",
            service::AccountError::Invalid => "invalid",
            service::AccountError::Schema => "schema",
            service::AccountError::Storage => "storage",
            service::AccountError::Conflict => "conflict",
            service::AccountError::CustodyUnavailable => "custody_unavailable",
            service::AccountError::EnrollmentDenied => "enrollment_denied",
            service::AccountError::IdentityOrScope => "identity_or_scope",
            service::AccountError::ReconnectRequired => "reconnect_required",
            service::AccountError::Interrupted => "interrupted",
            service::AccountError::Cancelled => "cancelled",
            service::AccountError::Limit => "limit",
        };
        error(kind)
    }
}
