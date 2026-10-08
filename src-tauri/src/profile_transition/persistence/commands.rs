//! Native-only path/window selection. The issued lease moves into the blocking
//! task and outlives an abandoned renderer invoke future.
use super::{JsonKind, JsonOwner, PersistedJson, PersistenceError, RendererIdentity, WriteReceipt};
use crate::profile_transition::NativeProfileTransition;
use mdreader_profile::Participant;
use std::sync::Arc;
use tauri::{Emitter, Manager, State};

pub(crate) struct TauriFlushChannel(pub tauri::AppHandle);
impl super::bridge::FlushChannel for TauriFlushChannel {
    fn request(
        &self,
        window: &str,
        request: &super::bridge::FlushRequest,
    ) -> mdreader_profile::Result<()> {
        self.0
            .get_webview_window(window)
            .ok_or(mdreader_profile::Error::NotQuiescent)?
            .emit("mdreader://profile-preservation-flush", request)
            .map_err(|_| mdreader_profile::Error::NotQuiescent)
    }
    fn released(
        &self,
        window: &str,
        request: &super::bridge::FlushRequest,
    ) -> mdreader_profile::Result<()> {
        self.0
            .get_webview_window(window)
            .ok_or(mdreader_profile::Error::NotQuiescent)?
            .emit("mdreader://profile-preservation-released", request)
            .map_err(|_| mdreader_profile::Error::NotQuiescent)
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn native_profile_persistence_host() -> super::types::PersistenceHost {
    if cfg!(all(unix, not(mobile))) {
        super::types::PersistenceHost::NativeAtomic
    } else {
        super::types::PersistenceHost::LegacyHost
    }
}

pub(crate) struct PersistenceState {
    pub owner: Arc<JsonOwner>,
    pub transition: Arc<NativeProfileTransition>,
    pub flush: Arc<super::bridge::FlushBridge>,
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn native_profile_persistence_flush(
    window: tauri::Window,
    state: State<'_, PersistenceState>,
    submission: super::bridge::FlushSubmission,
) -> Result<[WriteReceipt; 2], PersistenceError> {
    let bridge = Arc::clone(&state.flush);
    let label = window.label().to_owned();
    tauri::async_runtime::spawn_blocking(move || bridge.submit(&label, submission))
        .await
        .map_err(|_| PersistenceError::Unavailable)?
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn native_profile_persistence_read(
    window: tauri::Window,
    state: State<'_, PersistenceState>,
    identity: RendererIdentity,
) -> Result<PersistedJson, PersistenceError> {
    let lease = state
        .transition
        .effect(Participant::RendererSession)
        .map_err(|_| PersistenceError::Frozen)?;
    let owner = Arc::clone(&state.owner);
    let label = window.label().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        owner.read_current(&label, &identity)
    })
    .await
    .map_err(|_| PersistenceError::Unavailable)?
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn native_profile_persistence_load(
    window: tauri::Window,
    state: State<'_, PersistenceState>,
) -> Result<PersistedJson, PersistenceError> {
    let lease = state
        .transition
        .effect(Participant::RendererSession)
        .map_err(|_| PersistenceError::Frozen)?;
    let owner = Arc::clone(&state.owner);
    let label = window.label().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        owner.load(&label)
    })
    .await
    .map_err(|_| PersistenceError::Unavailable)?
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn native_profile_persistence_write(
    window: tauri::Window,
    state: State<'_, PersistenceState>,
    identity: RendererIdentity,
    kind: JsonKind,
    generation: u32,
    json: String,
) -> Result<WriteReceipt, PersistenceError> {
    if json.len() > super::types::MAX_JSON_BYTES {
        return Err(PersistenceError::TooLarge);
    }
    let participant = match kind {
        JsonKind::Session => Participant::RendererSession,
        JsonKind::Settings => Participant::RendererSettings,
    };
    let lease = state
        .transition
        .effect(participant)
        .map_err(|_| PersistenceError::Frozen)?;
    let owner = Arc::clone(&state.owner);
    let label = window.label().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        owner.write(&label, &identity, kind, generation, &json)
    })
    .await
    .map_err(|_| PersistenceError::Unavailable)?
}
