//! Explicit pairing restart remains bound to the selected library and OS slot.
use super::*;
use mdreader_sync::authority::{PairRenewalPreparation, PairRenewalResult};
impl Actor {
    pub async fn restart_pair(&self, command: &Command) -> Result<Outcome> {
        let binding = self.bound()?;
        let login = self.login(command).await?;
        self.shared.phase(command, Phase::PreparingEnrollment)?;
        let progress = self
            .setup
            .factory
            .prepare_pair_renewal(&login, &binding.reference, &binding.library.to_string())
            .await?;
        let result = match progress {
            PairRenewalPreparation::Complete(result) => result,
            PairRenewalPreparation::Login(enrollment) => {
                drop(login);
                let login = self.nonce_login(command, &enrollment).await?;
                self.setup
                    .factory
                    .request_pair_renewal(&login, &binding.reference, &binding.library.to_string())
                    .await?
            }
        };
        match result {
            PairRenewalResult::Request(packet) => Ok(Outcome::PublicPacket {
                packet_kind: PublicPacketKind::Request,
                packet: packet.out_of_band_text()?,
            }),
            PairRenewalResult::ApprovalRequired => Ok(Outcome::PairApprovalRequired),
            PairRenewalResult::NotCommitted => Err(AccountError::Conflict),
        }
    }
}
