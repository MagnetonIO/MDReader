//! One-shot failure injection after real candidate finalization. Thread-local
//! test state cannot affect production or an unrelated concurrent fixture.
use std::cell::Cell;
thread_local! {
    static INTERRUPT: Cell<bool> = const { Cell::new(false) };
}
pub(super) fn interrupt_before_staging() -> bool {
    INTERRUPT.replace(false)
}
pub(in crate::profile_transition::offline::windows_workspace) fn interrupt_next() {
    INTERRUPT.set(true);
}

#[path = "legacy_driver.rs"]
pub(in crate::profile_transition::offline::windows_workspace) mod driver;
