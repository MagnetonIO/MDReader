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
    /// Explicit selected file + native secure input; no arbitrary renderer path.
    /// Bound the read before allocating (33792 bytes), never log secret values.
    /// Dropping either future must cancel pending dialogs/requests: late native
    /// replies cannot save/display material or activate an abandoned operation.
    fn import<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<RecoveryInput>> + Send + 'a>>;
    /// Save encrypted file and present independent secret through native UI only.
    fn export<'a>(
        &'a self,
        value: RecoveryExport,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;
}
