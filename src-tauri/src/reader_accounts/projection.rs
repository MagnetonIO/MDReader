use super::model::*;
use mdreader_service::accounts as service;

fn id(value: String) -> Result<uuid::Uuid, AccountError> {
    let id = uuid::Uuid::parse_str(&value).map_err(|_| error("invalid"))?;
    if id.get_version_num() != 4 || id.to_string() != value {
        return Err(error("invalid"));
    }
    Ok(id)
}
impl AccountRequest {
    pub(super) fn service(self) -> Result<service::rpc::AccountRequest, AccountError> {
        Ok(match self {
            Self::SyncSelectedDelete { selection } => {
                service::rpc::AccountRequest::SyncSelectedDelete {
                    selection: selection.service()?,
                }
            }
            Self::InspectSelectedDelete { selection } => {
                service::rpc::AccountRequest::InspectSelectedDelete {
                    selection: selection.service()?,
                }
            }
            Self::Status {} => service::rpc::AccountRequest::Status {},
            Self::SyncSelectedState { selection } => {
                service::rpc::AccountRequest::SyncSelectedState {
                    selection: selection.service()?,
                }
            }
            Self::InspectSelectedState { selection } => {
                service::rpc::AccountRequest::InspectSelectedState {
                    selection: selection.service()?,
                }
            }
            Self::SyncSelected { selection } => service::rpc::AccountRequest::SyncSelected {
                selection: selection.service()?,
            },
            Self::InspectSelected { selection } => service::rpc::AccountRequest::InspectSelected {
                selection: selection.service()?,
            },
            Self::Begin { intent } => service::rpc::AccountRequest::Begin {
                intent: intent.service()?,
            },
            Self::Poll { job_id } => service::rpc::AccountRequest::Poll {
                job_id: id(job_id)?,
            },
            Self::Cancel { job_id } => service::rpc::AccountRequest::Cancel {
                job_id: id(job_id)?,
            },
            Self::Disconnect {} => service::rpc::AccountRequest::Disconnect {},
        })
    }
}
impl AccountAction {
    fn service(self) -> Result<service::AccountIntent, AccountError> {
        use service::AccountIntent as I;
        let packet = |value: String| {
            if value.is_empty() || value.len() > 8192 {
                Err(error("limit"))
            } else {
                Ok(value)
            }
        };
        Ok(match self {
            Self::SignIn {} => I::SignIn {},
            Self::Bootstrap { remote_library } => I::Bootstrap {
                remote_library: id(remote_library)?,
            },
            Self::JoinDevice { remote_library } => I::JoinDevice {
                remote_library: id(remote_library)?,
            },
            Self::Recover { remote_library } => I::Recover {
                remote_library: id(remote_library)?,
            },
            Self::ReconcileEnrollment {} => I::ReconcileEnrollment {},
            Self::RenewEnrollment {} => I::RenewEnrollment {},
            Self::ShowPairRequest {} => I::ShowPairRequest {},
            Self::AcceptDeviceApproval { packet: value } => I::AcceptDeviceApproval {
                packet: packet(value)?,
            },
            Self::ApproveDevice { packet: value } => I::ApproveDevice {
                packet: packet(value)?,
            },
            Self::RetryDeviceApproval {} => I::RetryDeviceApproval {},
            Self::DeviceApprovalHandedOff {} => I::DeviceApprovalHandedOff {},
            Self::ExportRecovery {} => I::ExportRecovery {},
            Self::RetryCustodyWrite {} => I::RetryCustodyWrite {},
            Self::SyncNow {} => I::SyncNow {},
        })
    }
}
fn label<T: serde::Serialize>(value: T) -> String {
    // Service public enum labels only; no credential/native transport values.
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
}
impl From<service::AccountStatus> for AccountStatus {
    fn from(value: service::AccountStatus) -> Self {
        Self {
            configured: value.configured,
            retained_library: value.retained_library,
            recovery_available: value.recovery_available,
            connected: value.connected,
            custody: value.custody.map(|c| CustodyStatus {
                state: label(c.state),
                enrollment_uncertain: c.enrollment_uncertain,
                enrollment_expired: c.enrollment_expired,
                recovery_export_available: c.recovery_export_available,
                material_staged: c.material_staged,
                device_approval: label(c.device_approval),
            }),
            latest_job: value.latest_job.map(Into::into),
            selected_scope: value.selected_scope.map(Into::into),
        }
    }
}
impl From<service::Job> for AccountJob {
    fn from(value: service::Job) -> Self {
        Self {
            id: value.id.to_string(),
            selected_sync: value.selected_sync.map(Into::into),
            selected_purpose: value.selected_purpose.map(Into::into),
            state: match value.state {
                service::JobState::Running { phase } => JobState::Running {
                    phase: label(phase),
                },
                service::JobState::Failed { error } => JobState::Failed {
                    error: error.into(),
                },
                service::JobState::Cancelled => JobState::Cancelled,
                service::JobState::Completed { outcome } => JobState::Completed {
                    outcome: outcome.into(),
                },
            },
        }
    }
}
impl From<service::Outcome> for Outcome {
    fn from(value: service::Outcome) -> Self {
        match value {
            service::Outcome::SelectedStatus { selection, purpose, disposition } => Self::SelectedStatus {
                selection: selection.into(),
                purpose:purpose.into(),
                disposition: match disposition {
                    mdreader_service::sync::outgoing::SelectedDisposition::Absent => SelectedDisposition::Absent,
                    mdreader_service::sync::outgoing::SelectedDisposition::Queued => SelectedDisposition::Queued,
                    mdreader_service::sync::outgoing::SelectedDisposition::Applied { sequence } => SelectedDisposition::Applied { sequence: sequence.to_string() },
                    mdreader_service::sync::outgoing::SelectedDisposition::ReauthorizationRequired => SelectedDisposition::ReauthorizationRequired,
                },
            },
            service::Outcome::Connected => Self::Connected,
            service::Outcome::PublicPacket {
                packet_kind,
                packet,
            } => Self::PublicPacket {
                packet_kind: label(packet_kind),
                packet,
            },
            service::Outcome::RecoveryExported => Self::RecoveryExported,
            service::Outcome::CustodyStored => Self::CustodyStored,
            service::Outcome::HandoffComplete => Self::HandoffComplete,
            service::Outcome::Sync {
                committed,
                conflicts,
                received,
                applied,
                acknowledged,
                remote_pending,
                local_pending,
                pending,
                ..
            } => Self::Sync {
                committed: committed.to_string(),
                conflicts: conflicts.to_string(),
                received: received.to_string(),
                applied: applied.to_string(),
                acknowledged: acknowledged.to_string(),
                remote_pending,
                local_pending,
                pending: pending.map(|value| AccountPending {
                    sequence: value.sequence.to_string(),
                    reason: label(value.reason),
                }),
            },
        }
    }
}
impl From<service::rpc::AccountReply> for AccountReply {
    fn from(value: service::rpc::AccountReply) -> Self {
        match value {
            service::rpc::AccountReply::Status { status } => Self::Status {
                status: status.into(),
            },
            service::rpc::AccountReply::Begun { job_id } => Self::Begun {
                job_id: job_id.to_string(),
            },
            service::rpc::AccountReply::Job { job } => Self::Job {
                job: job.map(Into::into),
            },
            service::rpc::AccountReply::Complete => Self::Complete,
        }
    }
}

