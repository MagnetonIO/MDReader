//! Crash ordering: SQLite opaque ref first, then exact same OS-vault material.
use super::*;
use actor::{Actor, Command};
use storage::Binding;
impl Actor {
    pub async fn ensure_binding(
        &mut self,
        command: &Command,
        library: Uuid,
        login: &VerifiedLogin,
        kind: Option<CandidateKind>,
    ) -> Result<Binding> {
        self.shared.check(command)?;
        if let Some(binding) = &self.binding {
            if binding.library != library {
                return Err(AccountError::Conflict);
            }
            if self.pending.is_some() {
                return Err(AccountError::CustodyUnavailable);
            }
            return self.bound();
        }
        let pending = match kind {
            Some(kind) => {
                self.setup
                    .factory
                    .prepare_candidate(login, &library.to_string(), kind)?
            }
            None => self
                .setup
                .factory
                .prepare_pending(login, &library.to_string())?,
        };
        // No await/network/files in this owner transaction. Retained reference is
        // sufficient to fail closed after a process crash before OS create.
        let binding = {
            let mut owner = self.owner.try_lock().map_err(|_| AccountError::Busy)?;
            storage::reserve(&mut owner, library, &pending.reference())?
        };
        self.binding = Some(binding);
        self.pending = Some(pending);
        {
            let mut state = self
                .shared
                .view
                .lock()
                .map_err(|_| AccountError::Interrupted)?;
            state.retained = true;
        }
        self.persist_exact(command).await?;
        self.bound()
    }
    pub async fn persist_exact(&mut self, command: &Command) -> Result<Outcome> {
        self.shared.check(command)?;
        let pending = self
            .pending
            .as_ref()
            .ok_or(AccountError::CustodyUnavailable)?;
        self.setup.factory.persist_pending(pending).await?;
        self.shared.check(command)?;
        // On timeout/cancellation/error above, the same record remains available
        // only in this native actor. Restart never creates a replacement record.
        self.pending = None;
        Ok(Outcome::CustodyStored)
    }
}
