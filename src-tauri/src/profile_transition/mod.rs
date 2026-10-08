//! Native profile transition registration boundary. Not registered in app startup
//! yet: legacy/reader/renderer writers currently lack a coordinated freeze.
//! Keep this unavailable; no caller-supplied readiness flag grants activation.
use mdreader_profile::{Error, Participant, Result, REQUIRED_PARTICIPANTS};

pub(crate) struct NativeProfileTransition;
impl NativeProfileTransition {
    pub(crate) fn missing_participants(&self) -> &'static [Participant] {
        &REQUIRED_PARTICIPANTS
    }
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
