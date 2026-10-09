//! Explicit status inspection: serialized native custody metadata, no sign-in.
use super::*;
use std::{sync::mpsc, time::Duration};

impl Coordinator {
    /// Called on the native request worker, never on an async/UI executor. The
    /// actor retains even timed-out inspection work; its freeze barrier follows
    /// that work on the same serial queue. No SQLite/view lock spans the wait.
    pub(super) fn inspected_status(&self) -> Result<AccountStatus> {
        let Some(sender) = &self.sender else {
            return self.status();
        };
        let receive = {
            let state = self
                .shared
                .view
                .lock()
                .map_err(|_| AccountError::Interrupted)?;
            if !state.retained || state.running.is_some() {
                drop(state);
                return self.status();
            }
            if state.frozen.is_some() {
                return Err(AccountError::Busy);
            }
            if state.custody_inspected {
                if state.custody.is_none() {
                    return Err(AccountError::CustodyUnavailable);
                }
                drop(state);
                return self.status();
            }
            let (reply, receive) = mpsc::sync_channel(1);
            sender
                .try_send(actor::Message::InspectCustody {
                    epoch: state.epoch,
                    reply,
                })
                .map_err(|error| match error {
                    tokio::sync::mpsc::error::TrySendError::Full(_) => AccountError::Busy,
                    tokio::sync::mpsc::error::TrySendError::Closed(_) => AccountError::Interrupted,
                })?;
            receive
        };
        receive
            .recv_timeout(Duration::from_secs(6))
            .map_err(|_| AccountError::Interrupted)??;
        self.status()
    }
}

impl actor::Actor {
    pub(super) async fn inspect_custody(&self, epoch: u64) -> Result<()> {
        {
            let state = self
                .shared
                .view
                .lock()
                .map_err(|_| AccountError::Interrupted)?;
            if state.epoch != epoch || state.running.is_some() || state.frozen.is_some() {
                return Err(AccountError::Cancelled);
            }
            // Two concurrent explicit Status requests can already be queued.
            // The second reuses the first result rather than rereading the vault.
            if state.custody_inspected {
                return if state.custody.is_some() {
                    Ok(())
                } else {
                    Err(AccountError::CustodyUnavailable)
                };
            }
        }
        let binding = self.bound()?;
        let summary = self
            .setup
            .factory
            .custody_summary(&binding.reference, &binding.library.to_string())
            .await?;
        let mut state = self
            .shared
            .view
            .lock()
            .map_err(|_| AccountError::Interrupted)?;
        if state.epoch != epoch || state.running.is_some() || state.frozen.is_some() {
            return Err(AccountError::Cancelled);
        }
        state.custody_inspected = true;
        state.custody = summary.map(Into::into);
        if state.custody.is_none() {
            return Err(AccountError::CustodyUnavailable);
        }
        Ok(())
    }
}
