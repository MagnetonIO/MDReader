//! Watches enabled agents' log folders and hands changed log paths to the
//! service owner in bounded batches. OS callbacks only enqueue names; they
//! never read logs or touch SQLite. A cheap revision check relays changes
//! made by hooks and MCP in other processes, so the renderer needs no poll.
use super::AgentActivityChanged;
use crate::reader::ReaderBackend;
use mdreader_service::agents::activity::{ActivityRequest, Homes, IngestReply, TrackingState};
use mdreader_service::Request;
use notify_debouncer_mini::{
    new_debouncer,
    notify::{RecommendedWatcher, RecursiveMode},
    DebounceEventResult, Debouncer,
};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
use tauri::Manager;
use tauri_specta::Event;

const DEBOUNCE: Duration = Duration::from_millis(300);
const BATCH: usize = 64;
/// Emit per-session events up to this many; more reloads the whole list.
const MAX_SESSION_EVENTS: usize = 16;
const REVISION_EVERY: Duration = Duration::from_secs(1);
const TRACKING_EVERY: Duration = Duration::from_secs(15);

/// Roots to watch for the enabled agents, with their recursion mode.
fn roots(homes: &Homes, state: &[TrackingState]) -> Vec<(PathBuf, RecursiveMode)> {
    use mdreader_core::agent_activity::Agent;
    let mut roots = Vec::new();
    for t in state.iter().filter(|t| t.enabled && t.available) {
        match t.agent {
            Agent::Claude => roots.push((homes.claude_projects.clone(), RecursiveMode::Recursive)),
            Agent::Codex => {
                roots.push((homes.codex_sessions.clone(), RecursiveMode::Recursive));
                if let Some(parent) = homes.codex_index.parent() {
                    roots.push((parent.to_owned(), RecursiveMode::NonRecursive));
                }
            }
        }
    }
    roots
}

fn relevant(homes: &Homes, path: &Path) -> bool {
    path == homes.codex_index
        || (path.extension().is_some_and(|e| e == "jsonl")
            && (path.starts_with(&homes.claude_projects)
                || path.starts_with(&homes.codex_sessions)))
}

struct Watching {
    roots: Vec<(PathBuf, RecursiveMode)>,
    _watcher: Option<Debouncer<RecommendedWatcher>>,
}

fn arm(
    homes: &Homes,
    roots: Vec<(PathBuf, RecursiveMode)>,
    tx: &mpsc::SyncSender<Vec<PathBuf>>,
    overflow: &Arc<AtomicBool>,
) -> Watching {
    if roots.is_empty() {
        return Watching {
            roots,
            _watcher: None,
        };
    }
    let (emit, failed, homes_cb) = (tx.clone(), overflow.clone(), homes.clone());
    let watcher = new_debouncer(DEBOUNCE, move |result: DebounceEventResult| {
        let Ok(events) = result else {
            failed.store(true, Ordering::Release);
            return;
        };
        let paths: BTreeSet<PathBuf> = events
            .into_iter()
            .map(|e| e.path)
            .filter(|p| relevant(&homes_cb, p))
            .collect();
        if !paths.is_empty() && emit.try_send(paths.into_iter().collect()).is_err() {
            failed.store(true, Ordering::Release);
        }
    });
    let watcher = watcher.ok().and_then(|mut w| {
        let mut armed = false;
        for (root, mode) in &roots {
            armed |= w.watcher().watch(root, *mode).is_ok();
        }
        armed.then_some(w)
    });
    Watching {
        roots,
        _watcher: watcher,
    }
}

fn activity<T: serde::de::DeserializeOwned>(
    backend: &ReaderBackend,
    request: ActivityRequest,
) -> Option<T> {
    backend.typed(Request::AgentActivity { request }).ok()
}

struct Relay<'a> {
    app: &'a tauri::AppHandle,
    backend: &'a ReaderBackend,
    revision: Option<i64>,
}
impl Relay<'_> {
    fn emit(&self, changed: &[String]) {
        if changed.is_empty() {
            return;
        }
        if changed.len() > MAX_SESSION_EVENTS {
            let _ = AgentActivityChanged { session_id: None }.emit(self.app);
            return;
        }
        for id in changed {
            let _ = AgentActivityChanged {
                session_id: Some(id.clone()),
            }
            .emit(self.app);
        }
    }

    /// Ingest in bounded batches, re-queuing whatever a batch left pending.
    fn ingest(&mut self, mut paths: Vec<PathBuf>, stop: &AtomicBool) {
        while !paths.is_empty() && !stop.load(Ordering::Acquire) {
            let rest = paths.split_off(paths.len().min(BATCH));
            let Some(reply) =
                activity::<IngestReply>(self.backend, ActivityRequest::Ingest { paths })
            else {
                return;
            };
            self.emit(&reply.changed);
            self.revision = Some(reply.revision);
            paths = reply.pending.into_iter().chain(rest).collect();
        }
    }

    /// Changes from other processes (hooks, MCP) or from commands.
    fn check_revision(&mut self) {
        let Some(current) = activity::<i64>(self.backend, ActivityRequest::Revision) else {
            return;
        };
        if self.revision.is_some_and(|seen| seen != current) {
            let _ = AgentActivityChanged { session_id: None }.emit(self.app);
        }
        self.revision = Some(current);
    }
}

