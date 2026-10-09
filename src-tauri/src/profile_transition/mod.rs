//! Native preservation admission and actual owner composition. Backup exports
//! copies only; selector activation is a separate offline profile lifecycle.
mod admission;
#[cfg(not(mobile))]
pub(crate) mod backup;
mod backup_inventory;
#[cfg(not(mobile))]
pub(crate) mod offline;
pub(crate) mod persistence;
mod registry;
#[cfg(all(any(unix, windows), not(mobile)))]
mod service;
mod storage_layout;
pub(crate) mod workspace;
pub(crate) use storage_layout::DatabaseKind;
pub(crate) use storage_layout::DatabaseRegistration;
#[cfg(all(any(unix, windows), not(mobile)))]
pub(crate) mod startup;
#[cfg(all(test, unix))]
mod tests;
pub(crate) use admission::EffectLease;
use mdreader_profile::{Error, FreezeOwner, FrozenProfile, Participant, Result, RootBinding};
#[cfg(test)]
use mdreader_profile::{Inventory, InventorySelection, Limits, VerifiedSnapshot};
pub(crate) use registry::{FreezeScope, FrozenParticipant, LiveParticipant};
use std::sync::Arc;
#[cfg(test)]
use std::{path::Path, time::Duration};

pub(crate) struct NativeProfileTransition {
    registry: registry::Registry,
}
impl NativeProfileTransition {
    /// Trusted native startup binds only the selected root. The preservation
    /// inventory is captured after actual writer freeze, never during launch.
    pub(crate) fn for_root(root: RootBinding) -> Result<Self> {
        root.validate()?;
        Ok(Self {
            registry: registry::Registry::for_root(root),
        })
    }
    #[cfg(test)]
    pub(crate) fn new(inventory: &Inventory) -> Self {
        Self {
            registry: registry::Registry::new(inventory),
        }
    }
    pub(crate) fn missing_participants(&self) -> Vec<Participant> {
        self.registry.missing()
    }
    pub(crate) fn register(
        &mut self,
        owner: Arc<dyn LiveParticipant>,
        databases: Vec<DatabaseRegistration>,
    ) -> Result<()> {
        self.registry.register(owner, databases)
    }
    pub(crate) fn effect(&self, participant: Participant) -> Result<EffectLease> {
        self.registry.effect(participant)
    }
    /// Call on a bounded blocking worker. Strict flush finishes before this
    /// inventory is captured; participant capabilities remain held throughout
    /// copying/backups/hash verification/publication. No selector changes.
    #[cfg(test)]
    pub(crate) fn preserve(
        &self,
        choices: &[InventorySelection],
        destination: &Path,
        limits: &Limits,
        timeout: Duration,
    ) -> Result<VerifiedSnapshot> {
        self.registry.validate_choices(choices)?;
        self.registry.with_frozen(timeout, |frozen| {
            let root = frozen.binding().locator.restore()?;
            let inventory = Inventory::capture(&root, choices, limits)?;
            let mut owner = BorrowedFreeze { frozen };
            mdreader_profile::stage(&mut owner, inventory, destination, limits)
        })
    }
    /// Fixture composition with explicit inventory; production backup owns its
    /// classification after freeze and uses the same portable outer envelope.
    #[cfg(test)]
    pub(crate) fn preserve_bundle(
        &self,
        choices: &[InventorySelection],
        destination: &Path,
        limits: &Limits,
        timeout: Duration,
    ) -> Result<mdreader_profile::bundle::VerifiedBundle> {
        self.registry.validate_choices(choices)?;
        self.registry.with_frozen(timeout, |frozen| {
            let root = frozen.binding().locator.restore()?;
            let inventory = Inventory::capture(&root, choices, limits)?;
            let mut owner = BorrowedFreeze { frozen };
            mdreader_profile::bundle::stage_bundle(&mut owner, inventory, destination, limits)
        })
    }
    // Durable import/startup selector and both-owner shutdown handoff are not
    // registered. A complete participant list never grants these operations.
    #[cfg(test)]
    pub(crate) fn prepare(&self) -> Result<()> {
        Err(Error::NotQuiescent)
    }
    #[cfg(test)]
    pub(crate) fn activate(&self) -> Result<()> {
        Err(Error::NotQuiescent)
    }
    #[cfg(test)]
    pub(crate) fn rollback(&self) -> Result<()> {
        Err(Error::NotQuiescent)
    }
}
struct BorrowedFreeze<'a> {
    frozen: &'a mut dyn FrozenProfile,
}
impl FreezeOwner for BorrowedFreeze<'_> {
    fn with_frozen<T>(
        &mut self,
        expected: &RootBinding,
        action: impl FnOnce(&mut dyn FrozenProfile) -> Result<T>,
    ) -> Result<T> {
        if self.frozen.binding() != expected {
            return Err(Error::Conflict);
        }
        self.frozen.revalidate()?;
        action(&mut *self.frozen)
    }
}
