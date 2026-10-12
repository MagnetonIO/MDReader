//! Recheck agent generation inside every owner callback around provider I/O.
use super::{Owner, Result};
use crate::agents::WriteFence;
use std::sync::Arc;
pub(super) struct ScopedOwner<O: Owner> {
    pub owner: Arc<O>,
    pub fence: Option<WriteFence>,
}
impl<O: Owner> Owner for ScopedOwner<O> {
    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut rusqlite::Connection) -> Result<T>,
    ) -> Result<T> {
        self.owner.with_connection(|db| {
            if let Some(fence) = &self.fence {
                fence.check(db)?;
            }
            let value = operation(db)?;
            if let Some(fence) = &self.fence {
                fence.check(db)?;
            }
            Ok(value)
        })
    }
}
