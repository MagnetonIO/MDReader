fn main() {
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
