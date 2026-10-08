use super::*;
use std::time::Duration;
impl Actor {
    pub async fn recover(&mut self, command: &Command, library: Uuid) -> Result<Outcome> {
        let adapter = self
            .setup
            .recovery
            .clone()
            .ok_or(AccountError::Unsupported)?;
        let login = self.login(command).await?;
        let binding = self
            .ensure_binding(command, library, &login, Some(CandidateKind::Recover))
            .await?;
        let summary = self.refresh_custody(command).await?;
        let enrollment = match summary.lifecycle {
            CustodyLifecycle::RecoveryCandidate {
                material_staged: false,
            } => {
                self.shared.phase(command, Phase::ReadingRecovery)?;
                let input = tokio::time::timeout(Duration::from_secs(300), adapter.import())
                    .await
                    .map_err(|_| AccountError::Interrupted)??;
                if input.file.len() > 33792 || input.secret.len() != 43 {
                    return Err(AccountError::Limit);
                }
                self.shared.check(command)?;
                let pending = self
                    .setup
                    .factory
                    .prepare_recovery(
                        &login,
                        &binding.reference,
                        &library.to_string(),
                        &input.file,
                        input.secret,
                    )
                    .await?;
                self.setup
                    .factory
                    .prepare_recovery_enrollment(&login, &pending, true)
                    .await?
            }
            CustodyLifecycle::RecoveryCandidate {
                material_staged: true,
            } => {
                enrollment::require_prepared(summary)?;
                self.setup
                    .factory
                    .recovery_enrollment_login(&login, &binding.reference, &library.to_string())
                    .await?
            }
            _ => return Err(AccountError::Conflict),
        };
        drop(login);
        let login = self.nonce_login(command, &enrollment).await?;
        let authority = self
            .setup
            .factory
            .recover(login, &binding.reference, &library.to_string(), true)
            .await?;
        self.activate(command, authority)
    }
    pub async fn export_recovery(&self, command: &Command) -> Result<Outcome> {
        let adapter = self
            .setup
            .recovery
            .clone()
            .ok_or(AccountError::Unsupported)?;
        self.shared.phase(command, Phase::ExportingRecovery)?;
        let active = self.active()?;
        let value = active.authority.export_recovery(true)?;
        tokio::time::timeout(Duration::from_secs(300), adapter.export(value))
            .await
            .map_err(|_| AccountError::Interrupted)??;
        self.shared.check(command)?;
        active
            .authority
            .scope()
            .map_err(|_| AccountError::ReconnectRequired)?;
        Ok(Outcome::RecoveryExported)
    }
}
