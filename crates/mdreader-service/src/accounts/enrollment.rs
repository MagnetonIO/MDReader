use super::*;
use actor::{Actor, Command};
use std::time::{Duration, Instant};

impl Actor {
    pub async fn login(&self, command: &Command) -> Result<VerifiedLogin> {
        self.shared.phase(command, Phase::SigningIn)?;
        {
            let mut state = self
                .shared
                .view
                .lock()
                .map_err(|_| AccountError::Interrupted)?;
            self.setup.factory.session_controller().logout()?;
            if let Some(active) = state.active.take() {
                active
                    .incoming
                    .clear_selection()
                    .map_err(AccountError::from)?;
            }
        }
        Ok(self
            .setup
            .factory
            .login_native(
                &self.setup.registered_redirect,
                self.setup.browser.as_ref(),
                Instant::now() + Duration::from_secs(300),
            )
            .await?)
    }
    pub async fn nonce_login(
        &self,
        command: &Command,
        enrollment: &EnrollmentLogin,
    ) -> Result<VerifiedLogin> {
        self.shared.phase(command, Phase::AuthorizingDevice)?;
        Ok(self
            .setup
            .factory
            .login_enrollment_native(
                enrollment,
                &self.setup.registered_redirect,
                self.setup.browser.as_ref(),
                Instant::now() + Duration::from_secs(300),
            )
            .await?)
    }
    pub async fn bootstrap(&mut self, command: &Command, library: Uuid) -> Result<Outcome> {
        let login = self.login(command).await?;
        let binding = self.ensure_binding(command, library, &login, None).await?;
        let summary = self.refresh_custody(command).await?;
        if summary.lifecycle != CustodyLifecycle::BootstrapPending {
            return Err(AccountError::Conflict);
        }
        // Uncertain or expired dispatch requires explicit reconcile/renew intent.
        require_prepared(summary)?;
        self.shared.phase(command, Phase::PreparingEnrollment)?;
        let enrollment = self
            .setup
            .factory
            .prepare_bootstrap(&login, &binding.reference, &library.to_string())
            .await?;
        drop(login);
        let login = self.nonce_login(command, &enrollment).await?;
        let authority = self.bootstrap_authority(command, login, &binding).await?;
        self.activate(command, authority)
    }
    async fn bootstrap_authority(
        &self,
        command: &Command,
        login: VerifiedLogin,
        binding: &storage::Binding,
    ) -> Result<NativeAuthority> {
        let result = self
            .setup
            .factory
            .bootstrap(login, &binding.reference, &binding.library.to_string())
            .await;
        if let Err(mdreader_sync::authority::AuthorityError::ExistingLibrary {
            library_id: Some(library),
        }) = &result
        {
            self.shared.check(command)?;
            {
                let mut owner = self.owner.try_lock().map_err(|_| AccountError::Busy)?;
                retained::observe(&mut owner, binding, *library)?;
            }
            let mut state = self
                .shared
                .view
                .lock()
                .map_err(|_| AccountError::Interrupted)?;
            if state.epoch != command.epoch || state.running != Some(command.id) {
                return Err(AccountError::Cancelled);
            }
            state.existing_library = Some(*library);
            state.existing_library_same_selection = *library == binding.library;
        }
        result.map_err(Into::into)
    }
    pub async fn reconcile(&mut self, command: &Command) -> Result<Outcome> {
        let binding = self.bound()?;
        let login = self.login(command).await?;
        let summary = self.refresh_custody(command).await?;
        let library = binding.library.to_string();
        let authority = match summary.lifecycle {
            CustodyLifecycle::PairCandidate {
                trusted_approval_staged: true,
            } => {
                self.setup
                    .factory
                    .reconcile_pair(login, &binding.reference, &library, true)
                    .await?
            }
            CustodyLifecycle::RecoveryCandidate {
                material_staged: true,
            } => {
                self.setup
                    .factory
                    .reconcile_recovery(login, &binding.reference, &library, true)
                    .await?
            }
            CustodyLifecycle::PairCandidate { .. } | CustodyLifecycle::RecoveryCandidate { .. } => {
                return Err(AccountError::Unsupported);
            }
            _ => {
                self.setup
                    .factory
                    .resume(login, &binding.reference, &library)
                    .await?
            }
        };
        self.activate(command, authority)
    }
    pub async fn renew(&mut self, command: &Command) -> Result<Outcome> {
        let binding = self.bound()?;
        let login = self.login(command).await?;
        let summary = self.refresh_custody(command).await?;
        let library = binding.library.to_string();
        let enrollment = if summary.lifecycle == CustodyLifecycle::BootstrapPending
            && summary
                .enrollment
                .is_none_or(|i| !i.expired && !i.uncertain)
        {
            // Cancellation before challenge creation or nonce login retains the
            // original slot and keys. Prepare a missing intent, or reopen its
            // exact unexpired bytes; never create another library/material.
            self.shared.phase(command, Phase::PreparingEnrollment)?;
            let enrollment = self
                .setup
                .factory
                .prepare_bootstrap(&login, &binding.reference, &library)
                .await?;
            drop(login);
            enrollment
        } else {
            if !summary.enrollment.is_some_and(|i| {
                i.expired
                    || (i.uncertain && summary.lifecycle == CustodyLifecycle::BootstrapPending)
            }) {
                return Err(AccountError::Conflict);
            }
            match self
                .setup
                .factory
                .renew_enrollment(login, &binding.reference, &library, true)
                .await?
            {
                EnrollmentProgress::Enrolled(authority) => {
                    return self.activate(command, *authority)
                }
                EnrollmentProgress::Login(enrollment) => enrollment,
            }
        };
        let login = self.nonce_login(command, &enrollment).await?;
        let authority = match summary.lifecycle {
            CustodyLifecycle::BootstrapPending => {
                self.bootstrap_authority(command, login, &binding).await?
            }
            CustodyLifecycle::RecoveryCandidate {
                material_staged: true,
            } => {
                self.setup
                    .factory
                    .recover(login, &binding.reference, &library, true)
                    .await?
            }
            _ => return Err(AccountError::Unsupported),
        };
        // A denied GET never meant never-enrolled or eligible; Worker sticky
        // account/device guards remain authoritative for the same-material attempt.
        self.activate(command, authority)
    }
}
pub(super) fn require_prepared(summary: CustodySummary) -> Result<()> {
    if summary.enrollment.is_some_and(|i| i.uncertain || i.expired) {
        Err(AccountError::Conflict)
    } else {
        Ok(())
    }
}
