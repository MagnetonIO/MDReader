//! MDReader desktop shell library.
//!
//! All app logic lives here so both the desktop binary (`main.rs`) and future
//! mobile entry points (P6) can share `run()`. The tauri-specta [`Builder`] is
//! constructed once in [`specta_builder`] and reused by both the running app
//! and the `export_bindings` test, guaranteeing the committed
//! `apps/mdreader/ui/src/bindings.ts` always matches the live command surface.

mod commands;
mod engine;
mod search;

use commands::{WatcherState, WorkspaceState};
use tauri_specta::{collect_commands, collect_events, Builder};

/// TypeScript bindings destination, relative to this crate's manifest dir
/// (`apps/mdreader/src-tauri`). `cargo test` and `tauri dev` both run with that
/// as the working directory, so this resolves to `apps/mdreader/ui/src`.
const BINDINGS_PATH: &str = "../ui/src/bindings.ts";

/// Menu id for the remapped "Close Window" item (macOS). See [`build_app_menu`].
#[cfg(target_os = "macos")]
const MENU_CLOSE_WINDOW: &str = "mdreader:close-window";

/// Builds the tauri-specta command + event registry. Single source of truth for
/// both the runtime `invoke_handler`/`mount_events` and the binding exporter.
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::ping,
            commands::set_workspace,
            commands::read_doc,
            commands::save_doc,
            commands::pick_workspace_folder,
            commands::watch_workspace,
            engine::list_workspace_files,
            engine::start_import,
            engine::import_status,
            engine::resolve_wikilink,
            search::search,
            search::related,
        ])
        .events(collect_events![commands::FsChanged, commands::WatchError])
}

/// Build the macOS application menu. Tauri's *default* menu binds ⌘W to the
/// native "Close Window", which would swallow the keystroke before it reaches
/// the webview — stealing the shell's "close tab" binding. This replacement
/// keeps the standard App/Edit menus (so copy/paste/select-all keep working) and
/// remaps window-close to ⌘⇧W, leaving ⌘W free to reach the webview.
#[cfg(target_os = "macos")]
fn build_app_menu(handle: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{AboutMetadata, MenuBuilder, MenuItemBuilder, SubmenuBuilder};

    let app_menu = SubmenuBuilder::new(handle, "MDReader")
        .about(Some(AboutMetadata::default()))
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let edit_menu = SubmenuBuilder::new(handle, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    // Custom "Close Window" remapped to ⌘⇧W so plain ⌘W is NOT claimed by any
    // menu item and therefore reaches the webview (the shell's tab-close chord).
    let close_window = MenuItemBuilder::new("Close Window")
        .id(MENU_CLOSE_WINDOW)
        .accelerator("CmdOrCtrl+Shift+W")
        .build(handle)?;

    let window_menu = SubmenuBuilder::new(handle, "Window")
        .minimize()
        .item(&close_window)
        .separator()
        .fullscreen()
        .build()?;

    MenuBuilder::new(handle)
        .items(&[&app_menu, &edit_menu, &window_menu])
        .build()
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

    #[allow(unused_mut)]
    let mut tauri_builder = tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .manage(WorkspaceState::default())
        .manage(WatcherState::default())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            Ok(())
        });

    // Only macOS needs the custom menu: it is the platform whose default menu
    // binds ⌘W to Close Window. Other platforms keep their (harmless) defaults.
    #[cfg(target_os = "macos")]
    {
        tauri_builder = tauri_builder
            .menu(build_app_menu)
            .on_menu_event(|app, event| {
                use tauri::Manager;
                if event.id().0.as_str() == MENU_CLOSE_WINDOW {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.close();
                    }
                }
            });
    }

    tauri_builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Writes the TypeScript bindings for the current command surface, then strips
/// trailing whitespace from every line. specta emits blank doc-comment
/// continuation lines as `\t * ` (a trailing space), which would trip
/// `git diff --check`; trimming keeps the committed bindings pristine. The pass
/// is deterministic, so drift detection is unaffected — a real command change
/// still surfaces as a diff.
#[cfg(any(debug_assertions, test))]
fn export_bindings(builder: &Builder<tauri::Wry>) -> Result<(), specta_typescript::Error> {
    builder.export(specta_typescript::Typescript::default(), BINDINGS_PATH)?;
    if let Ok(contents) = std::fs::read_to_string(BINDINGS_PATH) {
        let body = contents
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n");
        let _ = std::fs::write(BINDINGS_PATH, format!("{}\n", body.trim_end()));
    }
    Ok(())
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
