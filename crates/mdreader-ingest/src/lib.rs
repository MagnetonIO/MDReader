//! Scanner, watcher, parser, and chunker that emit CatOperations.

#![deny(missing_docs)]

/// Returns this crate's package name.
pub fn crate_name() -> &'static str {
    "mdreader-ingest"
}

/// Returns the IR crate whose CatOperations this crate emits.
pub fn ir_crate() -> &'static str {
    catdb_ir::crate_name()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_into_catdb_ir() {
        assert_eq!(crate_name(), env!("CARGO_PKG_NAME"));
        assert_eq!(ir_crate(), "catdb-ir");
    }
}
