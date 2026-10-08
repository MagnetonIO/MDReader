//! Injected native file/dialog boundary. Recovery secrets never become RPC data.
use super::*;
use mdreader_sync::authority::RecoveryExport;
use std::{future::Future, pin::Pin};
use zeroize::Zeroizing;
pub struct RecoveryInput {
    pub file: Vec<u8>,
    pub secret: Zeroizing<String>,
}
pub trait NativeRecovery: Send + Sync {
    /// Actual retained native dialog/file jobs must settle before profile backup.
    /// An adapter without this proof cannot claim all-owner preservation.
    fn settle<'a>(
        &'a self,
        _deadline: std::time::Instant,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async { Err(AccountError::Unsupported) })
    }
    /// Explicit selected file + native secure input; no arbitrary renderer path.
    /// Bound the read before allocating (33792 bytes), never log secret values.
    /// Dropping the future cancels pending native dialogs. Already-issued file
    /// work remains owned until settle; late replies cannot initiate another
    /// display/write or activate an abandoned account operation.
    fn import<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<RecoveryInput>> + Send + 'a>>;
    /// Save encrypted file and present independent secret through native UI only.
    fn export<'a>(
        &'a self,
        value: RecoveryExport,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;
}
