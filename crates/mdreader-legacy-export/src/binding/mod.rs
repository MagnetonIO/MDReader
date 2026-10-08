//! Read-only comparison with an explicitly selected owner/document revision.
//!
//! No native reader adapter, source discovery, grant, durable binding, review
//! transition or graph membership is supplied by this portable boundary.
mod types;
pub use types::*;
#[cfg(unix)]
mod verification;

/// Compare the authoritative archived row's cited text with one selected exact
/// revision. The owner must revalidate access and revision before return.
pub fn verify_binding(
    archive: &crate::reviews::ReviewArchive,
    selection: &SelectedBinding,
    reader: &impl CurrentDocumentReader,
) -> Result<BindingVerification, BindingError> {
    #[cfg(unix)]
    {
        verification::verify(archive, selection, reader)
    }
    #[cfg(not(unix))]
    {
        let _ = (archive, selection, reader);
        Err(BindingError::UnsupportedHost)
    }
}

#[cfg(all(test, unix))]
mod tests;