impl SelectedOperation {
    fn service(self) -> Result<service::SelectedOperation, AccountError> {
        let hash = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if !hash(&self.scope.account_digest)
            || !thumbprint(&self.scope.device_id)
            || !hash(&self.receipt.source_id)
            || !hash(&self.receipt.revision)
        {
            return Err(error("invalid"));
        }
        Ok(service::SelectedOperation {
            scope: service::SelectedScope {
                account_digest: self.scope.account_digest,
                library_id: id(self.scope.library_id)?,
                device_id: self.scope.device_id,
            },
            receipt: mdreader_core::device::catalog::Receipt {
                document_id: id(self.receipt.document_id)?.to_string(),
                source_id: self.receipt.source_id,
                revision: self.receipt.revision,
            },
            operation_id: id(self.operation_id)?,
        })
    }
}
impl From<service::SelectedScope> for SelectedScope {
    fn from(value: service::SelectedScope) -> Self {
        Self {
            account_digest: value.account_digest,
            library_id: value.library_id.to_string(),
            device_id: value.device_id,
        }
    }
}
impl From<service::SelectedOperation> for SelectedOperation {
    fn from(value: service::SelectedOperation) -> Self {
        Self {
            scope: value.scope.into(),
            receipt: SelectedReceipt {
                document_id: value.receipt.document_id,
                source_id: value.receipt.source_id,
                revision: value.receipt.revision,
            },
            operation_id: value.operation_id.to_string(),
        }
    }
}

fn thumbprint(value: &str) -> bool {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    value.len() == 43
        && URL_SAFE_NO_PAD
            .decode(value)
            .is_ok_and(|bytes| bytes.len() == 32 && URL_SAFE_NO_PAD.encode(bytes) == value)
}
