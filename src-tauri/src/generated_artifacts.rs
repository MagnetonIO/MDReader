//! Test-only artifact generation. Unchanged bytes preserve source timestamps,
//! avoiding invalidation of Rust tests that embed the generated contracts.
use std::{fs, io, path::Path};
use tauri_specta::Builder;

pub(super) fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    match fs::read(path) {
        Ok(current) if current == bytes => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::write(path, bytes)
}

pub(super) fn export_bindings(
    builder: &Builder<tauri::Wry>,
) -> Result<(), specta_typescript::Error> {
    let staging = tempfile::tempdir()?;
    let staged_path = staging.path().join("bindings.ts");
    builder.export(specta_typescript::Typescript::default(), &staged_path)?;
    // Specta's blank doc-comment lines contain trailing spaces. Normalize once
    // before comparison/publication, propagating read/write errors to the gate.
    let contents = fs::read_to_string(staged_path)?;
    let body = contents
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    write_if_changed(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/src/bindings.ts"),
        format!("{}\n", body.trim_end()).as_bytes(),
    )?;
    Ok(())
}
