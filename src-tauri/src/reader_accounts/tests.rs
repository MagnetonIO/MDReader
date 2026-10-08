use super::*;
use mdreader_service::accounts as service;

#[test]
fn account_projection_closes_input_and_preserves_exact_counter_evidence() {
    let unknown = r#"{"operation":"begin","intent":{"action":"sign_in","token":"secret"}}"#;
    assert!(serde_json::from_str::<AccountRequest>(unknown).is_err());
    assert!(AccountRequest::Poll {
        job_id: uuid::Uuid::nil().to_string()
    }
    .service()
    .is_err());
    assert!(AccountRequest::Begin {
        intent: AccountAction::AcceptDeviceApproval {
            packet: "p".repeat(8193)
        }
    }
    .service()
    .is_err());
    let outcome: Outcome = service::Outcome::Sync {
        committed: 1,
        conflicts: 0,
        received: u64::MAX,
        applied: 0,
        acknowledged: 0,
        remote_pending: true,
        local_pending: false,
        pending: Some(service::SyncPending {
            sequence: u64::MAX,
            reason: mdreader_service::sync::incoming::PendingReason::HeadConflict,
        }),
    }
    .into();
    let value = serde_json::to_value(outcome).unwrap();
    assert_eq!(value["received"], u64::MAX.to_string());
    assert_eq!(value["applied"], "0");
    assert_eq!(value["remotePending"], true);
    assert_eq!(value["pending"]["sequence"], u64::MAX.to_string());
    assert_eq!(value["pending"]["reason"], "head_conflict");
    assert!(value.get("token").is_none());
    assert!(value.get("credentialReference").is_none());
    let selection = service::SelectedOperation {
        scope: service::SelectedScope {
            account_digest: "a".repeat(64),
            library_id: uuid::Uuid::new_v4(),
            device_id: "A".repeat(43),
        },
        receipt: mdreader_core::device::catalog::Receipt {
            document_id: uuid::Uuid::new_v4().to_string(),
            source_id: "b".repeat(64),
            revision: "c".repeat(64),
        },
        operation_id: uuid::Uuid::new_v4(),
    };
    let native: SelectedOperation = selection.clone().into();
    let request = AccountRequest::SyncSelected { selection: native }
        .service()
        .unwrap_or_else(|_| panic!("closed selected request rejected"));
    assert!(
        matches!(request, service::rpc::AccountRequest::SyncSelected { selection: exact } if exact == selection)
    );
    let mut value = serde_json::to_value(SelectedOperation::from(selection.clone())).unwrap();
    value["receipt"]["revision"] = serde_json::json!("f".repeat(63));
    assert!(AccountRequest::InspectSelected {
        selection: serde_json::from_value(value.clone()).unwrap()
    }
    .service()
    .is_err());
    value["receipt"]["path"] = serde_json::json!("/private/not-authority");
    assert!(serde_json::from_value::<SelectedOperation>(value).is_err());
    let projected: Outcome = service::Outcome::SelectedStatus {
        selection,
        disposition: mdreader_service::sync::outgoing::SelectedDisposition::Applied {
            sequence: u64::MAX,
        },
    }
    .into();
    assert_eq!(
        serde_json::to_value(projected).unwrap()["disposition"]["sequence"],
        u64::MAX.to_string()
    );
}
