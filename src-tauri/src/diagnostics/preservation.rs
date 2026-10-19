//! Borrow the real diagnostic writer lock while a frozen profile is inventoried.
//! Logs are excluded, but their names/identities still need a consistent snapshot.
use super::{DiagnosticTransaction, DiagnosticsState};
use mdreader_core::SourceRoot;
use mdreader_profile::{Error, Result};
use std::path::Path;

impl DiagnosticsState {
    pub(crate) fn with_preservation<T>(
        &self,
        profile: &Path,
        action: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let (_guard, _lease, authority) = self
            .log
            .acquire_transaction(DiagnosticTransaction::Write)
            .map_err(|_| Error::NotQuiescent)?;
        let expected =
            SourceRoot::capture(profile.join("diagnostics")).map_err(|_| Error::Conflict)?;
        // Path-based; `verify` below still refuses a replaced directory.
        if authority.path != expected.path {
            return Err(Error::Conflict);
        }
        let result = action();
        authority.verify().map_err(|_| Error::Conflict)?;
        expected.verify().map_err(|_| Error::Conflict)?;
        result
    }
}
