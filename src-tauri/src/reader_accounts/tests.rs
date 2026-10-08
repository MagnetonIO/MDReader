use super::*;
use mdreader_service::accounts as service;

#[test]
fn account_projection_closes_input_and_preserves_exact_counter_evidence() {
    let unknown = r#"{"operation":"begin","intent":{"action":"sign_in","token":"secret"}}"#;
    assert!(serde_json::from_str::<AccountRequest>(unknown).is_err());
    assert!(
        AccountRequest::Poll {
            job_id: uuid::Uuid::nil().to_string()
        }
        .service()
        .is_err()
    );
    assert!(
        AccountRequest::Begin {
            intent: AccountAction::AcceptDeviceApproval {
                packet: "p".repeat(8193)
            }
        }
        .service()
        .is_err()
    );
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
}
