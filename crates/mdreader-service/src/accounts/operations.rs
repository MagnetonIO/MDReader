use super::*;
use actor::{Actor, Command};
mod outgoing;
mod pair_renewal;
mod pairing;
mod pump;
mod recovery;
mod resolution;

impl Actor {
    pub async fn execute(&mut self, command: &Command) -> Result<Outcome> {
        let intent = match &command.action {
            actor::Action::ListPending(scope) => {
                return self.list_pending(command, scope.clone()).await
            }
            actor::Action::ResolvePending(scope, request) => {
                return self
                    .resolve_pending(command, scope.clone(), request.clone())
                    .await
            }
            actor::Action::Account(intent) => intent,
            actor::Action::SelectedSync(selection, purpose) => {
                return self
                    .selected_sync(command, selection.clone(), *purpose)
                    .await;
            }
            actor::Action::InspectSelected(selection, purpose) => {
                return self
                    .inspect_selected(command, selection.clone(), *purpose)
                    .await;
            }
        };
        match intent {
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
            AccountIntent::RestartPairRequest {} => self.restart_pair(command).await,
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
}
