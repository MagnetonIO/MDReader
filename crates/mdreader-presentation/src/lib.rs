//! DTO shaping for the Tauri command layer.

#![deny(missing_docs)]

/// Returns this crate's package name.
pub fn crate_name() -> &'static str {
    "mdreader-presentation"
}

/// Returns the engine crate whose results this layer shapes into DTOs.
pub fn engine_crate() -> &'static str {
    catdb_sdk::engine::crate_name()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_catdb_engine_output() {
        assert_eq!(crate_name(), env!("CARGO_PKG_NAME"));
        assert_eq!(engine_crate(), "catdb-engine");
    }
}
