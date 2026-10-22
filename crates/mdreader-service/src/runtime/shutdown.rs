//! Native closed-owner handoff: wake the blocking listener only after a verified
//! held preservation request. Closing proof is the real OS writer lease later.
use super::*;
use std::net::{SocketAddr, TcpStream};
impl Runtime {
    pub(crate) fn install_shutdown_wake(&self, address: SocketAddr) -> Result<()> {
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err(ReaderError::Unavailable);
        }
        let mut wake = self
            .shutdown_wake
            .lock()
            .map_err(|_| ReaderError::Unavailable)?;
        if wake.is_some() {
            return Err(ReaderError::Unavailable);
        }
        *wake = Some(address);
        Ok(())
    }
    pub(crate) fn shutdown_requested(&self) -> bool {
        if self.retiring.load(std::sync::atomic::Ordering::Acquire) {
            return true;
        }
        #[cfg(feature = "sync-native")]
        {
            self.preservation
                .as_ref()
                .is_ok_and(|session| session.shutdown_requested())
        }
        #[cfg(not(feature = "sync-native"))]
        {
            false
        }
    }
    /// A client of another build asked this owner to exit so it can start its
    /// own. The same build never retires itself.
    pub(crate) fn retire(&self, build: &str) -> Result<serde_json::Value> {
        super::super::build::validate(build)?;
        if build == super::super::build::own() {
            return Err(protocol_error("the running reader service is this build"));
        }
        self.retiring
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(serde_json::json!({"retiring":true}))
    }
    pub(crate) fn wake_shutdown_if_requested(&self) {
        if !self.shutdown_requested() {
            return;
        }
        if let Ok(wake) = self.shutdown_wake.lock() {
            if let Some(address) = *wake {
                let _ = TcpStream::connect_timeout(&address, Duration::from_secs(1));
            }
        }
    }
    pub(crate) fn stop_for_shutdown(&self) {
        self.daemon_admission.stop_worker();
        let _ = self.wake.try_send(());
    }
}
