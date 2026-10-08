//! Unit-test execution barrier only, after real preparation/files/image validation.
//! No input, keys, authority, result substitution or production branch exists.
use super::*;
type Pause = Arc<dyn Fn() -> Result<()> + Send + Sync>;
pub(super) type Gate = Mutex<Option<Pause>>;
impl IncomingCoordinator {
    pub(crate) fn set_materialization_gate(
        &self,
        pause: Option<Arc<dyn Fn() -> Result<()> + Send + Sync>>,
    ) {
        *self.materialization_gate.lock().expect("test gate") = pause;
    }
    pub(super) fn pause_after_materialization(&self) -> Result<()> {
        let pause = self
            .materialization_gate
            .lock()
            .map_err(|_| Error::Inactive)?
            .clone();
        if let Some(pause) = pause {
            pause()?;
        }
        Ok(())
    }
}
