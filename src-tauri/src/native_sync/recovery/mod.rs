//! Production macOS recovery adapter. Ciphertext uses selected native files;
//! independent secrets use only owned OS dialogs and zeroizing native buffers.
mod dialog;
mod files;
mod owner;
use mdreader_service::accounts::{AccountError, NativeRecovery, RecoveryInput};
use mdreader_sync::authority::RecoveryExport;
use std::{future::Future, pin::Pin, time::Instant};
use zeroize::Zeroizing;
#[derive(Default)]
pub(super) struct Recovery {
    owner: owner::Owner,
}
impl NativeRecovery for Recovery {
    fn import<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<RecoveryInput, AccountError>> + Send + 'a>> {
        Box::pin(self.owner.run(|cancel| async move {
            let path = files::path(dialog::run(dialog::PICK_IMPORT, None, &cancel, 8193).await?)?;
            let read_cancel = cancel.clone();
            let file = tokio::task::spawn_blocking(move || files::read(&path, &read_cancel))
                .await
                .map_err(|_| AccountError::Interrupted)??;
            cancel.check()?;
            let bytes = dialog::run(dialog::ENTER_SECRET, None, &cancel, 64).await?;
            let secret = Zeroizing::new(
                std::str::from_utf8(bytes.as_slice())
                    .map_err(|_| AccountError::Invalid)?
                    .to_owned(),
            );
            if secret.len() != 43
                || !secret
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                return Err(AccountError::Invalid);
            }
            cancel.check()?;
            Ok(RecoveryInput { file, secret })
        }))
    }
    fn export<'a>(
        &'a self,
        value: RecoveryExport,
    ) -> Pin<Box<dyn Future<Output = Result<(), AccountError>> + Send + 'a>> {
        Box::pin(self.owner.run(|cancel| async move {
            let path = files::path(dialog::run(dialog::PICK_EXPORT, None, &cancel, 8193).await?)?;
            let confirmation = dialog::run(
                dialog::SHOW_SECRET,
                Some(value.export_secret()),
                &cancel,
                64,
            )
            .await?;
            if confirmation.as_slice() != b"confirmed" {
                return Err(AccountError::Cancelled);
            }
            cancel.check()?;
            let write_cancel = cancel.clone();
            tokio::task::spawn_blocking(move || {
                files::write(&path, value.file_bytes(), &write_cancel)
            })
            .await
            .map_err(|_| AccountError::Interrupted)??;
            cancel.check()
        }))
    }
    fn settle<'a>(
        &'a self,
        deadline: Instant,
    ) -> Pin<Box<dyn Future<Output = Result<(), AccountError>> + Send + 'a>> {
        Box::pin(self.owner.settle(deadline))
    }
}
