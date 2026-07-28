//! MDReader desktop shell library.
//!
//! All app logic lives here so both the desktop binary (`main.rs`) and future
//! mobile entry points (P6) can share `run()`. The tauri-specta [`Builder`] is
//! constructed once in [`specta_builder`] and reused by both the running app
//! and the `export_bindings` test, guaranteeing the committed
//! `apps/mdreader/ui/src/bindings.ts` always matches the live command surface.

#[cfg(all(feature = "direct-updater", feature = "e2e-build"))]
compile_error!("the direct updater cannot be compiled into the isolated E2E application");

#[cfg(feature = "e2e-build")]
#[used]
static E2E_BUILD_MANIFEST: &str = concat!(
    "MDREADER_E2E_BUILD_MANIFEST:",
    "{\"schema\":1,\"source_commit\":\"",
    env!("MDREADER_SOURCE_COMMIT"),
    "\",\"bundle_identifier\":\"dev.mdreader.app.e2e\",\"profile\":\"e2e-build\"}",
    ":MDREADER_E2E_BUILD_MANIFEST_END",
);

mod cloud_accounts;
mod commands;
mod diagnostics;
mod engine;
mod federation;
mod file_lock;
mod library;
mod library_query;
mod mapping;
mod mobile_enterprise;
mod overlay;
mod search;
mod updater;

use commands::{WatcherState, WorkspaceState};
use diagnostics::DiagnosticsState;
use library::LibraryState;
use tauri::Manager;
#[cfg(not(mobile))]
use tauri_specta::collect_events;
use tauri_specta::{collect_commands, Builder};

/// TypeScript bindings destination, relative to this crate's manifest dir
/// (`apps/mdreader/src-tauri`). `cargo test` and `tauri dev` both run with that
/// as the working directory, so this resolves to `apps/mdreader/ui/src`.
#[cfg(any(debug_assertions, test))]
const BINDINGS_PATH: &str = "../ui/src/bindings.ts";

/// Menu id for the remapped "Close Window" item (macOS). See [`build_app_menu`].
#[cfg(target_os = "macos")]
const MENU_CLOSE_WINDOW: &str = "mdreader:close-window";

/// Menu id for the native macOS Settings item.
#[cfg(target_os = "macos")]
const MENU_SETTINGS: &str = "mdreader:settings";

/// Static renderer event used by the native Settings menu item.
#[cfg(target_os = "macos")]
const SHOW_SETTINGS_SCRIPT: &str = "window.dispatchEvent(new Event('mdreader:show-settings'))";

/// Builds the tauri-specta command + event registry. Single source of truth for
/// both the runtime `invoke_handler`/`mount_events` and the binding exporter.
///
/// Every command listed here is dispatched OFF the platform's UI thread —
/// `async fn`, or `#[tauri::command(async)]` for the synchronous ones. That is
/// not a style choice: a plain `#[tauri::command]` runs on the UI thread, where
/// it blocks the webview's asset loads, layer-tree commits and key events for as
/// long as it takes. See `tests::no_command_runs_on_the_ui_thread`, which
/// enforces it.
#[cfg(not(mobile))]
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::ping,
            commands::read_doc,
            commands::save_doc,
            commands::pick_workspace_folder,
            cloud_accounts::list_cloud_sources,
            cloud_accounts::connect_cloud_source,
            cloud_accounts::set_cloud_source_enabled,
            cloud_accounts::disconnect_cloud_source,
            cloud_accounts::prepare_cloud_catalog,
            cloud_accounts::entity_query,
            cloud_accounts::query_library_documents,
            cloud_accounts::query_cloud_documents,
            cloud_accounts::cancel_cloud_document_query,
            mobile_enterprise::connected_catdb_state,
            mobile_enterprise::enroll_connected_catdb,
            mobile_enterprise::set_connected_catdb_source_enabled,
            mobile_enterprise::disconnect_connected_catdb,
            mobile_enterprise::connected_catdb_entity_query,
            mobile_enterprise::mobile_enterprise_connection,
            mobile_enterprise::mobile_enterprise_request,
            library::list_library_sources,
            library::pick_and_add_library_source,
            library::reconcile_library_restore,
            library::watch_library_source,
            library::grant_library_source,
            library::resolve_library_document_edit,
            library::mark_library_source_active,
            library::forget_library_source,
            diagnostics::diagnostics_info,
            diagnostics::set_diagnostics_level,
            diagnostics::record_diagnostic_event,
            diagnostics::reveal_diagnostics,
            diagnostics::export_diagnostics,
            diagnostics::clear_diagnostics,
            engine::list_workspace_files,
            engine::start_import,
            engine::import_status,
            engine::resolve_wikilink,
            search::search,
            search::related,
            search::explain_query,
            federation::query_federated_document,
            federation::cancel_federated_document,
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
            updater::updater_available,
            updater::check_for_update,
            updater::install_update,
        ])
        .events(collect_events![commands::FsChanged, commands::WatchError])
}