pub(super) fn run(app: tauri::AppHandle, stop: Arc<AtomicBool>, kick: Arc<AtomicBool>) {
    let Ok(home) = crate::agent_cli::home() else {
        return;
    };
    let homes = Homes::under(&home);
    let backend = app.state::<ReaderBackend>().inner().clone();
    let (tx, rx) = mpsc::sync_channel::<Vec<PathBuf>>(64);
    let overflow = Arc::new(AtomicBool::new(false));
    let mut watching = Watching {
        roots: vec![],
        _watcher: None,
    };
    let mut relay = Relay {
        app: &app,
        backend: &backend,
        revision: None,
    };
    let mut cadence = Cadence::default();
    while !stop.load(Ordering::Acquire) {
        let kicked = kick.swap(false, Ordering::AcqRel);
        if cadence.tracking_due(kicked, Instant::now()) {
            if let Some(state) = activity::<Vec<TrackingState>>(&backend, ActivityRequest::Tracking)
            {
                cadence.tracked(state.iter().any(|t| t.enabled), Instant::now());
                let desired = roots(&homes, &state);
                let rearm = desired != watching.roots;
                if rearm {
                    // Arm first, then catch up: events during the scan queue.
                    watching = arm(&homes, desired, &tx, &overflow);
                }
                if rearm || kicked {
                    overflow.store(true, Ordering::Release);
                }
            }
        }
        if overflow.swap(false, Ordering::AcqRel) && !watching.roots.is_empty() {
            if let Some(paths) = activity::<Vec<PathBuf>>(&backend, ActivityRequest::Discover) {
                relay.ingest(paths, &stop);
            }
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(mut paths) => {
                while let Ok(more) = rx.try_recv() {
                    paths.extend(more);
                }
                paths.sort();
                paths.dedup();
                relay.ingest(paths, &stop);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        if cadence.revision_due(Instant::now()) {
            relay.check_revision();
        }
    }
}

/// When the loop calls the owner. With no agent tracked it makes no calls
/// at all until a tracking command kicks it.
#[derive(Default)]
struct Cadence {
    tracked_at: Option<Instant>,
    revised_at: Option<Instant>,
    any_enabled: bool,
}
impl Cadence {
    fn tracking_due(&self, kicked: bool, now: Instant) -> bool {
        kicked
            || self.tracked_at.is_none()
            || (self.any_enabled
                && self
                    .tracked_at
                    .is_some_and(|t| now.duration_since(t) >= TRACKING_EVERY))
    }
    fn tracked(&mut self, any_enabled: bool, now: Instant) {
        self.tracked_at = Some(now);
        self.any_enabled = any_enabled;
    }
    fn revision_due(&mut self, now: Instant) -> bool {
        if !self.any_enabled
            || self
                .revised_at
                .is_some_and(|t| now.duration_since(t) < REVISION_EVERY)
        {
            return false;
        }
        self.revised_at = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_owner_calls_while_no_agent_is_tracked() {
        let start = Instant::now();
        let mut cadence = Cadence::default();
        assert!(cadence.tracking_due(false, start), "one startup check");
        cadence.tracked(false, start);
        for seconds in [1, 5, 60, 3600] {
            let later = start + Duration::from_secs(seconds);
            assert!(!cadence.tracking_due(false, later));
            assert!(!cadence.revision_due(later));
        }
        // Turning tracking on kicks the loop; polling then resumes.
        assert!(cadence.tracking_due(true, start));
        cadence.tracked(true, start);
        assert!(cadence.revision_due(start));
        assert!(!cadence.revision_due(start + Duration::from_millis(500)));
        assert!(cadence.revision_due(start + REVISION_EVERY));
        assert!(cadence.tracking_due(false, start + TRACKING_EVERY));
    }
}
