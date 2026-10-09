//! Bounded account commands for the existing private service channel.
//! Public packets are explicit OOB material; native custody never crosses this API.
use super::{AccountError, AccountIntent, AccountStatus, Coordinator, Job, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AccountRequest {
    ListPending {
        scope: super::SelectedScope,
    },
    ResolvePending {
        scope: super::SelectedScope,
        request: crate::sync::incoming::ResolutionRequest,
    },
    SyncSelectedDelete {
        selection: super::SelectedOperation,
    },
    InspectSelectedDelete {
        selection: super::SelectedOperation,
    },
    Status {},
    Begin {
        intent: AccountIntent,
    },
    Poll {
        job_id: Uuid,
    },
    Cancel {
        job_id: Uuid,
    },
    Disconnect {},
    SyncSelectedState {
        selection: super::SelectedOperation,
    },
    InspectSelectedState {
        selection: super::SelectedOperation,
    },
    SyncSelected {
        selection: super::SelectedOperation,
    },
    InspectSelected {
        selection: super::SelectedOperation,
    },
}
// Request's diagnostic formatter must never print public packet contents either.
impl std::fmt::Debug for AccountRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ListPending { .. } => "AccountListPending",
            Self::ResolvePending { .. } => "AccountResolvePending",
            Self::SyncSelectedDelete { .. } => "AccountSyncSelectedDelete",
            Self::InspectSelectedDelete { .. } => "AccountInspectSelectedDelete",
            Self::Status {} => "AccountStatus",
            Self::Begin { .. } => "AccountBegin",
            Self::Poll { .. } => "AccountPoll",
            Self::Cancel { .. } => "AccountCancel",
            Self::Disconnect {} => "AccountDisconnect",
            Self::SyncSelectedState { .. } => "AccountSyncSelectedState",
            Self::InspectSelectedState { .. } => "AccountInspectSelectedState",
            Self::SyncSelected { .. } => "AccountSyncSelected",
            Self::InspectSelected { .. } => "AccountInspectSelected",
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum AccountReply {
    Status { status: AccountStatus },
    Begun { job_id: Uuid },
    Job { job: Option<Job> },
    Complete,
}
impl Coordinator {
    pub fn execute(&self, request: AccountRequest) -> Result<AccountReply> {
        match request {
            AccountRequest::ListPending { scope } => self
                .list_pending(scope)
                .map(|job_id| AccountReply::Begun { job_id }),
            AccountRequest::ResolvePending { scope, request } => self
                .resolve_pending(scope, request)
                .map(|job_id| AccountReply::Begun { job_id }),
            AccountRequest::SyncSelectedDelete { selection } => self
                .begin_selected_delete(selection)
                .map(|job_id| AccountReply::Begun { job_id }),
            AccountRequest::InspectSelectedDelete { selection } => self
                .inspect_selected_delete(selection)
                .map(|job_id| AccountReply::Begun { job_id }),
            AccountRequest::SyncSelectedState { selection } => self
                .begin_selected_state(selection)
                .map(|job_id| AccountReply::Begun { job_id }),
            AccountRequest::InspectSelectedState { selection } => self
                .inspect_selected_state(selection)
                .map(|job_id| AccountReply::Begun { job_id }),
            AccountRequest::SyncSelected { selection } => self
                .begin_selected_sync(selection)
                .map(|job_id| AccountReply::Begun { job_id }),
            AccountRequest::InspectSelected { selection } => self
                .inspect_selected_sync(selection)
                .map(|job_id| AccountReply::Begun { job_id }),
            AccountRequest::Status {} => {
                self.status().map(|status| AccountReply::Status { status })
            }
            AccountRequest::Begin { intent } => self
                .begin(intent)
                .map(|job_id| AccountReply::Begun { job_id }),
            AccountRequest::Poll { job_id } => {
                valid_job(job_id)?;
                self.operation(job_id).map(|job| AccountReply::Job { job })
            }
            AccountRequest::Cancel { job_id } => {
                valid_job(job_id)?;
                self.cancel(job_id)?;
                Ok(AccountReply::Complete)
            }
            AccountRequest::Disconnect {} => {
                self.disconnect()?;
                Ok(AccountReply::Complete)
            }
        }
    }
}
fn valid_job(id: Uuid) -> Result<()> {
    if id.get_version_num() == 4 {
        Ok(())
    } else {
        Err(AccountError::Invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::CacheRoots;
    use mdreader_core::ReaderStore;
    use std::sync::{Arc, Mutex};

    #[test]
    fn unconfigured_rpc_is_metadata_only_closed_and_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let owner = Arc::new(Mutex::new(ReaderStore::open(directory.path()).unwrap()));
        let coordinator = Coordinator::new(
            owner,
            None,
            CacheRoots {
                documents: directory.path().join("documents"),
                assets: directory.path().join("assets"),
            },
        )
        .unwrap();
        let AccountReply::Status { status } =
            coordinator.execute(AccountRequest::Status {}).unwrap()
        else {
            panic!("status")
        };
        assert!(!status.configured);
        assert!(!status.connected);
        assert_eq!(
            coordinator
                .execute(AccountRequest::Begin {
                    intent: AccountIntent::SignIn {}
                })
                .err(),
            Some(AccountError::Unconfigured)
        );
        assert!(matches!(
            coordinator
                .execute(AccountRequest::Poll {
                    job_id: Uuid::new_v4()
                })
                .unwrap(),
            AccountReply::Job { job: None }
        ));
        assert_eq!(
            coordinator
                .execute(AccountRequest::Poll {
                    job_id: Uuid::nil()
                })
                .err(),
            Some(AccountError::Invalid)
        );
        assert!(serde_json::from_str::<AccountRequest>(
            r#"{"operation":"status","token":"secret"}"#
        )
        .is_err());
        let oversized = AccountRequest::Begin {
            intent: AccountIntent::ApproveDevice {
                packet: "x".repeat(8193),
            },
        };
        assert_eq!(
            coordinator.execute(oversized.clone()).err(),
            Some(AccountError::Limit)
        );
        assert_eq!(format!("{oversized:?}"), "AccountBegin");
        let selection = super::super::SelectedOperation {
            scope: super::super::SelectedScope {
                account_digest: "a".repeat(64),
                library_id: Uuid::new_v4(),
                device_id: "A".repeat(43),
            },
            receipt: mdreader_core::device::catalog::Receipt {
                document_id: Uuid::new_v4().to_string(),
                source_id: "b".repeat(64),
                revision: "c".repeat(64),
            },
            operation_id: Uuid::new_v4(),
        };
        let selected = AccountRequest::SyncSelected {
            selection: selection.clone(),
        };
        assert_eq!(format!("{selected:?}"), "AccountSyncSelected");
        assert_eq!(
            coordinator.execute(selected).err(),
            Some(AccountError::Unconfigured)
        );
        let mut malformed =
            serde_json::to_value(AccountRequest::InspectSelected { selection }).unwrap();
        malformed["selection"]["receipt"]["path"] =
            serde_json::json!("/private/not-a-source-grant");
        assert!(serde_json::from_value::<AccountRequest>(malformed).is_err());

        assert!(!directory.path().join("documents").exists());
        assert!(!directory.path().join("assets").exists());
    }
}
