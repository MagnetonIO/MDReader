//! Actual native participant registration and preservation-only freeze. Startup
//! must supply every real owner; this cannot infer a writer from a pathname.
use super::admission::{Admission, DeniedLease, EffectLease, MAX_FREEZE_WAIT};
use super::storage_layout::{self, DatabaseKind, DatabaseRegistration};
use mdreader_profile::{
    Error, FrozenProfile, Inventory, Participant, REQUIRED_PARTICIPANTS, Result, RootBinding,
};
use rusqlite::Connection;
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

pub(crate) struct FreezeScope<'a> {
    operation: Uuid,
    owner_epoch: Uuid,
    root: &'a RootBinding,
    deadline: Instant,
}
impl FreezeScope<'_> {
    pub(crate) fn operation(&self) -> Uuid {
        self.operation
    }
    pub(crate) fn owner_epoch(&self) -> Uuid {
        self.owner_epoch
    }
    pub(crate) fn root(&self) -> &RootBinding {
        self.root
    }
    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }
}

/// Implemented only by real native owner adapters, never renderer input. Freeze
/// must deny independent admission, join issued work and strictly flush latest
/// state before returning its owned, noncloneable capability. A partial failure
/// must not return a capability. Drop releases that participant's freeze only
/// after any borrowed callback exits; it must not initiate a write on release.
pub(crate) trait LiveParticipant: Send + Sync {
    fn participant(&self) -> Participant;
    fn root(&self) -> &RootBinding;
    fn freeze(&self, scope: &FreezeScope<'_>) -> Result<Box<dyn FrozenParticipant>>;
    /// Notification only, after all frozen capabilities have dropped and the
    /// exact global admission lease has explicitly resumed. Never a writer grant.
    fn post_resume(&self, _scope: &FreezeScope<'_>) -> Result<()> {
        Ok(())
    }
}
pub(crate) trait FrozenParticipant {
    /// Bind exact native owner, root, operation and epoch, and reject replacement.
    fn revalidate(&mut self, scope: &FreezeScope<'_>) -> Result<()>;
    /// Explicitly confirm remote release before global admission can resume.
    /// Default local capabilities release through their retained RAII guards.
    fn release(&mut self, _scope: &FreezeScope<'_>) -> Result<()> {
        Ok(())
    }
    /// Output-only archival evidence, not files to be restored into the current
    /// installed profile. Real JSON and editor participants supply their separate archival entries.
    fn preflush_json(&mut self) -> Result<Vec<mdreader_profile::bundle::PreflushArtifact>> {
        Ok(Vec::new())
    }
    /// A service owner writes only the stage-created exact target; it does not
    /// pretend to lend its SQLite connection across a process boundary.
    fn backup_database(
        &mut self,
        target: &mut mdreader_profile::database_backup::DatabaseBackupTarget,
    ) -> Result<mdreader_profile::database_backup::BackupReceipt> {
        #[cfg(unix)]
        {
            target.with_local_database(|relative, action| self.with_database(relative, action))
        }
        #[cfg(not(unix))]
        {
            let _ = target;
            Err(Error::Unsupported)
        }
    }
    /// Yield the actual registered owner's connection, or the specifically
    /// authorized fully closed legacy backup connection. Never reopen an SDK.
    fn with_database(
        &mut self,
        relative: &Path,
        action: &mut dyn FnMut(&Connection) -> Result<()>,
    ) -> Result<()>;
}
struct Registered {
    owner: Arc<dyn LiveParticipant>,
    databases: Vec<DatabaseRegistration>,
}
pub(crate) struct Registry {
    root: RootBinding,
    admission: Arc<Admission>,
    participants: Vec<Option<Registered>>,
}
impl Registry {
    pub(crate) fn new(inventory: &Inventory) -> Self {
        Self::for_root(inventory.binding().clone())
    }
    pub(crate) fn for_root(root: RootBinding) -> Self {
        Self {
            root,
            admission: Admission::new(),
            participants: (0..REQUIRED_PARTICIPANTS.len()).map(|_| None).collect(),
        }
    }
    /// Composition-only, before exposing this registry to writers. Registrations
    /// cannot replace each other or change while a capability is issued.
    pub(crate) fn register(
        &mut self,
        owner: Arc<dyn LiveParticipant>,
        databases: Vec<DatabaseRegistration>,
    ) -> Result<()> {
        self.admission.registration_open()?;
        if owner.root() != &self.root || databases.len() > 64 {
            return Err(Error::Invalid);
        }
        let index = slot(owner.participant())?;
        if self.participants[index].is_some() {
            return Err(Error::Conflict);
        }
        storage_layout::validate_registrations(
            self.participants
                .iter()
                .flatten()
                .flat_map(|registered| &registered.databases)
                .chain(&databases),
        )?;
        self.participants[index] = Some(Registered { owner, databases });
        Ok(())
    }
    pub(crate) fn validate_choices(
        &self,
        choices: &[mdreader_profile::InventorySelection],
    ) -> Result<()> {
        if !self.missing().is_empty() {
            return Err(Error::NotQuiescent);
        }
        storage_layout::validate_choices(
            self.participants
                .iter()
                .flatten()
                .flat_map(|registered| &registered.databases),
            choices,
        )
    }
    pub(crate) fn missing(&self) -> Vec<Participant> {
        REQUIRED_PARTICIPANTS
            .iter()
            .zip(&self.participants)
            .filter_map(|(participant, registered)| registered.is_none().then_some(*participant))
            .collect()
    }
    /// Every ordinary writer must acquire before issuing work, then carry the
    /// lease into its actual task until persistence/unknown receipt completes.
    pub(crate) fn effect(&self, participant: Participant) -> Result<EffectLease> {
        if self.participants[slot(participant)?].is_none() {
            return Err(Error::NotQuiescent);
        }
        self.admission.enter()
    }
    /// Preservation only. Capture inventory AFTER participant flush inside this
    /// callback and keep it in scope through snapshot publication. This method
    /// must never wrap selector activation: automatic resume would restart the
    /// old owner after a committed/uncertain switch. That needs durable handoff.
    pub(crate) fn with_frozen<T>(
        &self,
        timeout: Duration,
        action: impl FnOnce(&mut dyn FrozenProfile) -> Result<T>,
    ) -> Result<T> {
        if timeout.is_zero() || timeout > MAX_FREEZE_WAIT {
            return Err(Error::Bounds);
        }
        if !self.missing().is_empty() {
            return Err(Error::NotQuiescent);
        }
        let scope = FreezeScope {
            operation: Uuid::new_v4(),
            owner_epoch: self.admission.epoch(),
            root: &self.root,
            deadline: Instant::now().checked_add(timeout).ok_or(Error::Bounds)?,
        };
        // Declaration order ensures all participant handles drop BEFORE global
        // admission resumes, including partial-freeze/error/timeout unwinding.
        let denied = self
            .admission
            .deny_and_drain(scope.operation, scope.deadline)?;
        // If an adapter panics before structured finalization, never resume
        // global admission merely because stack unwinding drops capabilities.
        let mut finalization = Finalization {
            denied: &denied,
            armed: true,
        };
        let mut handles = Vec::with_capacity(REQUIRED_PARTICIPANTS.len());
        let result = (|| {
            for registered in &self.participants {
                if Instant::now() >= scope.deadline {
                    return Err(Error::NotQuiescent);
                }
                let registered = registered.as_ref().ok_or(Error::NotQuiescent)?;
                let handle = registered.owner.freeze(&scope)?;
                // Retain even the newest capability before any check can fail.
                handles.push((registered, handle));
                revalidate_capabilities(&scope, &denied, &mut handles)?;
            }
            let mut frozen = Frozen {
                scope: &scope,
                denied: &denied,
                handles: &mut handles,
            };
            frozen.revalidate()?;
            action(&mut frozen)
        })();
        // ONE exit path for every partial acquisition, timeout and callback
        // error. Check every acquired handle, not only the one that failed.
        let proof = revalidate_capabilities(&scope, &denied, &mut handles);
        proof?;
        let result = if Instant::now() >= scope.deadline {
            Err(Error::NotQuiescent)
        } else {
            result
        };
        // Keep the panic/quarantine guard armed through participant destruction
        // and resumption. No participant Drop may emit an early release notice.
        if matches!(&result, Err(Error::ReconciliationRequired)) {
            return result;
        }
        for (_, handle) in handles.iter_mut() {
            handle
                .release(&scope)
                .inspect_err(|_| denied.quarantine())?;
        }
        drop(handles);
        denied.resume()?;
        for registered in self.participants.iter().flatten() {
            registered.owner.post_resume(&scope)?;
        }
        finalization.armed = false;
        result
    }
}
fn slot(participant: Participant) -> Result<usize> {
    REQUIRED_PARTICIPANTS
        .iter()
        .position(|value| *value == participant)
        .ok_or(Error::Invalid)
}
struct Frozen<'a, 'root> {
    scope: &'a FreezeScope<'root>,
    denied: &'a DeniedLease<'a>,
    handles: &'a mut Vec<(&'root Registered, Box<dyn FrozenParticipant>)>,
}
impl FrozenProfile for Frozen<'_, '_> {
    fn binding(&self) -> &RootBinding {
        self.scope.root
    }
    fn revalidate(&mut self) -> Result<()> {
        revalidate_capabilities(self.scope, self.denied, self.handles)?;
        if Instant::now() >= self.scope.deadline {
            return Err(Error::NotQuiescent);
        }
        Ok(())
    }
    fn preflush_json(&mut self) -> Result<Vec<mdreader_profile::bundle::PreflushArtifact>> {
        self.revalidate()?;
        let mut artifacts = Vec::new();
        let mut bytes = 0usize;
        for (_, handle) in self.handles.iter_mut() {
            for artifact in handle.preflush_json()? {
                bytes = bytes
                    .checked_add(artifact.bytes.as_ref().map_or(0, Vec::len))
                    .ok_or(Error::Bounds)?;
                artifacts.push(artifact);
                if artifacts.len() > 3 || bytes > 18 * 1024 * 1024 {
                    return Err(Error::Bounds);
                }
            }
        }
        self.revalidate()?;
        Ok(artifacts)
    }
    fn backup_database(
        &mut self,
        target: &mut mdreader_profile::database_backup::DatabaseBackupTarget,
    ) -> Result<mdreader_profile::database_backup::BackupReceipt> {
        self.revalidate()?;
        let relative = target.request().source.relative.restore()?;
        let (_, handle) = self
            .handles
            .iter_mut()
            .find(|(registered, _)| {
                registered.databases.iter().any(|database| {
                    database.relative == relative && matches!(database.kind, DatabaseKind::Durable)
                })
            })
            .ok_or(Error::Invalid)?;
        let receipt = handle.backup_database(target)?;
        target.accept(&receipt)?;
        self.revalidate()?;
        Ok(receipt)
    }
    fn with_database(
        &mut self,
        relative: &Path,
        action: &mut dyn FnMut(&Connection) -> Result<()>,
    ) -> Result<()> {
        self.revalidate()?;
        let (_, handle) = self
            .handles
            .iter_mut()
            .find(|(registered, _)| {
                registered.databases.iter().any(|database| {
                    database.relative == relative && matches!(database.kind, DatabaseKind::Durable)
                })
            })
            .ok_or(Error::Invalid)?;
        let mut calls = 0;
        handle.with_database(relative, &mut |connection| {
            calls += 1;
            if calls != 1 {
                return Err(Error::Invalid);
            }
            action(connection)
        })?;
        if calls != 1 {
            return Err(Error::NotQuiescent);
        }
        self.revalidate()
    }
}

fn revalidate_capabilities(
    scope: &FreezeScope<'_>,
    denied: &DeniedLease<'_>,
    handles: &mut [(&Registered, Box<dyn FrozenParticipant>)],
) -> Result<()> {
    let admission_error = denied.revalidate().err();
    let mut owner_error = None;
    for (_, handle) in handles {
        if let Err(error) = handle.revalidate(scope) {
            // Continue checking all acquired owners before releasing any.
            if owner_error.is_none() {
                owner_error = Some(error);
            }
        }
    }
    if let Some(error) = owner_error.or(admission_error) {
        denied.quarantine();
        return Err(error);
    }
    Ok(())
}
struct Finalization<'a> {
    denied: &'a DeniedLease<'a>,
    armed: bool,
}
impl Drop for Finalization<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.denied.quarantine();
        }
    }
}
