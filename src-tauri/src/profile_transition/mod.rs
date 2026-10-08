//! Native preservation admission. Unix desktop startup registers the two strict
//! JSON owners. Other actual writers are still missing; installed preservation
//! and selector activation remain unavailable until they are composed.
mod admission;
pub(crate) mod persistence;
mod registry;
#[cfg(all(unix, not(mobile)))]
mod service;
mod storage_layout;
pub(crate) use storage_layout::DatabaseKind;
pub(crate) use storage_layout::DatabaseRegistration;
#[cfg(all(unix, not(mobile)))]
pub(crate) mod startup;
#[cfg(all(test, unix))]
mod tests;
pub(crate) use admission::EffectLease;
use mdreader_profile::{
    Error, FreezeOwner, FrozenProfile, Inventory, InventorySelection, Limits, Participant, Result,
    RootBinding, VerifiedSnapshot,
};
pub(crate) use registry::{FreezeScope, FrozenParticipant, LiveParticipant};
use std::{path::Path, sync::Arc, time::Duration};

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
    /// Production installed-profile preservation uses the outer envelope so
    /// exact preflush originals are never mistaken for latest current JSON.
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
    pub(crate) fn prepare(&self) -> Result<()> {
        Err(Error::NotQuiescent)
    }
    pub(crate) fn activate(&self) -> Result<()> {
        Err(Error::NotQuiescent)
    }
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
