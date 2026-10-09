//! Stage-bound actual-owner SQLite backup, local or through a trusted service.
mod types;
pub use types::*;
#[cfg(any(unix, windows))]
mod materialization;
#[cfg(any(unix, windows))]
mod target;
#[cfg(any(unix, windows))]
pub use materialization::RemoteBackup;
#[cfg(any(unix, windows))]
pub use target::DatabaseBackupTarget;
#[cfg(not(any(unix, windows)))]
pub struct DatabaseBackupTarget {
    request: BackupRequest,
}
#[cfg(not(any(unix, windows)))]
impl DatabaseBackupTarget {
    pub fn request(&self) -> &BackupRequest {
        &self.request
    }
    pub fn accept(&mut self, _: &BackupReceipt) -> crate::Result<()> {
        Err(crate::Error::Unsupported)
    }
}
