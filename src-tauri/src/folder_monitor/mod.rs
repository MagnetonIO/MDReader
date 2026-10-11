//! Saved-folder watch ownership, independent of active workspace/view epochs.
pub(crate) mod commands;
mod watcher;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
#[derive(Default)]
pub(crate) struct FolderMonitoring {
    stop: Arc<AtomicBool>,
    pub(super) unavailable: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
}
impl Drop for FolderMonitoring {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
pub(crate) fn plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    use tauri::Manager;
    tauri::plugin::Builder::new("saved-folders")
        .on_event(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                if let Some(owner) = app.try_state::<FolderMonitoring>() {
                    owner.stop.store(true, Ordering::Release);
                }
            }
        })
        .build()
}
pub(crate) fn start(app: &tauri::AppHandle) {
    use tauri::Manager;
    let owner = FolderMonitoring::default();
    let stop = owner.stop.clone();
    let unavailable = owner.unavailable.clone();
    app.manage(owner);
    #[cfg(not(mobile))]
    {
        let app = app.clone();
        std::thread::spawn(move || watcher::run(app, stop, unavailable));
    }
    #[cfg(mobile)]
    let _ = (stop, unavailable);
}
