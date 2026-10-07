//! Shared hook entrypoint for standalone and packaged MDReader executables.
use crate::{Adapter, CaptureError, CaptureOrigin, EnqueueOutcome, Limits, ProjectPolicy, Spool};
use std::{ffi::OsString, io::Read, path::Path};

pub fn run(args: impl IntoIterator<Item = OsString>, mut input: impl Read) -> crate::Result<()> {
    let args: Vec<_> = args.into_iter().collect();
    if args.len() < 4 || args.len() > 5 {
        return Err(CaptureError::Invalid);
    }
    let origin = match args[0].to_str() {
        Some("hook") => CaptureOrigin::Live,
        Some("backfill") => CaptureOrigin::SelectedBackfill,
        _ => return Err(CaptureError::Invalid),
    };
    let adapter = match args[1].to_str() {
        Some("claude") => Adapter::ClaudeCode,
        Some("codex") => Adapter::CodexHook,
        Some("codex-notify") => Adapter::CodexNotify,
        _ => return Err(CaptureError::Invalid),
    };
    if (adapter == Adapter::CodexNotify) != (args.len() == 5) {
        return Err(CaptureError::Invalid);
    }
    let policy = ProjectPolicy::load(Path::new(&args[2]))?;
    if !policy.enabled {
        return Ok(());
    }
    let limits = Limits::default();
    let mut bytes = Vec::new();
    if adapter == Adapter::CodexNotify {
        bytes.extend_from_slice(args[4].to_str().ok_or(CaptureError::Invalid)?.as_bytes());
    } else {
        input
            .by_ref()
            .take(limits.max_event_bytes as u64 + 1)
            .read_to_end(&mut bytes)?;
    }
    if bytes.len() > limits.max_event_bytes {
        return Err(CaptureError::TooLarge);
    }
    // The daemon's projection/receipt work briefly holds this same nonblocking
    // lock. Retry only explicit contention within the hook's three-second budget;
    // I/O, capacity and policy errors remain failures, never presumed durable.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let outcome = loop {
        let result = Spool::open(Path::new(&args[3]), limits.clone())
            .and_then(|spool| spool.enqueue(adapter, &bytes, &policy, origin));
        match result {
            Err(CaptureError::Busy) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            result => break result?,
        }
    };
    if let EnqueueOutcome::Skipped { reason } = outcome {
        eprintln!("MDReader capture skipped: {reason:?}");
    }
    Ok(())
}
