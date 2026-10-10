//! Static admission phases only. Test binaries expose no profile spelling,
//! original bytes, OS handles or nested error text in these diagnostics.
use super::*;

pub(super) fn at<T>(phase: &'static str, result: Result<T>) -> Result<T> {
    #[cfg(any(test, feature = "admission-test-diagnostics"))]
    if result.is_err() {
        eprintln!("Windows legacy original admission refused at {phase}");
    }
    #[cfg(not(any(test, feature = "admission-test-diagnostics")))]
    let _ = phase;
    result
}
pub(super) fn conflict(phase: &'static str) -> Error {
    let _ = at::<()>(phase, Err(Error::Conflict));
    Error::Conflict
}
