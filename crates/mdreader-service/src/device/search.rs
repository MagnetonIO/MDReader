//! Bounded readonly search selection through the same retained device owner.
use super::*;
impl Coordinator {
    pub(crate) fn search_sources(&self) -> Result<Vec<catalog::Receipt>> {
        let _admission = self.admission.begin()?;
        let query = self.queries.try_lock().map_err(|_| DeviceError::Busy)?;
        let mut result = Vec::new();
        let mut after = None;
        let mut snapshot = None;
        loop {
            let page = catalog::page_in(
                &query,
                &self.documents,
                after.as_deref(),
                snapshot.as_ref(),
                100,
            )
            .map_err(document_error)?;
            result.extend(page.documents.iter().map(catalog::receipt));
            // Each device copy has its own source. Catalog validation already
            // enforces the 1,000-document owner limit; do not truncate a library
            // to the unrelated external-source budget.
            if result.len() > mdreader_core::MAX_SEARCH_SOURCES {
                return Err(DeviceError::Invalid);
            }
            if page.next_after.is_none() {
                break;
            }
            after = page.next_after;
            snapshot = page.snapshot;
        }
        Ok(result)
    }
}
