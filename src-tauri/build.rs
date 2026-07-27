fn main() {
    let unset_release_commit = "0000000000000000000000000000000000000000";
    let release_source_commit = std::env::var("MDREADER_RELEASE_SOURCE_COMMIT")
        .unwrap_or_else(|_| unset_release_commit.to_owned());
    assert!(
        release_source_commit.len() == 40
            && release_source_commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "MDREADER_RELEASE_SOURCE_COMMIT must be a lowercase 40-character Git SHA"
    );
    println!("cargo:rerun-if-env-changed=MDREADER_RELEASE_SOURCE_COMMIT");
    println!("cargo:rustc-env=MDREADER_RELEASE_SOURCE_COMMIT={release_source_commit}");

    let source_commit = if release_source_commit != unset_release_commit {
        release_source_commit
    } else {
        std::env::var("MDREADER_SOURCE_COMMIT")
            .ok()
            .filter(|value| {
                value.len() == 40
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
            .or_else(|| {
                std::process::Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .and_then(|output| String::from_utf8(output.stdout).ok())
                    .map(|value| value.trim().to_string())
                    .filter(|value| {
                        value.len() == 40
                            && value
                                .bytes()
                                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                    })
            })
            .unwrap_or_else(|| "unbound-source-commit".to_string())
    };
    println!("cargo:rustc-env=MDREADER_SOURCE_COMMIT={source_commit}");
    println!("cargo:rerun-if-env-changed=MDREADER_SOURCE_COMMIT");
    tauri_build::build()
}
