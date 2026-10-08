use super::*;
impl Actor {
    pub async fn join(&mut self, command: &Command, library: Uuid) -> Result<Outcome> {
        let login = self.login(command).await?;
        let binding = self
            .ensure_binding(command, library, &login, Some(CandidateKind::Pair))
            .await?;
        let summary = self.refresh_custody(command).await?;
        if summary.lifecycle
            != (CustodyLifecycle::PairCandidate {
                trusted_approval_staged: false,
            })
        {
            return Err(AccountError::Conflict);
        }
        enrollment::require_prepared(summary)?;
        self.shared.phase(command, Phase::PreparingEnrollment)?;
        let enrollment = self
            .setup
            .factory
            .prepare_pair_login(&login, &binding.reference, &library.to_string())
            .await?;
        drop(login);
        let login = self.nonce_login(command, &enrollment).await?;
        let packet = self
            .setup
            .factory
            .request_pair(&login, &binding.reference, &library.to_string())
            .await?;
        Ok(Outcome::PublicPacket {
            packet_kind: PublicPacketKind::Request,
            packet: packet.out_of_band_text()?,
        })
    }
    pub async fn show_pair(&self, command: &Command) -> Result<Outcome> {
        let binding = self.bound()?;
        let login = self.login(command).await?;
        let packet = self
            .setup
            .factory
            .pair_request_packet(&login, &binding.reference, &binding.library.to_string())
            .await?;
        // Reopening the local OOB packet is not a receipt of server approval.
        Ok(Outcome::PublicPacket {
            packet_kind: PublicPacketKind::Request,
            packet: packet.out_of_band_text()?,
        })
    }
    pub async fn accept_pair(&mut self, command: &Command, packet: &str) -> Result<Outcome> {
        let binding = self.bound()?;
        let login = self.login(command).await?;
        self.shared
            .phase(command, Phase::WaitingForDeviceApproval)?;
        let authority = self
            .setup
            .factory
            .accept_pair(
                login,
                &binding.reference,
                &binding.library.to_string(),
                packet,
                true,
            )
            .await?;
        self.activate(command, authority)
    }
    pub async fn approve(&self, command: &Command, packet: &str) -> Result<Outcome> {
        self.shared
            .phase(command, Phase::WaitingForDeviceApproval)?;
        let active = self.active()?;
        // This typed action means the user supplied/verified the full OOB packet.
        // No server roster lookup or automatic pairing may invoke it.
        let packet = self
            .setup
            .factory
            .approve_pair(&active.authority, packet, true)
            .await?;
        Ok(Outcome::PublicPacket {
            packet_kind: PublicPacketKind::Approval,
            packet: packet.out_of_band_text()?,
        })
    }
    pub async fn retry_approval(&self) -> Result<Outcome> {
        let active = self.active()?;
        let packet = self
            .setup
            .factory
            .retry_pair_approval(&active.authority, true)
            .await?;
        Ok(Outcome::PublicPacket {
            packet_kind: PublicPacketKind::Approval,
            packet: packet.out_of_band_text()?,
        })
    }
}
