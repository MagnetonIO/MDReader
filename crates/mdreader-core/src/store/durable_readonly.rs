//! Durable-only offline observation, independent of disposable index availability.
use super::*;
impl ReaderStore {
    /// Open the exact existing durable library without creating/migrating it or
    /// opening index.sqlite. Device catalog, exact Markdown reads and asset
    /// evidence use only this owner connection and its fresh device session.
    ///
    /// This handle has NO derived-index capability: its query-only in-memory
    /// placeholder deliberately has no index schema. Search/index operations
    /// fail instead of returning fabricated empty results. No source is granted.
    pub fn open_durable_readonly(directory: &Path) -> Result<Self> {
        let (library, library_identity) = identity::open_library(directory, false, |path| {
            open_readonly_database(path, LIBRARY_VERSION)
        })?;
        library.pragma_update(None, "query_only", true)?;
        projection::check_schema(&library)?;
        crate::provider_reading_state::schema::check(&library)?;
        let index = Connection::open_in_memory()?;
        index.pragma_update(None, "query_only", true)?;
        identity::verify_connection(&library, library_identity.as_ref())?;
        Ok(Self {
            library,
            library_identity,
            index_identity: None,
            index,
            #[cfg(feature = "device-assets")]
            owner_session: Arc::new(()),
        })
    }
}
