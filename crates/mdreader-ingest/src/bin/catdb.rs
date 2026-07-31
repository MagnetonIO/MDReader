//! The `catdb` binary: the application-side composition root.
//!
//! The platform boundary (catdb/* never depends on mdreader-*) means the
//! platform's CLI ships as a library and the app supplies the entry point:
//! the MDReader domain — schemas, mappings, and the markdown ingestor —
//! registers itself into the engine right here, and everything else is
//! `catdb-cli`.

use std::process::ExitCode;

fn main() -> ExitCode {
    catdb_sdk::cli::main_with_domains(vec![mdreader_ingest::registration()])
}
