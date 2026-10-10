//! Borrow the actual Windows JSON owner only after both renderer flushes.
//! This does not reopen the canonical database or confer a writer capability.
use super::{bridge::profile_error, files, JsonOwner, PersistenceError};
use mdreader_profile::{Error, Result};
use rusqlite::Connection;
use uuid::Uuid;

impl JsonOwner {
    pub(super) fn with_frozen_database(
        &self,
        operation: Uuid,
        receipts: &[super::WriteReceipt; 2],
        action: &mut dyn FnMut(&Connection) -> Result<()>,
    ) -> Result<()> {
        self.revalidate_flush(operation, receipts)
            .map_err(profile_error)?;
        // Keep exact issued state retained throughout the connection borrow.
        // Flush/release and renderer lifecycle replacement cannot interleave.
        let state = self.state.lock().map_err(|_| Error::NotQuiescent)?;
        super::check(&state).map_err(profile_error)?;
        let frozen = state.frozen.as_ref().ok_or(Error::NotQuiescent)?;
        if frozen.operation != operation || frozen.published != [true; 2] {
            return Err(Error::NotQuiescent);
        }
        for (index, receipt) in receipts.iter().enumerate() {
            let current = &state.receipts[index].as_ref().ok_or(Error::Conflict)?.1;
            if current.generation != receipt.generation
                || current.sha256 != receipt.sha256
                || current.owner_epoch != receipt.owner_epoch
                || current.renderer_epoch != receipt.renderer_epoch
            {
                return Err(Error::Conflict);
            }
        }
        self.verify_owner().map_err(profile_error)?;
        let result = files::with_database(&self.root, &self.lifecycle, action);
        self.verify_owner().map_err(profile_error)?;
        drop(state);
        self.revalidate_flush(operation, receipts)
            .map_err(profile_error)?;
        result
    }
}

pub(super) fn storage_error(error: PersistenceError) -> Error {
    profile_error(error)
}
