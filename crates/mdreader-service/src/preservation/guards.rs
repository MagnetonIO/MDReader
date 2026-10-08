//! Real, independent service producer custody. No absence/readiness booleans.
use super::*;
use crate::{accounts, capture::admission as capture, daemon, device};
use mdreader_core::ReaderStore;
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

pub(crate) struct Owners {
    pub store: Arc<Mutex<ReaderStore>>,
    pub daemon: Arc<daemon::Coordinator>,
    pub accounts: Arc<accounts::Coordinator>,
    pub devices: Arc<device::Coordinator>,
    pub capture: Arc<capture::Coordinator>,
    pub providers: Option<Arc<dyn crate::provider::ProviderFactory>>,
}
// Explicit release order: independent producers first, scheduler last. A
// quarantined session retains this whole value until actual service teardown.
pub(super) struct Guards {
    capture: Option<capture::FrozenCapture>,
    devices: Option<device::FrozenDevices>,
    accounts: Option<accounts::FrozenAccounts>,
    daemon: Option<daemon::FrozenReaderDaemon>,
}
impl Owners {
    pub(super) fn acquire(
        &self,
        scope: &Scope,
        deadline: Instant,
    ) -> std::result::Result<Box<Guards>, (Error, Option<Box<Guards>>)> {
        let daemon = self
            .daemon
            .freeze(scope.operation, scope.owner_epoch, deadline)
            .map_err(|error| (daemon_error(error), None))?;
        let mut guards = Box::new(Guards {
            capture: None,
            devices: None,
            accounts: None,
            daemon: Some(daemon),
        });
        let result = (|| {
            let executor = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| Error::Unavailable)?;
            if let Some(providers) = &self.providers {
                let context = mdreader_provider::IoContext::new(
                    scope.operation,
                    deadline,
                    1,
                    1,
                    mdreader_provider::Cancellation::default(),
                )
                .map_err(|_| Error::Invalid)?;
                executor
                    .block_on(providers.settle(&context))
                    .map_err(|error| match error {
                        mdreader_provider::ProviderError::Unsupported => Error::Unsupported,
                        _ => Error::Unavailable,
                    })?;
            }
            guards.accounts = Some(
                executor
                    .block_on(
                        self.accounts
                            .freeze(scope.operation, scope.owner_epoch, deadline),
                    )
                    .map_err(|_| Error::Unavailable)?,
            );
            guards.devices = Some(
                self.devices
                    .freeze(scope.operation, scope.owner_epoch, deadline)
                    .map_err(|_| Error::Unavailable)?,
            );
            guards.capture = Some(
                self.capture
                    .freeze(scope.operation, scope.owner_epoch, deadline)
                    .map_err(capture_error)?,
            );
            guards.verify(self, scope)
        })();
        match result {
            Ok(()) => Ok(guards),
            Err(error) => Err((error, Some(guards))),
        }
    }
}
impl Guards {
    pub fn with_main<T>(
        &self,
        action: impl FnOnce(&rusqlite::Connection) -> mdreader_profile::Result<T>,
    ) -> Result<T> {
        self.daemon
            .as_ref()
            .ok_or(Error::NotHeld)?
            .with_library(|connection| Ok(action(connection)))
            .map_err(daemon_error)?
            .map_err(profile_error)
    }

    pub fn release(
        mut self: Box<Self>,
        owners: &Owners,
        scope: &Scope,
    ) -> std::result::Result<(), Box<Self>> {
        // Keep dispatch/scheduler denied until independent producer releases
        // were positively observed. An uncertain release retains that barrier.
        drop(self.capture.take());
        drop(self.devices.take());
        drop(self.accounts.take());
        let resumed = (|| {
            owners
                .accounts
                .resumed(scope.operation, scope.owner_epoch)
                .map_err(|_| Error::Quarantined)?;
            owners
                .devices
                .resumed(scope.operation, scope.owner_epoch)
                .map_err(|_| Error::Quarantined)?;
            let _capture = owners.capture.enter().map_err(capture_error)?;
            Ok::<_, Error>(())
        })();
        if resumed.is_err() {
            return Err(self);
        }
        drop(self.daemon.take());
        if owners.daemon.enter().is_err() {
            return Err(self);
        }
        Ok(())
    }

    pub fn verify(&self, owners: &Owners, scope: &Scope) -> Result<()> {
        self.daemon
            .as_ref()
            .ok_or(Error::NotHeld)?
            .revalidate(&owners.daemon, scope.operation, scope.owner_epoch)
            .map_err(daemon_error)?;
        self.accounts
            .as_ref()
            .ok_or(Error::NotHeld)?
            .revalidate(&owners.accounts, scope.operation, scope.owner_epoch)
            .map_err(|_| Error::StaleOwner)?;
        self.devices
            .as_ref()
            .ok_or(Error::NotHeld)?
            .revalidate(&owners.devices, scope.operation, scope.owner_epoch)
            .map_err(|_| Error::StaleOwner)?;
        self.capture
            .as_ref()
            .ok_or(Error::NotHeld)?
            .revalidate(
                &owners.capture,
                &owners.store,
                scope.operation,
                scope.owner_epoch,
            )
            .map_err(capture_error)
    }
}
fn daemon_error(error: daemon::Error) -> Error {
    match error {
        daemon::Error::Busy => Error::Busy,
        daemon::Error::Deadline => Error::Deadline,
        daemon::Error::Conflict => Error::StaleOwner,
        daemon::Error::Unsupported => Error::Unsupported,
        daemon::Error::Unavailable => Error::Unavailable,
    }
}
fn capture_error(error: capture::Error) -> Error {
    match error {
        capture::Error::Busy => Error::Busy,
        capture::Error::Deadline => Error::Deadline,
        capture::Error::Conflict => Error::StaleOwner,
        capture::Error::Unsupported => Error::Unsupported,
        capture::Error::Unavailable => Error::Unavailable,
    }
}

pub(super) fn profile_error(error: mdreader_profile::Error) -> Error {
    match error {
        mdreader_profile::Error::Unsupported => Error::Unsupported,
        mdreader_profile::Error::Bounds => Error::Bounds,
        mdreader_profile::Error::Conflict | mdreader_profile::Error::Integrity => Error::StaleOwner,
        mdreader_profile::Error::NotQuiescent => Error::Deadline,
        mdreader_profile::Error::Invalid
        | mdreader_profile::Error::InvalidDestination
        | mdreader_profile::Error::Unclassified => Error::Invalid,
        mdreader_profile::Error::Storage => Error::Unavailable,
        mdreader_profile::Error::ReconciliationRequired => Error::Uncertain,
    }
}
