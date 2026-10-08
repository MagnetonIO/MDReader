use mdreader_legacy_export::{export, read_spec, verify, Limits, Manifest};
use std::{ffi::OsStr, path::Path};
fn run() -> mdreader_legacy_export::Result<Manifest> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let limits = Limits::default();
    match args.as_slice() {
        [operation, spec, destination] if operation == OsStr::new("export") => export(
            &read_spec(Path::new(spec), &limits)?,
            Path::new(destination),
            &limits,
        ),
        [operation, archive] if operation == OsStr::new("verify") => {
            verify(Path::new(archive), &limits)
        }
        _ => Err(mdreader_legacy_export::Error::Invalid),
    }
}
fn main() {
    match run() {
        Ok(m) => println!(
            "{}",
            serde_json::json!({"format":m.format,"version":m.version,"exportId":m.export_id,"artifacts":m.artifacts.len(),"members":m.members.len()})
        ),
        Err(error) => {
            eprintln!("{error}; usage: mdreader-legacy-export export ABS_SPEC_JSON ABS_DESTINATION | verify ABS_ARCHIVE");
            std::process::exit(1)
        }
    }
}
