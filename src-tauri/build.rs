fn main() {
    let source_commit = std::env::var("MDREADER_RELEASE_SOURCE_COMMIT")
        .unwrap_or_else(|_| "0000000000000000000000000000000000000000".to_owned());
    assert!(
        source_commit.len() == 40
            && source_commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "MDREADER_RELEASE_SOURCE_COMMIT must be a lowercase 40-character Git SHA"
    );
    println!("cargo:rerun-if-env-changed=MDREADER_RELEASE_SOURCE_COMMIT");
    println!("cargo:rustc-env=MDREADER_RELEASE_SOURCE_COMMIT={source_commit}");
    tauri_build::build()
}