/// The mobile webview can inspect semantic state but cannot invoke document,
/// approval, local-source, or updater mutations.
#[cfg(mobile)]
fn mobile_specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new().commands(collect_commands![
        commands::ping,
        cloud_accounts::list_cloud_sources,
        cloud_accounts::query_cloud_documents,
        mobile_enterprise::connected_catdb_state,
        mobile_enterprise::enroll_connected_catdb,
        mobile_enterprise::set_connected_catdb_source_enabled,
        mobile_enterprise::disconnect_connected_catdb,
        mobile_enterprise::connected_catdb_entity_query,
        mobile_enterprise::mobile_enterprise_connection,
        mobile_enterprise::mobile_enterprise_request,
        federation::query_federated_document,
        federation::cancel_federated_document,
        overlay::list_schemas,
        overlay::project_corpus,
        overlay::describe_schema,
        overlay::inspect_span,
        overlay::check_constraints,
        mapping::list_mappings,
        mapping::inspect_mapping,
        mapping::compose_mappings,
        mapping::diff_schema_versions,
        mapping::inspect_migration,
    ])
}

#[cfg(mobile)]
fn specta_builder() -> Builder<tauri::Wry> {
    mobile_specta_builder()
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuntimeTarget {
    Desktop,
    Mobile,
}

#[cfg(debug_assertions)]
impl RuntimeTarget {
    const fn current() -> Self {
        if cfg!(mobile) {
            Self::Mobile
        } else {
            Self::Desktop
        }
    }
}

#[cfg(debug_assertions)]
fn runtime_specta_builder(
    target: RuntimeTarget,
    export: impl FnOnce(&Builder<tauri::Wry>),
) -> Builder<tauri::Wry> {
    let builder = specta_builder();
    if target == RuntimeTarget::Desktop {
        export(&builder);
    }
    builder
}

/// Build the macOS application menu. Tauri's *default* menu binds ⌘W to the
/// native "Close Window", which would swallow the keystroke before it reaches
/// the webview — stealing the shell's "close tab" binding. This replacement
/// keeps the standard App/Edit menus (so copy/paste/select-all keep working) and
/// remaps window-close to ⌘⇧W, leaving ⌘W free to reach the webview.
#[cfg(target_os = "macos")]
fn build_app_menu(handle: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{AboutMetadata, MenuBuilder, MenuItemBuilder, SubmenuBuilder};

    let settings = MenuItemBuilder::new("Settings…")
        .id(MENU_SETTINGS)
        .accelerator("CmdOrCtrl+,")
        .build(handle)?;

    let app_menu = SubmenuBuilder::new(handle, "MDReader")
        .about(Some(AboutMetadata::default()))
        .separator()
        .item(&settings)
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
    #[cfg(debug_assertions)]
    let builder = runtime_specta_builder(RuntimeTarget::current(), |builder| {
        // Desktop `tauri dev` refreshes the committed bindings on launch.
        // Packaged mobile runtimes cannot and must not write source artifacts.
        export_bindings(builder).expect("failed to export typescript bindings");
    });
    #[cfg(not(debug_assertions))]
    let builder = specta_builder();

    #[allow(unused_mut)]
    let mut tauri_builder = tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .manage(WorkspaceState::default())
        .manage(federation::FederationRuntime::default())
        .manage(WatcherState::default())
        .manage(LibraryState::default())
        .manage(cloud_accounts::CloudAccountsState::default())
        .manage(mobile_enterprise::MobileEnterpriseState::default())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            app.state::<cloud_accounts::CloudAccountsState>()
                .configure_catalog(app.handle())?;
            let diagnostics =
                DiagnosticsState::new(diagnostics::diagnostics_directory(app.handle())?);
            app.manage(diagnostics);
            builder.mount_events(app);
            Ok(())
        });

    // The updater is a direct-distribution capability, not part of normal dev,
    // E2E, store, or mobile builds. Release CI is the only caller that enables
    // `direct-updater`, after injecting a validated endpoint/public-key overlay.
    #[cfg(all(feature = "direct-updater", not(mobile)))]
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
                if let Some(window) = app.get_webview_window("main") {
                    match event.id().0.as_str() {
                        MENU_SETTINGS => {
                            let _ = window.eval(SHOW_SETTINGS_SCRIPT);
                        }
                        MENU_CLOSE_WINDOW => {
                            let _ = window.close();
                        }
                        _ => {}
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
    use std::cell::Cell;

    use super::{export_bindings, runtime_specta_builder, specta_builder, RuntimeTarget};

    /// Regenerates `apps/mdreader/ui/src/bindings.ts` headlessly (no window, no
    /// webview). `scripts/check-bindings.sh` runs this then `git diff
    /// --exit-code` to fail CI on drift.
    #[test]
    fn export_bindings_up_to_date() {
        export_bindings(&specta_builder()).expect("failed to export typescript bindings");
    }

    #[test]
    fn mobile_runtime_skips_binding_export_while_desktop_debug_exports() {
        let export_called = Cell::new(false);
        let _mobile_builder = runtime_specta_builder(RuntimeTarget::Mobile, |_| {
            export_called.set(true);
        });
        assert!(
            !export_called.get(),
            "mobile runtime attempted to write generated bindings"
        );

        let _desktop_builder = runtime_specta_builder(RuntimeTarget::Desktop, |_| {
            export_called.set(true);
        });
        assert!(
            export_called.get(),
            "desktop debug startup no longer refreshes generated bindings"
        );
    }

    #[test]
    fn raw_path_grant_commands_are_neither_registered_nor_bound() {
        let registry = include_str!("lib.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production registry");
        assert!(!registry.contains("commands::set_workspace"));
        assert!(!registry.contains("commands::watch_workspace,"));

        let bindings = include_str!("../../ui/src/bindings.ts");
        assert!(!bindings.contains("__TAURI_INVOKE(\"set_workspace\""));
        assert!(!bindings.contains("__TAURI_INVOKE(\"watch_workspace\""));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_application_menu_routes_settings_to_the_renderer() {
        let production = include_str!("lib.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production module");
        assert!(production.contains("MENU_SETTINGS"));
        assert!(production.contains(".accelerator(\"CmdOrCtrl+,\")"));
        assert!(production.contains("mdreader:show-settings"));
    }

    #[test]
    fn federated_document_query_is_a_registered_specta_command() {
        let registry = include_str!("lib.rs");
        assert!(
            registry.contains("federation::query_federated_document,"),
            "the P8 federation must cross the real Tauri/Specta command boundary"
        );
    }

    #[test]
    fn cloud_account_commands_are_registered_and_generated_for_the_renderer() {
        let registry = include_str!("lib.rs");
        let bindings = include_str!("../../ui/src/bindings.ts");
        for (rust_command, invoke_name) in [
            (
                "cloud_accounts::list_cloud_sources,",
                "\"list_cloud_sources\"",
            ),
            (
                "cloud_accounts::connect_cloud_source,",
                "\"connect_cloud_source\"",
            ),
            (
                "cloud_accounts::set_cloud_source_enabled,",
                "\"set_cloud_source_enabled\"",
            ),
            (
                "cloud_accounts::disconnect_cloud_source,",
                "\"disconnect_cloud_source\"",
            ),
            (
                "cloud_accounts::prepare_cloud_catalog,",
                "\"prepare_cloud_catalog\"",
            ),
            (
                "cloud_accounts::query_cloud_documents,",
                "\"query_cloud_documents\"",
            ),
            (
                "cloud_accounts::cancel_cloud_document_query,",
                "\"cancel_cloud_document_query\"",
            ),
        ] {
            assert!(registry.contains(rust_command), "missing {rust_command}");
            assert!(bindings.contains(invoke_name), "missing {invoke_name}");
        }
    }

    #[test]
    fn mobile_enterprise_transport_is_registered_without_a_browser_credential_header() {
        let registry = include_str!("lib.rs");
        let bindings = include_str!("../../ui/src/bindings.ts");
        let adapter = include_str!("../../ui/src/mobile/mobile-source-adapter.ts");
        for (rust_command, invoke_name) in [
            (
                "mobile_enterprise::connected_catdb_state,",
                "\"connected_catdb_state\"",
            ),
            (
                "mobile_enterprise::enroll_connected_catdb,",
                "\"enroll_connected_catdb\"",
            ),
            (
                "mobile_enterprise::set_connected_catdb_source_enabled,",
                "\"set_connected_catdb_source_enabled\"",
            ),
            (
                "mobile_enterprise::disconnect_connected_catdb,",
                "\"disconnect_connected_catdb\"",
            ),
            (
                "mobile_enterprise::connected_catdb_entity_query,",
                "\"connected_catdb_entity_query\"",
            ),
            (
                "mobile_enterprise::mobile_enterprise_connection,",
                "\"mobile_enterprise_connection\"",
            ),
            (
                "mobile_enterprise::mobile_enterprise_request,",
                "\"mobile_enterprise_request\"",
            ),
        ] {
            assert!(registry.contains(rust_command), "missing {rust_command}");
            assert!(bindings.contains(invoke_name), "missing {invoke_name}");
        }
        assert!(!adapter.contains("X-MDReader-Session-Reference"));
        assert!(!adapter.contains("Authorization"));
        assert!(!adapter.contains("fetch.bind(globalThis)"));
    }

    #[test]
    fn shared_entity_query_is_registered_and_generated_for_the_renderer() {
        let registry = include_str!("lib.rs");
        let bindings = include_str!("../../ui/src/bindings.ts");
        assert!(
            registry.contains("cloud_accounts::entity_query,"),
            "cloud P8 must cross the shared entity-query command boundary"
        );
        assert!(
            bindings.contains("\"entity_query\""),
            "the shared entity-query command must be generated for the renderer"
        );
        assert!(bindings.contains("EntityQueryRequest"));
        assert!(bindings.contains("EntityQueryResponse"));
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
    /// The startup path invokes commands before first paint (`watch_library_source`,
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
            ("cloud_accounts.rs", include_str!("cloud_accounts.rs")),
            ("diagnostics.rs", include_str!("diagnostics.rs")),
            ("engine.rs", include_str!("engine.rs")),
            ("library.rs", include_str!("library.rs")),
            ("search.rs", include_str!("search.rs")),
            ("federation.rs", include_str!("federation.rs")),
            ("overlay.rs", include_str!("overlay.rs")),
            ("mapping.rs", include_str!("mapping.rs")),
            ("mobile_enterprise.rs", include_str!("mobile_enterprise.rs")),
            ("updater.rs", include_str!("updater.rs")),
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
