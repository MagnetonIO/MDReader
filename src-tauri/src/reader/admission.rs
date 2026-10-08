//! Shared by every clone of the one native service transport.
use crate::profile_transition::{EffectLease, NativeProfileTransition};
use mdreader_profile::Participant;
use std::sync::{Arc, OnceLock, Weak};
#[derive(Default)]
pub(super) struct Gate(OnceLock<Weak<NativeProfileTransition>>);
impl Gate {
    pub(super) fn bind(&self, profile: &Arc<NativeProfileTransition>) -> Result<(), String> {
        self.0
            .set(Arc::downgrade(profile))
            .map_err(|_| "Reader admission already bound".into())
    }
    pub(super) fn enter(&self) -> Result<Option<EffectLease>, String> {
        let Some(profile) = self.0.get() else {
            return Ok(None);
        };
        profile
            .upgrade()
            .ok_or("Reader admission is unavailable")?
            .effect(Participant::ReaderDaemon)
            .map(Some)
            .map_err(|_| "Profile preservation is in progress".into())
    }
}
