//! MDReader desktop shell library.
//!
//! All app logic lives here so both the desktop binary (`main.rs`) and future
//! mobile entry points (P6) can share `run()`. The tauri-specta [`Builder`] is
//! constructed once in [`specta_builder`] and reused by both the running app
//! and the `export_bindings` test, guaranteeing the committed
//! `apps/mdreader/ui/src/bindings.ts` always matches the live command surface.

mod commands;
mod engine;
mod mapping;
mod overlay;
mod search;

use commands::{WatcherState, WorkspaceState};
use tauri_specta::{collect_commands, collect_events, Builder};

/// TypeScript bindings destination, relative to this crate's manifest dir
/// (`apps/mdreader/src-tauri`). `cargo test` and `tauri dev` both run with that
/// as the working directory, so this resolves to `apps/mdreader/ui/src`.
#[cfg(any(debug_assertions, test))]
const BINDINGS_PATH: &str = "../ui/src/bindings.ts";

/// Menu id for the remapped "Close Window" item (macOS). See [`build_app_menu`].
#[cfg(target_os = "macos")]
const MENU_CLOSE_WINDOW: &str = "mdreader:close-window";

/// Builds the tauri-specta command + event registry. Single source of truth for
/// both the runtime `invoke_handler`/`mount_events` and the binding exporter.
///
/// Every command listed here is dispatched OFF the platform's UI thread —
/// `async fn`, or `#[tauri::command(async)]` for the synchronous ones. That is
/// not a style choice: a plain `#[tauri::command]` runs on the UI thread, where
/// it blocks the webview's asset loads, layer-tree commits and key events for as
/// long as it takes. See `tests::no_command_runs_on_the_ui_thread`, which
/// enforces it.
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
            search::explain_query,
            overlay::list_schemas,
            overlay::project_corpus,
            overlay::describe_schema,
            overlay::inspect_span,
            overlay::approve_fact,
            overlay::reject_fact,
            overlay::check_constraints,
            mapping::list_mappings,
            mapping::inspect_mapping,
            mapping::compose_mappings,
            mapping::diff_schema_versions,
            mapping::inspect_migration,
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

    // The updater is a direct-distribution capability, not part of normal dev,
    // E2E, store, or mobile builds. Release CI is the only caller that enables
    // `direct-updater`, after injecting a validated endpoint/public-key overlay.
    #[cfg(feature = "direct-updater")]
    {
        tauri_builder = tauri_builder.plugin(tauri_plugin_updater::Builder::new().build());
    }

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

    /// THE first-paint guarantee, enforced by construction.
    ///
    /// Tauri dispatches a command that is neither `async fn` nor marked
    /// `#[tauri::command(async)]` **on the platform's UI thread** — on macOS that
    /// is `com.apple.main-thread`, running inside the WKWebView custom-protocol
    /// handler (`wry … url_scheme_handler::start_task` → `tauri::ipc::protocol::get`
    /// → the handler). While such a command runs, the UI process serves nothing
    /// else for the webview: no asset request, no layer-tree commit, no key event.
    /// A slow one therefore does not merely delay its own answer — it freezes the
    /// window, and if it lands before the first layer-tree commit the window never
    /// paints at all (a blank frame, dead to the keyboard, at 0% CPU).
    ///
    /// The startup path invokes commands before first paint (`watch_workspace`,
    /// then `list_workspace_files`), so ANY command that can block — on I/O, on a
    /// lock, on a permission prompt, on the engine — is a first-paint hazard.
    /// Rather than audit which ones are "fast enough", every command is dispatched
    /// off the UI thread; then no backend work can ever hold first paint, whatever
    /// it waits on. A new command that forgets the flag trips this test.
    #[test]
    fn no_command_runs_on_the_ui_thread() {
        let mut checked = 0;
        for (module, source) in [
            ("commands.rs", include_str!("commands.rs")),
            ("engine.rs", include_str!("engine.rs")),
            ("search.rs", include_str!("search.rs")),
            ("overlay.rs", include_str!("overlay.rs")),
            ("mapping.rs", include_str!("mapping.rs")),
        ] {
            let file: syn::File = syn::parse_str(source).expect("module parses");
            for item in &file.items {
                let syn::Item::Fn(func) = item else { continue };
                let Some(attr) = func.attrs.iter().find(|attr| {
                    let path = &attr.path();
                    path.segments.len() == 2
                        && path.segments[0].ident == "tauri"
                        && path.segments[1].ident == "command"
                }) else {
                    continue;
                };
                checked += 1;
                let name = func.sig.ident.to_string();
                // Off the UI thread either way: `async fn` is spawned onto the
                // async runtime, and `(async)` spawns an otherwise-sync body onto
                // it too. A bare `#[tauri::command]` is the hazard.
                let spawned = func.sig.asyncness.is_some()
                    || attr
                        .meta
                        .require_list()
                        .is_ok_and(|list| list.tokens.to_string().contains("async"));
                assert!(
                    spawned,
                    "{module}: command `{name}` is dispatched on the UI thread — while it runs \
                     the window cannot paint or accept input. Declare it `async fn` or mark it \
                     `#[tauri::command(async)]`."
                );
            }
        }
        // The scan is only as good as its reach: a marker that stopped matching
        // would pass vacuously.
        assert!(
            checked >= 20,
            "only {checked} commands were scanned — the attribute marker has drifted"
        );
    }
}
