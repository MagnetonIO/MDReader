//! Gated live-provider smoke. Compiling this target does not establish
//! Dropbox availability.

#[test]
#[ignore = "requires an explicitly provisioned Dropbox fixture account"]
fn live_dropbox_account_gate_is_explicit() {
    assert!(
        std::env::var_os("CATDB_DROPBOX_LIVE_FIXTURE").is_some(),
        "set CATDB_DROPBOX_LIVE_FIXTURE only in the isolated provider gate"
    );
}
