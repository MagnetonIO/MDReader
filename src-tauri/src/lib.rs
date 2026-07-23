//! MDReader desktop shell library.
//!
//! All app logic lives here so both the desktop binary (`main.rs`) and future
//! mobile entry points (P6) can share `run()`. The tauri-specta [`Builder`] is
//! constructed once in [`specta_builder`] and reused by both the running app
//! and the `export_bindings` test, guaranteeing the committed
//! `apps/mdreader/ui/src/bindings.ts` always matches the live command surface.

mod commands;

use commands::WatcherState;
use tauri_specta::{collect_commands, collect_events, Builder};

/// TypeScript bindings destination, relative to this crate's manifest dir
/// (`apps/mdreader/src-tauri`). `cargo test` and `tauri dev` both run with that
/// as the working directory, so this resolves to `apps/mdreader/ui/src`.
const BINDINGS_PATH: &str = "../ui/src/bindings.ts";

/// Builds the tauri-specta command + event registry. Single source of truth for
/// both the runtime `invoke_handler`/`mount_events` and the binding exporter.
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::ping,
            commands::list_workspace_files,
            commands::read_doc,
            commands::pick_workspace_folder,
            commands::watch_workspace,
        ])
        .events(collect_events![commands::FsChanged])
}

/// Desktop (and, later, mobile) entry point.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = specta_builder();

    // In debug builds, refresh the committed bindings on every launch so local
    // `tauri dev` iterations stay in sync. CI relies on the `export_bindings`
    // test + `scripts/check-bindings.sh` to detect drift without launching.
    #[cfg(debug_assertions)]
    export_bindings(&builder).expect("failed to export typescript bindings");

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .manage(WatcherState::default())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Writes the TypeScript bindings for the current command surface.
#[cfg(any(debug_assertions, test))]
fn export_bindings(builder: &Builder<tauri::Wry>) -> Result<(), specta_typescript::Error> {
    builder.export(specta_typescript::Typescript::default(), BINDINGS_PATH)
}

#[cfg(test)]
mod tests {
    use super::{export_bindings, specta_builder};

    /// Regenerates `apps/mdreader/ui/src/bindings.ts` headlessly (no window, no
    /// webview). `scripts/check-bindings.sh` runs this then `git diff
    /// --exit-code` to fail CI on drift.
    #[test]
    fn export_bindings_up_to_date() {
        export_bindings(&specta_builder()).expect("failed to export typescript bindings");
    }
}
