use super::*;
use std::sync::Condvar;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Tag {
    pub token: Uuid,
    pub operation: Uuid,
    pub owner_epoch: Uuid,
}
pub(super) struct State {
    pub active: usize,
    pub frozen: Option<Tag>,
    pub broken: bool,
    // Retain the acquired OS-lock guard after failed final proof/release. An
    // unlock error is uncertain, never proof of either resumed or held exclusion.
    pub quarantine: Option<mdreader_capture::FrozenSpool>,
}
pub(super) struct Shared {
    pub state: Mutex<State>,
    pub changed: Condvar,
}
impl Shared {
    pub fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>> {
        self.state.lock().map_err(|_| Error::Unavailable)
    }
    pub fn quarantine(&self, spool: Option<mdreader_capture::FrozenSpool>) {
        // Poison recovery is solely to retain denial and the held OS lock.
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.broken = true;
        if spool.is_some() {
            state.quarantine = spool;
        }
        self.changed.notify_all();
    }
}
/// Move into the actual effect task. Dropping an IPC future must not drop this
/// lease while an independently issued worker can still publish a receipt.
pub struct EffectLease {
    pub(super) shared: Arc<Shared>,
}
impl Drop for EffectLease {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().unwrap_or_else(|error| {
            let mut state = error.into_inner();
            state.broken = true;
            state
        });
        if state.active == 0 {
            state.broken = true;
        } else {
            state.active -= 1;
        }
        self.shared.changed.notify_all();
    }
}
