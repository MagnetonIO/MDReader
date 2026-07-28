use std::process::Command;

fn valid_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn main() {
    println!("cargo:rerun-if-env-changed=CATDB_DEMO_SOURCE_COMMIT");

    let commit = std::env::var("CATDB_DEMO_SOURCE_COMMIT")
        .ok()
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--verify", "HEAD"])
                .current_dir(std::env::var_os("CARGO_MANIFEST_DIR")?)
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|value| value.trim().to_string())
        })
        .unwrap_or_else(|| {
            panic!("CATDB_DEMO_SOURCE_COMMIT is required when Git metadata is unavailable")
        });
    assert!(
        valid_commit(&commit),
        "CATDB_DEMO_SOURCE_COMMIT must be an exact lowercase 40-hex Git commit"
    );
    println!("cargo:rustc-env=CATDB_DEMO_SOURCE_COMMIT={commit}");
}
