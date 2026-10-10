//! One Windows Common Controls manifest for the app and its real test binaries.
//!
//! Tauri's locked default embeds this manifest through binary-only resources.
//! rfd's TaskDialogIndirect import also exists in library test executables, which
//! need the same Common Controls v6 activation context before the harness starts.

pub fn embed() {
    println!("cargo:rerun-if-changed=build_support/windows-common-controls.manifest");
    let output = std::path::PathBuf::from(
        std::env::var_os("OUT_DIR").expect("Cargo resource output directory is required"),
    );
    // Use distinct resource names so Tauri's ordinary resource.lib continues to
    // carry the icon/version only. Resource compiler includes OUT_DIR by default.
    std::fs::write(
        output.join("mdreader-common-controls.manifest"),
        include_bytes!("windows-common-controls.manifest"),
    )
    .expect("failed to write shared Windows manifest");
    let resource = output.join("mdreader-common-controls.rc");
    std::fs::write(&resource, "1 24 \"mdreader-common-controls.manifest\"\n")
        .expect("failed to write shared Windows manifest resource");
    // Unlike compile_for_tests, this documented API includes library #[test]
    // binaries as well as the shipping executable. No production feature changes.
    embed_resource::compile_for_everything(resource, embed_resource::NONE)
        .manifest_required()
        .expect("failed to compile shared Windows manifest resource");
}
