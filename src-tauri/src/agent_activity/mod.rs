//! Agent Sessions: Tauri commands, the transcript watcher and the live
//! `mdreader://agent-activity-changed` event. The service owner reads logs
//! and stores activity; this adapter only names changed logs and relays.
pub(crate) mod commands;
#[cfg(not(mobile))]
mod watch;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri_specta::Event;

/// Emitted after ingest, a hook or an MCP report changes visible activity.
/// `sessionId` is the changed session, or `null` when several (or unknown)
/// changed and the whole list should reload.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
#[tauri_specta(event_name = "mdreader://agent-activity-changed")]
pub struct AgentActivityChanged {
    pub session_id: Option<String>,
}

/// Watcher lifetime and its "state changed, rescan" signal.
#[derive(Default)]
pub(crate) struct ActivityWatch {
    stop: Arc<AtomicBool>,
    kick: Arc<AtomicBool>,
}
impl ActivityWatch {
    pub(crate) fn kick(&self) {
        self.kick.store(true, Ordering::Release);
    }
}
impl Drop for ActivityWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

pub(crate) fn plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    use tauri::Manager;
    tauri::plugin::Builder::new("agent-activity")
        .on_event(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                if let Some(owner) = app.try_state::<ActivityWatch>() {
                    owner.stop.store(true, Ordering::Release);
                }
            }
        })
        .build()
}

pub(crate) fn start(app: &tauri::AppHandle) {
    use tauri::Manager;
    let owner = ActivityWatch::default();
    // The first pass catches up on logs written while the app was closed.
    owner.kick();
    #[cfg(not(mobile))]
    {
        let (app, stop, kick) = (app.clone(), owner.stop.clone(), owner.kick.clone());
        std::thread::spawn(move || watch::run(app, stop, kick));
    }
    app.manage(owner);
}
