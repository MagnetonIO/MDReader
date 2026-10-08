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
            Self::Status => service::rpc::AccountRequest::Status,
            Self::Begin { intent } => service::rpc::AccountRequest::Begin {
                intent: intent.service()?,
            },
            Self::Poll { job_id } => service::rpc::AccountRequest::Poll {
                job_id: id(job_id)?,
            },
            Self::Cancel { job_id } => service::rpc::AccountRequest::Cancel {
                job_id: id(job_id)?,
            },
            Self::Disconnect => service::rpc::AccountRequest::Disconnect,
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
            Self::SignIn => I::SignIn {},
            Self::Bootstrap { remote_library } => I::Bootstrap {
                remote_library: id(remote_library)?,
            },
            Self::JoinDevice { remote_library } => I::JoinDevice {
                remote_library: id(remote_library)?,
            },
            Self::Recover { remote_library } => I::Recover {
                remote_library: id(remote_library)?,
            },
            Self::ReconcileEnrollment => I::ReconcileEnrollment {},
            Self::RenewEnrollment => I::RenewEnrollment {},
            Self::ShowPairRequest => I::ShowPairRequest {},
            Self::AcceptDeviceApproval { packet: value } => I::AcceptDeviceApproval {
                packet: packet(value)?,
            },
            Self::ApproveDevice { packet: value } => I::ApproveDevice {
                packet: packet(value)?,
            },
            Self::RetryDeviceApproval => I::RetryDeviceApproval {},
            Self::DeviceApprovalHandedOff => I::DeviceApprovalHandedOff {},
            Self::ExportRecovery => I::ExportRecovery {},
            Self::RetryCustodyWrite => I::RetryCustodyWrite {},
            Self::SyncNow => I::SyncNow {},
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
        }
    }
}
impl From<service::Job> for AccountJob {
    fn from(value: service::Job) -> Self {
        Self {
            id: value.id.to_string(),
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
                ..
            } => Self::Sync {
                committed: committed.to_string(),
                conflicts: conflicts.to_string(),
                received: received.to_string(),
                applied: applied.to_string(),
                acknowledged: acknowledged.to_string(),
                remote_pending,
                local_pending,
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
