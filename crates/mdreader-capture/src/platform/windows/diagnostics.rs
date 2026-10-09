//! Unit-test-only fixed boundary labels. No paths, IDs, content or error strings.
use crate::{CaptureError, Result};

pub(crate) fn record<T>(stage: &'static str, result: &Result<T>) {
    match result {
        Ok(_) => eprintln!("capture-boundary {stage} ok"),
        Err(CaptureError::Io(error)) => eprintln!(
            "capture-boundary {stage} io={}",
            error.raw_os_error().unwrap_or(-1)
        ),
        Err(CaptureError::Database(_)) => eprintln!("capture-boundary {stage} database"),
        Err(_) => eprintln!("capture-boundary {stage} refused"),
    }
}

pub(crate) fn native_status(status: i32) {
    eprintln!("capture-boundary rename.native_call ntstatus={status}");
}
