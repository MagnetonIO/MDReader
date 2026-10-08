//! Notify only after registry-owned global resumption. Participant destruction
//! retains the exact request; it never emits a notification or grants a writer.
use super::*;
impl FlushBridge {
    pub(super) fn post_resume(&self, scope: &FreezeScope<'_>) -> Result<()> {
        let notice = {
            let mut phase = self.phase.lock().map_err(|_| Error::NotQuiescent)?;
            match &*phase {
                Phase::Released {
                    operation,
                    epoch,
                    window,
                    request,
                } if *operation == scope.operation()
                    && *epoch == scope.owner_epoch()
                    && &self.root == scope.root() =>
                {
                    self.owner
                        .verify_binding(scope.root())
                        .map_err(profile_error)?;
                    let notice = (window.clone(), request.clone());
                    *phase = Phase::Idle;
                    Some(notice)
                }
                Phase::Idle => None, // The other JSON participant already notified.
                _ => return Err(Error::NotQuiescent),
            }
        };
        if let Some((window, request)) = notice {
            self.channel.released(&window, &request)?;
        }
        Ok(())
    }
}
