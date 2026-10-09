//! Exact frozen original capabilities. Their metadata does not recreate closure.
use super::*;
use mdreader_profile::{legacy_windows::ClosedOriginal, FrozenProfile};

pub(super) trait Original: FrozenProfile {
    fn path(&self) -> &Path;
    fn inventory(&mut self, limits: &Limits) -> Result<Inventory>;
}
impl Original for ClosedOwner {
    fn path(&self) -> &Path {
        ClosedOwner::path(self)
    }
    fn inventory(&mut self, limits: &Limits) -> Result<Inventory> {
        self.revalidate()?;
        let choices = super::super::super::backup_inventory::selections(self.path(), limits)?;
        Inventory::capture(self.path(), &choices, limits)
    }
}
impl Original for ClosedOriginal {
    fn path(&self) -> &Path {
        ClosedOriginal::path(self)
    }
    fn inventory(&mut self, _: &Limits) -> Result<Inventory> {
        ClosedOriginal::inventory(self)
    }
}
