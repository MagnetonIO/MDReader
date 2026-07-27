// Prevents an extra console window on Windows in release builds. DO NOT REMOVE.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Retained in every shipping executable so post-packaging verification can
// bind real ELF/PE/Mach-O bytes to the reviewed source and product. NUL
// delimiters avoid accidental matches in ordinary UI strings.
#[used]
static MDREADER_RELEASE_IDENTITY: &str = concat!(
    "MDREADER_RELEASE_IDENTITY_V1\0",
    "productName=MDReader\0",
    "bundleIdentifier=dev.mdreader.app\0",
    "sourceCommit=",
    env!("MDREADER_RELEASE_SOURCE_COMMIT"),
    "\0",
);

fn main() {
    std::hint::black_box(MDREADER_RELEASE_IDENTITY);
    mdreader_app_lib::run()
}
