//! The Markdown domain package: CatDB schemas for the FS -> MD -> KN -> RX pipeline.

#![deny(missing_docs)]

/// Returns this crate's package name.
pub fn crate_name() -> &'static str {
    "mdreader-domain"
}

/// Returns the categorical kernel crate this domain package is defined against.
pub fn kernel_crate() -> &'static str {
    catdb_core::crate_name()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_on_catdb_core() {
        assert_eq!(crate_name(), env!("CARGO_PKG_NAME"));
        assert_eq!(kernel_crate(), "catdb-core");
    }
}
