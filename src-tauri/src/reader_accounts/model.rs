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
    ListPending {
        scope: SelectedScope,
    },
    ResolvePending {
        scope: SelectedScope,
        sequence: String,
        revision_id: String,
        resolution_token: String,
        choice: ResolutionChoice,
    },
    SyncSelectedDelete {
        selection: SelectedOperation,
    },
    InspectSelectedDelete {
        selection: SelectedOperation,
    },
    SyncSelectedState {
        selection: SelectedOperation,
    },
    InspectSelectedState {
        selection: SelectedOperation,
    },
    SyncSelected {
        selection: SelectedOperation,
    },
    InspectSelected {
        selection: SelectedOperation,
    },
    Status {},
    Begin {
        intent: AccountAction,
    },
    Poll {
        job_id: String,
    },
    Cancel {
        job_id: String,
    },
    Disconnect {},
}
/// Exact opaque public identity only. No body, native path or credential reference.
#[derive(Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectedScope {
    pub account_digest: String,
    pub library_id: String,
    pub device_id: String,
}
#[derive(Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectedReceipt {
    pub document_id: String,
    pub source_id: String,
    pub revision: String,
}
#[derive(Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectedOperation {
    pub scope: SelectedScope,
    pub receipt: SelectedReceipt,
    pub operation_id: String,
}
#[derive(Serialize, Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SelectedDisposition {
    Absent,
    Queued,
    Applied { sequence: String },
    ReauthorizationRequired,
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
    pub custody_write_pending: bool,
    pub active_library_id: Option<String>,
    pub existing_library_id: Option<String>,
    pub existing_library_same_selection: bool,
    pub recovery_available: bool,
    pub connected: bool,
    pub custody: Option<CustodyStatus>,
    pub latest_job: Option<AccountJob>,
    pub selected_scope: Option<SelectedScope>,
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
    #[serde(rename = "selectedSync")]
    pub selected_sync: Option<SelectedOperation>,
    #[serde(rename = "selectedPurpose")]
    pub selected_purpose: Option<SelectedPurpose>,
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
    PendingList {
        scope: SelectedScope,
        entries: Vec<PendingEntry>,
    },
    PendingResolved {
        scope: SelectedScope,
        sequence: String,
        revision_id: String,
        choice: ResolutionChoice,
        document: SelectedReceipt,
    },
    SelectedStatus {
        selection: SelectedOperation,
        purpose: SelectedPurpose,
        disposition: SelectedDisposition,
    },
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
            service::AccountError::ExistingLibrary => "existing_library",
            service::AccountError::IdentityOrScope => "identity_or_scope",
            service::AccountError::ReconnectRequired => "reconnect_required",
            service::AccountError::Interrupted => "interrupted",
            service::AccountError::Cancelled => "cancelled",
            service::AccountError::Limit => "limit",
        };
        error(kind)
    }
}

#[derive(Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SelectedPurpose {
    Delete,
    Content,
    UserState,
}
impl From<service::SelectedPurpose> for SelectedPurpose {
    fn from(value: service::SelectedPurpose) -> Self {
        match value {
            service::SelectedPurpose::Delete => Self::Delete,
            service::SelectedPurpose::Content => Self::Content,
            service::SelectedPurpose::UserState => Self::UserState,
        }
    }
}

#[derive(Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionChoice {
    KeepBoth,
    KeepLocalState,
}
#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PendingEntry {
    pub sequence: String,
    pub revision_id: String,
    pub kind: String,
    pub title: String,
    pub reason: String,
    pub resolution_token: String,
    pub choices: Vec<ResolutionChoice>,
}
