//! Renderer-bound strict flush. A completed native write receipt, never an
//! acknowledgement boolean, is the proof retained by both JSON participants.
use super::{JsonKind, JsonOwner, PersistenceError, PreflushJson, RendererIdentity, WriteReceipt};
use crate::profile_transition::{FreezeScope, FrozenParticipant, LiveParticipant};
use mdreader_profile::{Error, Participant, Result, RootBinding};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex, Weak,
    },
    time::Instant,
};
use uuid::Uuid;
mod resume;

#[derive(Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FlushRequest {
    pub request_id: String,
    pub identity: RendererIdentity,
}
#[derive(Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FlushSubmission {
    pub request: FlushRequest,
    pub session_generation: u32,
    pub settings_generation: u32,
    pub session_json: String,
    pub settings_json: String,
}

/// Native composition emits to the exact registered window. No caller-selected
/// labels, paths, deadlines or registry operation IDs cross this interface.
pub(crate) trait FlushChannel: Send + Sync {
    fn request(&self, window: &str, request: &FlushRequest) -> Result<()>;
    fn released(&self, window: &str, request: &FlushRequest) -> Result<()>;
}
pub(crate) struct FlushBridge {
    owner: Arc<JsonOwner>,
    root: RootBinding,
    channel: Arc<dyn FlushChannel>,
    phase: Mutex<Phase>,
    changed: Condvar,
}
enum Phase {
    Idle,
    Waiting(Pending),
    Ready(Arc<Completed>),
    Issued(Weak<Completed>),
    Released {
        operation: Uuid,
        epoch: Uuid,
        window: String,
        request: FlushRequest,
    },
    Quarantined,
}
struct Pending {
    operation: Uuid,
    epoch: Uuid,
    deadline: Instant,
    window: String,
    request: FlushRequest,
    preflush: Vec<PreflushJson>,
    submitted: bool,
}
struct Completed {
    bridge: Arc<FlushBridge>,
    operation: Uuid,
    epoch: Uuid,
    window: String,
    request: FlushRequest,
    receipts: [WriteReceipt; 2],
    preflush: Vec<PreflushJson>,
    quarantined: AtomicBool,
}
impl FlushBridge {
    pub(crate) fn new(
        owner: Arc<JsonOwner>,
        root: RootBinding,
        channel: Arc<dyn FlushChannel>,
    ) -> Result<Arc<Self>> {
        owner.verify_binding(&root).map_err(profile_error)?;
        Ok(Arc::new(Self {
            owner,
            root,
            channel,
            phase: Mutex::new(Phase::Idle),
            changed: Condvar::new(),
        }))
    }
    pub(crate) fn participants(self: &Arc<Self>) -> [Arc<dyn LiveParticipant>; 2] {
        [Participant::RendererSession, Participant::RendererSettings].map(|kind| {
            Arc::new(JsonParticipant {
                bridge: Arc::clone(self),
                kind,
            }) as Arc<dyn LiveParticipant>
        })
    }
    fn freeze(self: &Arc<Self>, scope: &FreezeScope<'_>) -> Result<Arc<Completed>> {
        self.owner
            .verify_binding(scope.root())
            .map_err(profile_error)?;
        let mut phase = self.phase.lock().map_err(|_| Error::NotQuiescent)?;
        if matches!(*phase, Phase::Idle) {
            let (window, identity) = self.owner.renderer().map_err(profile_error)?;
            let preflush = self
                .owner
                .begin_flush(scope.operation())
                .map_err(profile_error)?;
            let request = FlushRequest {
                request_id: Uuid::new_v4().to_string(),
                identity,
            };
            *phase = Phase::Waiting(Pending {
                operation: scope.operation(),
                epoch: scope.owner_epoch(),
                deadline: scope.deadline(),
                window: window.clone(),
                request: request.clone(),
                preflush,
                submitted: false,
            });
            drop(phase);
            if let Err(error) = self.channel.request(&window, &request) {
                let previous = {
                    let mut phase = self.phase.lock().map_err(|_| Error::NotQuiescent)?;
                    std::mem::replace(&mut *phase, Phase::Quarantined)
                };
                if let Phase::Ready(completed) = &previous {
                    completed.quarantined.store(true, Ordering::Release);
                }
                drop(previous);
                self.changed.notify_all();
                return Err(error);
            }
            phase = self.phase.lock().map_err(|_| Error::NotQuiescent)?;
        }
        loop {
            let completed = match &*phase {
                Phase::Ready(value) => Some(Arc::clone(value)),
                Phase::Issued(value) => value.upgrade(),
                Phase::Waiting(pending)
                    if pending.operation == scope.operation()
                        && pending.epoch == scope.owner_epoch() =>
                {
                    None
                }
                _ => return Err(Error::NotQuiescent),
            };
            if let Some(completed) = completed {
                if let Err(error) = completed.revalidate(scope) {
                    *phase = Phase::Quarantined;
                    drop(phase);
                    return Err(error);
                }
                *phase = Phase::Issued(Arc::downgrade(&completed));
                return Ok(completed);
            }
            let now = Instant::now();
            if now >= scope.deadline() {
                // Do not resume a timed-out renderer/scoped blocking write. It
                // may still finish; its owner remains denied until recreated.
                *phase = Phase::Quarantined;
                return Err(Error::NotQuiescent);
            }
            phase = self
                .changed
                .wait_timeout(phase, scope.deadline() - now)
                .map_err(|_| Error::NotQuiescent)?
                .0;
        }
    }
    /// Invoked on an actual retained blocking worker. The fresh request token
    /// must have been emitted to this window during the active native freeze.
    pub(crate) fn submit(
        self: &Arc<Self>,
        window: &str,
        submission: FlushSubmission,
    ) -> std::result::Result<[WriteReceipt; 2], PersistenceError> {
        if submission.session_json.len() > super::types::MAX_JSON_BYTES
            || submission.settings_json.len() > super::types::MAX_JSON_BYTES
        {
            return Err(PersistenceError::TooLarge);
        }
        let (operation, epoch) = {
            let mut phase = self
                .phase
                .lock()
                .map_err(|_| PersistenceError::Unavailable)?;
            let Phase::Waiting(pending) = &mut *phase else {
                return Err(PersistenceError::Conflict);
            };
            if pending.submitted
                || pending.window != window
                || pending.request.request_id != submission.request.request_id
                || pending.request.identity.owner_epoch != submission.request.identity.owner_epoch
                || pending.request.identity.renderer_epoch
                    != submission.request.identity.renderer_epoch
                || Instant::now() >= pending.deadline
            {
                return Err(PersistenceError::Conflict);
            }
            pending.submitted = true;
            (pending.operation, pending.epoch)
        };
        let result = (|| {
            let session = self.owner.flush(
                operation,
                window,
                &submission.request.identity,
                JsonKind::Session,
                submission.session_generation,
                &submission.session_json,
            )?;
            let settings = self.owner.flush(
                operation,
                window,
                &submission.request.identity,
                JsonKind::Settings,
                submission.settings_generation,
                &submission.settings_json,
            )?;
            let receipts = [session, settings];
            self.owner.revalidate_flush(operation, &receipts)?;
            Ok(receipts)
        })();
        let mut phase = self
            .phase
            .lock()
            .map_err(|_| PersistenceError::Unavailable)?;
        let pending = match std::mem::replace(&mut *phase, Phase::Quarantined) {
            Phase::Waiting(pending)
                if pending.operation == operation
                    && pending.epoch == epoch
                    && Instant::now() < pending.deadline =>
            {
                pending
            }
            _ => {
                self.changed.notify_all();
                return Err(PersistenceError::Frozen);
            }
        };
        if let Ok(receipts) = &result {
            *phase = Phase::Ready(Arc::new(Completed {
                bridge: Arc::clone(self),
                operation,
                epoch,
                window: pending.window,
                request: pending.request,
                receipts: receipts.clone(),
                preflush: pending.preflush,
                quarantined: AtomicBool::new(false),
            }));
        }
        self.changed.notify_all();
        result
    }
}
impl Completed {
    fn revalidate(&self, scope: &FreezeScope<'_>) -> Result<()> {
        let proof = (|| {
            if self.operation != scope.operation()
                || self.epoch != scope.owner_epoch()
                || &self.bridge.root != scope.root()
                || self.quarantined.load(Ordering::Acquire)
                || Instant::now() >= scope.deadline()
            {
                return Err(Error::NotQuiescent);
            }
            self.bridge
                .owner
                .verify_binding(scope.root())
                .map_err(profile_error)?;
            self.bridge
                .owner
                .revalidate_flush(self.operation, &self.receipts)
                .map_err(profile_error)
        })();
        if proof.is_err() {
            self.quarantined.store(true, Ordering::Release);
        }
        proof
    }
}
impl Drop for Completed {
    fn drop(&mut self) {
        let valid = !self.quarantined.load(Ordering::Acquire)
            && self
                .bridge
                .owner
                .revalidate_flush(self.operation, &self.receipts)
                .is_ok()
            && self.bridge.owner.release_flush(self.operation).is_ok();
        if let Ok(mut phase) = self.bridge.phase.lock() {
            *phase = if valid {
                Phase::Released {
                    operation: self.operation,
                    epoch: self.epoch,
                    window: self.window.clone(),
                    request: self.request.clone(),
                }
            } else {
                Phase::Quarantined
            };
        }
    }
}
struct JsonParticipant {
    bridge: Arc<FlushBridge>,
    kind: Participant,
}
impl LiveParticipant for JsonParticipant {
    fn participant(&self) -> Participant {
        self.kind
    }
    fn root(&self) -> &RootBinding {
        &self.bridge.root
    }
    fn freeze(&self, scope: &FreezeScope<'_>) -> Result<Box<dyn FrozenParticipant>> {
        Ok(Box::new(JsonFrozen {
            completed: self.bridge.freeze(scope)?,
            kind: self.kind,
        }))
    }
    fn post_resume(&self, scope: &FreezeScope<'_>) -> Result<()> {
        self.bridge.post_resume(scope)
    }
}
struct JsonFrozen {
    completed: Arc<Completed>,
    kind: Participant,
}
impl FrozenParticipant for JsonFrozen {
    fn revalidate(&mut self, scope: &FreezeScope<'_>) -> Result<()> {
        self.completed.revalidate(scope)
    }
    fn with_database(
        &mut self,
        relative: &Path,
        action: &mut dyn FnMut(&rusqlite::Connection) -> Result<()>,
    ) -> Result<()> {
        #[cfg(windows)]
        if self.kind == Participant::RendererSession
            && relative == Path::new("reader-json-v1.sqlite")
        {
            return self.completed.bridge.owner.with_frozen_database(
                self.completed.operation,
                &self.completed.receipts,
                action,
            );
        }
        let _ = (relative, action);
        Err(Error::Invalid)
    }
    fn preflush_json(&mut self) -> Result<Vec<mdreader_profile::bundle::PreflushArtifact>> {
        let (json_kind, kind) = match self.kind {
            Participant::RendererSession => (
                JsonKind::Session,
                mdreader_profile::bundle::PreflushKind::Session,
            ),
            Participant::RendererSettings => (
                JsonKind::Settings,
                mdreader_profile::bundle::PreflushKind::Settings,
            ),
            _ => return Err(Error::Invalid),
        };
        let original = self
            .completed
            .preflush
            .iter()
            .find(|v| v.kind == json_kind)
            .ok_or(Error::NotQuiescent)?;
        Ok(vec![mdreader_profile::bundle::PreflushArtifact {
            kind,
            bytes: original.bytes.clone(),
        }])
    }
}
pub(super) fn profile_error(error: PersistenceError) -> Error {
    match error {
        PersistenceError::Invalid | PersistenceError::UnsupportedSchema => Error::Invalid,
        PersistenceError::TooLarge => Error::Bounds,
        PersistenceError::Conflict => Error::Conflict,
        PersistenceError::Storage => Error::Storage,
        _ => Error::NotQuiescent,
    }
}
