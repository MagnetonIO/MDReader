//! Gated live-provider smoke. Merely compiling this target does not publish
//! Google Drive availability.

#[test]
#[ignore = "requires an explicitly provisioned Google Drive fixture account"]
fn live_google_drive_account_gate_is_explicit() {
    assert!(
        std::env::var_os("CATDB_GOOGLE_DRIVE_LIVE_FIXTURE").is_some(),
        "set CATDB_GOOGLE_DRIVE_LIVE_FIXTURE only in the isolated provider gate"
    );
}
