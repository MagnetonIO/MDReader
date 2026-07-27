fn main() {
    println!("cargo:rerun-if-env-changed=TAURI_CONFIG");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    if std::env::var_os("CARGO_FEATURE_E2E_BUILD").is_some() {
        let base: serde_json::Value = serde_json::from_str(include_str!("tauri.conf.json"))
            .expect("tauri.conf.json must contain valid JSON");
        let overlay = std::env::var("TAURI_CONFIG").ok().map(|value| {
            serde_json::from_str::<serde_json::Value>(&value)
                .expect("TAURI_CONFIG must contain valid JSON")
        });
        let identifier = overlay
            .as_ref()
            .and_then(|value| value.get("identifier"))
            .or_else(|| base.get("identifier"))
            .and_then(serde_json::Value::as_str);
        assert_eq!(
            identifier,
            Some("dev.mdreader.app.e2e"),
            "e2e-build requires Tauri bundle identity dev.mdreader.app.e2e"
        );
    }

    let source_commit = std::env::var("MDREADER_SOURCE_COMMIT")
        .ok()
        .filter(|value| value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .or_else(|| {
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|value| value.trim().to_string())
                .filter(|value| {
                    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
        })
        .unwrap_or_else(|| "unbound-source-commit".to_string());
    println!("cargo:rustc-env=MDREADER_SOURCE_COMMIT={source_commit}");
    println!("cargo:rerun-if-env-changed=MDREADER_SOURCE_COMMIT");
    tauri_build::build()
}
