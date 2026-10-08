#![cfg(unix)]
use mdreader_legacy_export::{Input, Role, SnapshotSpec};
use std::{fs, os::unix::fs::PermissionsExt, process::Command};
#[test]
fn tool_exports_only_explicit_offline_selection_and_reports_bounded_receipt() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let snapshot = root.path().join("settings with spaces.json");
    fs::write(
        &snapshot,
        b"{\"settings\":{\"theme\":\"dark\",\"privateMarker\":\"synthetic-private-value\"}}",
    )
    .unwrap();
    let spec = root.path().join("spec.json");
    fs::write(
        &spec,
        serde_json::to_vec(
            &SnapshotSpec::from_inputs(&[Input {
                role: Role::Settings,
                path: snapshot,
            }])
            .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let destination = root.path().join("archive");
    let out = Command::new(env!("CARGO_BIN_EXE_mdreader-legacy-export"))
        .arg("export")
        .arg(&spec)
        .arg(&destination)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.len() < 512);
    assert!(!String::from_utf8_lossy(&out.stdout).contains("synthetic-private-value"));
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(receipt["artifacts"], 1);
    let out = Command::new(env!("CARGO_BIN_EXE_mdreader-legacy-export"))
        .arg("verify")
        .arg(&destination)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["exportId"],
        receipt["exportId"]
    );
    fs::write(destination.join("0000.raw"), b"tampered").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mdreader-legacy-export"))
        .arg("verify")
        .arg(&destination)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
}
