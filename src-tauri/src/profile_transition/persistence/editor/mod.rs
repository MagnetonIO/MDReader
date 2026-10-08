//! Exact renderer/window-bound unsaved buffer owner. Only an actual held and
//! drained editor adapter may answer the fresh native request. Archival bodies
//! never authorize filesystem writes and are not installed into the profile.
mod participant;
mod types;
use super::{JsonOwner, PersistenceError, bridge::FlushRequest};
use crate::profile_transition::{FreezeScope, LiveParticipant};
use mdreader_profile::{Error, Result, RootBinding};
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, Condvar, Mutex},
    time::Instant,
};
use tauri::{Emitter, Manager, State};
pub(crate) use types::{EditorReceipt, EditorSubmission};
use uuid::Uuid;

pub(crate) struct EditorOwner {
    json: Arc<JsonOwner>,
    root: RootBinding,
    app: tauri::AppHandle,
    phase: Mutex<Phase>,
    changed: Condvar,
}
struct Pending {
    operation: Uuid,
    epoch: Uuid,
    deadline: Instant,
    window: String,
    request: FlushRequest,
    archive: Option<Arc<Vec<u8>>>,
}
enum Phase {
    Idle,
    Held(Pending),
    Released(Pending),
    Quarantined,
}
impl EditorOwner {
    pub(crate) fn new(json: Arc<JsonOwner>, root: RootBinding, app: tauri::AppHandle) -> Arc<Self> {
        Arc::new(Self {
            json,
            root,
            app,
            phase: Mutex::new(Phase::Idle),
            changed: Condvar::new(),
        })
    }
    pub(crate) fn participant(self: &Arc<Self>) -> Arc<dyn LiveParticipant> {
        Arc::new(participant::ParticipantOwner(Arc::clone(self)))
    }
    fn emit(&self, window: &str, event: &str, request: &FlushRequest) -> Result<()> {
        self.app
            .get_webview_window(window)
            .ok_or(Error::NotQuiescent)?
            .emit(event, request)
            .map_err(|_| Error::NotQuiescent)
    }
    fn verify(&self, pending: &Pending, scope: &FreezeScope<'_>) -> Result<()> {
        self.json
            .verify_binding(scope.root())
            .map_err(|_| Error::NotQuiescent)?;
        let (window, identity) = self.json.renderer().map_err(|_| Error::NotQuiescent)?;
        if self.root != *scope.root()
            || pending.operation != scope.operation()
            || pending.epoch != scope.owner_epoch()
            || pending.deadline != scope.deadline()
            || Instant::now() >= pending.deadline
            || window != pending.window
            || identity.owner_epoch != pending.request.identity.owner_epoch
            || identity.renderer_epoch != pending.request.identity.renderer_epoch
        {
            return Err(Error::NotQuiescent);
        }
        Ok(())
    }
    fn freeze(&self, scope: &FreezeScope<'_>) -> Result<Arc<Vec<u8>>> {
        self.json
            .verify_binding(scope.root())
            .map_err(|_| Error::NotQuiescent)?;
        let (window, identity) = self.json.renderer().map_err(|_| Error::NotQuiescent)?;
        let request = FlushRequest {
            request_id: Uuid::new_v4().to_string(),
            identity,
        };
        {
            let mut phase = self.phase.lock().map_err(|_| Error::NotQuiescent)?;
            if !matches!(*phase, Phase::Idle) {
                return Err(Error::NotQuiescent);
            }
            *phase = Phase::Held(Pending {
                operation: scope.operation(),
                epoch: scope.owner_epoch(),
                deadline: scope.deadline(),
                window: window.clone(),
                request: request.clone(),
                archive: None,
            });
        }
        self.emit(&window, "mdreader://profile-editor-freeze", &request)?;
        let mut phase = self.phase.lock().map_err(|_| Error::NotQuiescent)?;
        loop {
            let Phase::Held(pending) = &*phase else {
                return Err(Error::NotQuiescent);
            };
            self.verify(pending, scope)?;
            if let Some(bytes) = &pending.archive {
                return Ok(Arc::clone(bytes));
            }
            let remaining = scope
                .deadline()
                .checked_duration_since(Instant::now())
                .ok_or(Error::NotQuiescent)?;
            phase = self
                .changed
                .wait_timeout(phase, remaining)
                .map_err(|_| Error::NotQuiescent)?
                .0;
        }
    }
    fn submit(
        &self,
        window: &str,
        submission: EditorSubmission,
    ) -> std::result::Result<EditorReceipt, PersistenceError> {
        // Bound and validate before acquiring the owner state. No file or
        // SQLite lock is held through encoding/hashing these explicit bodies.
        let bytes = submission.archive()?;
        let receipt = EditorReceipt {
            request_id: submission.request.request_id.clone(),
            identity: submission.request.identity.clone(),
            generation: submission.generation.clone(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            entry_count: submission.entries.len() as u32,
        };
        let mut phase = self
            .phase
            .lock()
            .map_err(|_| PersistenceError::Unavailable)?;
        let Phase::Held(pending) = &mut *phase else {
            return Err(PersistenceError::Conflict);
        };
        if pending.archive.is_some()
            || pending.window != window
            || Instant::now() >= pending.deadline
            || pending.request.request_id != submission.request.request_id
            || pending.request.identity.owner_epoch != submission.request.identity.owner_epoch
            || pending.request.identity.renderer_epoch != submission.request.identity.renderer_epoch
        {
            return Err(PersistenceError::Conflict);
        }
        self.json.verify_binding(&self.root)?;
        let (current_window, current_identity) = self.json.renderer()?;
        if current_window != window
            || current_identity.owner_epoch != submission.request.identity.owner_epoch
            || current_identity.renderer_epoch != submission.request.identity.renderer_epoch
        {
            return Err(PersistenceError::Conflict);
        }
        pending.archive = Some(Arc::new(bytes));
        self.changed.notify_all();
        Ok(receipt)
    }
}
#[tauri::command]
#[specta::specta]
pub(crate) async fn native_profile_editor_submit(
    window: tauri::Window,
    state: State<'_, super::commands::PersistenceState>,
    submission: EditorSubmission,
) -> std::result::Result<EditorReceipt, PersistenceError> {
    let owner = state.editor.clone().ok_or(PersistenceError::Unavailable)?;
    let label = window.label().to_owned();
    tauri::async_runtime::spawn_blocking(move || owner.submit(&label, submission))
        .await
        .map_err(|_| PersistenceError::Unavailable)?
}
