//! Stage-bound actual-owner SQLite backup, local or through a trusted service.
mod types;
pub use types::*;
#[cfg(unix)]
mod materialization;
#[cfg(unix)]
mod target;
#[cfg(unix)]
pub use materialization::RemoteBackup;
#[cfg(unix)]
pub use target::DatabaseBackupTarget;
#[cfg(not(unix))]
pub struct DatabaseBackupTarget {
    request: BackupRequest,
}
#[cfg(not(unix))]
impl DatabaseBackupTarget {
    pub fn request(&self) -> &BackupRequest {
        &self.request
    }
    pub fn accept(&mut self, _: &BackupReceipt) -> crate::Result<()> {
        Err(crate::Error::Unsupported)
    }
}
