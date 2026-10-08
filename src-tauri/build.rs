#[path = "build_support/release_identity.rs"]
mod release_identity;

fn main() {
    println!("cargo:rerun-if-env-changed=TAURI_CONFIG");
    // Fresh narrow native provider registrations are independent of legacy
    // broad cloud-account configuration. GUI and headless service share identity.
    for field in [
        "MDREADER_GOOGLE_DRIVE_CLIENT_ID",
        "MDREADER_DROPBOX_CLIENT_ID",
        "MDREADER_DROPBOX_REDIRECT_URI",
        "MDREADER_NATIVE_DROPBOX_ACCESS_TYPE",
        "MDREADER_NATIVE_DRIVE_CLIENT_ID",
        "MDREADER_NATIVE_DROPBOX_CLIENT_ID",
        "MDREADER_NATIVE_DROPBOX_APP_FOLDER_REGISTRATION_SHA256",
        "MDREADER_NATIVE_DROPBOX_REDIRECT_URI",
        "MDREADER_NATIVE_SYNC_STAGE",
        "MDREADER_NATIVE_SYNC_ISSUER",
        "MDREADER_NATIVE_SYNC_CLIENT_ID",
        "MDREADER_NATIVE_SYNC_AUDIENCE",
        "MDREADER_NATIVE_SYNC_AUTHORIZATION_ENDPOINT",
        "MDREADER_NATIVE_SYNC_TOKEN_ENDPOINT",
        "MDREADER_NATIVE_SYNC_JWKS_ENDPOINT",
        "MDREADER_NATIVE_SYNC_ENDPOINT",
        "MDREADER_NATIVE_SYNC_REDIRECT_URI",
        "MDREADER_MOBILE_SYNC_REDIRECT_URI",
    ] {
        println!("cargo:rerun-if-env-changed={field}");
    }
    let native_base: serde_json::Value = serde_json::from_str(include_str!("tauri.conf.json"))
        .expect("tauri.conf.json must contain valid JSON");
    let native_overlay = std::env::var("TAURI_CONFIG").ok().map(|value| {
        serde_json::from_str::<serde_json::Value>(&value)
            .expect("TAURI_CONFIG must contain valid JSON")
    });
    let native_identifier = native_overlay
        .as_ref()
        .and_then(|v| v.get("identifier"))
        .or_else(|| native_base.get("identifier"))
        .and_then(serde_json::Value::as_str)
        .expect("native app identifier is required");
    assert!(
        !native_identifier.is_empty()
            && native_identifier.len() <= 128
            && native_identifier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b)),
        "native app identifier is invalid"
    );
    println!("cargo:rustc-env=MDREADER_NATIVE_APP_ID={native_identifier}");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=../ui/dist");

    // The macOS release links the reviewed ONNX Runtime, whose install name is
    // `@rpath/libonnxruntime.1.24.2.dylib`, and Tauri packages it under
    // `MDReader.app/Contents/Frameworks`. Neither the linker nor the bundler
    // records the LC_RPATH that lets dyld follow `@rpath` there, so the
    // executable would fail to launch without this load command. Target-scoped:
    // no other platform links through `@rpath`, so nothing else is affected.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,@executable_path/../Frameworks");
    }
    let e2e_build = std::env::var_os("CARGO_FEATURE_E2E_BUILD").is_some();
    if e2e_build {
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

    let unset_release_commit = release_identity::UNSET;
    let release_handoff_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".release-source-commit");
    println!("cargo:rerun-if-env-changed=MDREADER_RELEASE_SOURCE_COMMIT");
    println!("cargo:rerun-if-changed={}", release_handoff_path.display());
    let release_source_commit = release_identity::resolve(
        std::env::var_os("MDREADER_RELEASE_SOURCE_COMMIT").as_deref(),
        &release_handoff_path,
    )
    .expect("invalid release source-identity configuration");
    println!("cargo:rustc-env=MDREADER_RELEASE_SOURCE_COMMIT={release_source_commit}");

    let source_commit = if e2e_build {
        let environment_commit = std::env::var("MDREADER_SOURCE_COMMIT").ok();
        let mobile_handoff_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".e2e-source-commit");
        let mobile_handoff = match std::fs::read_to_string(&mobile_handoff_path) {
            Ok(value) => Some(value),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => panic!(
                "failed to read mobile E2E source-identity handoff {}: {error}",
                mobile_handoff_path.display()
            ),
        };
        println!("cargo:rerun-if-changed={}", mobile_handoff_path.display());
        let mobile_commit = mobile_handoff.map(|value| {
            value
                .strip_suffix('\n')
                .filter(|commit| !commit.contains('\n'))
                .expect(
                    "mobile E2E source-identity handoff must contain one newline-terminated SHA",
                )
                .to_owned()
        });
        if let (Some(environment_commit), Some(mobile_commit)) =
            (&environment_commit, &mobile_commit)
        {
            assert_eq!(
                environment_commit, mobile_commit,
                "E2E source commit identities do not match"
            );
        }
        let source_commit = environment_commit.or(mobile_commit).expect(
            "e2e-build requires MDREADER_SOURCE_COMMIT or a mobile source-identity handoff",
        );
        assert!(
            source_commit.len() == 40
                && source_commit
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "MDREADER_SOURCE_COMMIT must be a lowercase 40-character Git SHA"
        );
        if release_source_commit != unset_release_commit {
            assert_eq!(
                source_commit, release_source_commit,
                "release and E2E source commits must identify the same checkout"
            );
        }
        source_commit
    } else if release_source_commit != unset_release_commit {
        release_source_commit
    } else {
        "unbound-source-commit".to_owned()
    };
    println!("cargo:rustc-env=MDREADER_SOURCE_COMMIT={source_commit}");
    println!("cargo:rerun-if-env-changed=MDREADER_SOURCE_COMMIT");
    tauri_build::build()
}
