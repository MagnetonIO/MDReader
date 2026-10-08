use super::*;
use actor::{Actor, Command};
mod pairing;
mod recovery;

impl Actor {
    pub async fn execute(&mut self, command: &Command) -> Result<Outcome> {
        match &command.intent {
            AccountIntent::SignIn {} => {
                let binding = self.bound()?;
                let login = self.login(command).await?;
                let authority = self
                    .setup
                    .factory
                    .resume(login, &binding.reference, &binding.library.to_string())
                    .await?;
                self.activate(command, authority)
            }
            AccountIntent::Bootstrap { remote_library } => {
                self.bootstrap(command, *remote_library).await
            }
            AccountIntent::JoinDevice { remote_library } => {
                self.join(command, *remote_library).await
            }
            AccountIntent::Recover { remote_library } => {
                self.recover(command, *remote_library).await
            }
            AccountIntent::ReconcileEnrollment {} => self.reconcile(command).await,
            AccountIntent::RenewEnrollment {} => self.renew(command).await,
            AccountIntent::ShowPairRequest {} => self.show_pair(command).await,
            AccountIntent::AcceptDeviceApproval { packet } => {
                self.accept_pair(command, packet).await
            }
            AccountIntent::ApproveDevice { packet } => self.approve(command, packet).await,
            AccountIntent::RetryDeviceApproval {} => self.retry_approval().await,
            AccountIntent::DeviceApprovalHandedOff {} => {
                let active = self.active()?;
                self.setup
                    .factory
                    .finish_pair_handoff(&active.authority, true)
                    .await?;
                Ok(Outcome::HandoffComplete)
            }
            AccountIntent::ExportRecovery {} => self.export_recovery(command).await,
            AccountIntent::RetryCustodyWrite {} => self.persist_exact(command).await,
            AccountIntent::SyncNow {} => self.sync(command).await,
        }
    }
    async fn sync(&self, command: &Command) -> Result<Outcome> {
        self.shared.phase(command, Phase::Syncing)?;
        let active = self.active()?;
        let barrier = self
            .setup
            .factory
            .session_controller()
            .active_scope_barrier();
        let owner = crate::sync::TransportOwner {
            owner: &self.owner,
            active_scope: &barrier,
        };
        let report = active
            .authority
            .run(
                &self.setup.transport,
                &owner,
                Budget {
                    outgoing: 5,
                    pages: 2,
                    page_size: 5,
                    transfer_bytes: 8 * 1024 * 1024,
                },
            )
            .await?;
        self.shared.check(command)?;
        Ok(Outcome::Sync {
            committed: report.committed,
            conflicts: report.conflicts,
            received: report.received,
            applied: report.applied,
            acknowledged: report.acknowledged,
            remote_pending: report.remote_pending,
            local_pending: report.local_pending,
            pending: None,
        })
    }
}
