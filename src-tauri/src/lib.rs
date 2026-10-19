//! MDReader desktop shell library.
//!
//! All app logic lives here so both the desktop binary (`main.rs`) and future
//! mobile entry points (P6) can share `run()`. The tauri-specta [`Builder`] is
//! constructed once in [`specta_builder`] and reused by both the running app
//! and the headless `export_bindings_up_to_date` test, guaranteeing the committed
//! `ui/src/bindings.ts` always matches the live command surface.

#[cfg(all(feature = "direct-updater", feature = "e2e-build"))]
compile_error!("the direct updater cannot be compiled into the isolated E2E application");

// Mobile targets link this shared library directly and never link the desktop
// `main.rs`. Keep the production identity here so IPA/AAB verification can bind
// the native bytes to the exact reviewed source in the same way desktop
// packaging does.
#[used]
static MDREADER_SHARED_RELEASE_IDENTITY: &str = concat!(
    "MDREADER_RELEASE_IDENTITY_V1\0",
    "productName=",
    env!("MDREADER_NATIVE_PRODUCT_NAME"),
    "\0bundleIdentifier=",
    env!("MDREADER_NATIVE_APP_ID"),
    "\0",
    "sourceCommit=",
    env!("MDREADER_RELEASE_SOURCE_COMMIT"),
    "\0",
);

fn release_identity() -> &'static str {
    MDREADER_SHARED_RELEASE_IDENTITY
}

#[cfg(feature = "e2e-build")]
#[used]
static E2E_BUILD_MANIFEST: &str = concat!(
    "MDREADER_E2E_BUILD_MANIFEST:",
    "{\"schema\":1,\"source_commit\":\"",
    env!("MDREADER_SOURCE_COMMIT"),
    "\",\"bundle_identifier\":\"dev.mdreader.app.e2e\",\"profile\":\"e2e-build\"}",
    ":MDREADER_E2E_BUILD_MANIFEST_END",
);

#[cfg(not(mobile))]
mod agent_bootstrap;
#[cfg(not(mobile))]
mod agent_cli;
mod agent_session_hook;
mod cloud_accounts;
mod commands;
mod diagnostics;
mod editor_drafts;
mod engine;
mod file_lock;
mod folder_monitor;
mod library;
#[cfg(mobile)]
mod mobile_accounts;
mod mobile_assets;
mod mobile_assets_commands;
mod mobile_incoming;
mod mobile_provider_state;
mod mobile_reader;
mod mobile_search;
mod mobile_transfer;
#[cfg(not(mobile))]
mod native_browser;
mod native_providers;
#[cfg(not(mobile))]
mod native_sync;
mod profile_transition;
mod provider_oauth;
mod reader;
mod reader_accounts;
mod reader_agent_projects;
#[cfg(not(mobile))]
mod reader_agent_setup;
mod reader_agents;
mod reader_related;
#[cfg(not(mobile))]
pub use agent_cli::run as run_agent_cli;
mod reader_assets;
mod reader_backfill;
#[cfg(not(mobile))]
mod reader_catalog;
#[cfg(not(mobile))]
mod reader_device;
#[cfg(not(mobile))]
mod reader_export;
mod reader_external;
mod reader_owned;
mod reader_profile;
#[cfg(not(mobile))]
mod reader_reviews;
mod reader_workload;
mod search;
mod updater;

#[cfg(not(mobile))]
pub fn serve_reader_service(
    directory: &std::path::Path,
    options: mdreader_service::ServiceOptions,
) -> mdreader_core::Result<()> {
    native_providers::serve(directory, options)
}

use commands::{WatcherState, WorkspaceState};
use diagnostics::DiagnosticsState;
use library::LibraryState;
use tauri::Manager;
#[cfg(not(mobile))]
use tauri_specta::collect_events;
use tauri_specta::{collect_commands, Builder};

/// Menu id for the remapped "Close Window" item (macOS). See [`build_app_menu`].
#[cfg(target_os = "macos")]
const MENU_CLOSE_WINDOW: &str = "mdreader:close-window";

/// Menu id for the standard macOS Settings item.
#[cfg(target_os = "macos")]
const MENU_SETTINGS: &str = "mdreader:settings";

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
            reader_profile::native_profile_startup,
            profile_transition::persistence::commands::native_profile_persistence_host,
            profile_transition::persistence::commands::native_profile_persistence_load,
            profile_transition::persistence::commands::native_profile_persistence_read,
            profile_transition::persistence::commands::native_profile_persistence_write,
            profile_transition::persistence::commands::native_profile_persistence_flush,
            profile_transition::persistence::editor::native_profile_editor_submit,
            profile_transition::backup::native_profile_backup,
            profile_transition::offline::commands::native_profile_migration,
            profile_transition::offline::imported_history::native_profile_imported_history,
            reader_reviews::reader_archived_reviews,
            reader_catalog::reader_catalog,
            reader_accounts::reader_account,
            reader_external::reader_open_external,
            reader_device::reader_device,
            reader_export::reader_export_document,
            reader_export::draft::reader_export_draft,
            native_providers::native_provider_status,
            native_providers::native_provider_connect,
            native_providers::native_provider_reconnect,
            native_providers::native_provider_reconcile,
            native_providers::native_provider_request,
            reader_workload::reader_embedding_policy,
            reader_workload::set_reader_embedding_policy,
            mobile_reader::mobile_reader_list,
            mobile_reader::mobile_reader_discard_import,
            mobile_reader::mobile_reader_import,
            mobile_incoming::mobile_reader_import_incoming,
            mobile_reader::mobile_reader_read,
            mobile_reader::mobile_reader_search,
            mobile_search::mobile_search_settings,
            mobile_search::mobile_search_set_enabled,
            mobile_search::mobile_search_query,
            mobile_reader::mobile_reader_rebuild,
            mobile_reader::mobile_reader_pin,
            mobile_reader::mobile_reader_position,
            mobile_assets_commands::mobile_reader_assets,
            mobile_transfer::mobile_reader_transfer,
            mobile_transfer::provider::mobile_provider_transfer,
            mobile_provider_state::mobile_provider_reading_state,
            mobile_transfer::mobile_reader_transfer_capabilities,
            mobile_transfer::mobile_reader_take_reading_links,
            commands::read_doc,
            reader_assets::reader_workspace_asset,
            commands::save_doc,
            commands::pick_workspace_folder,
            folder_monitor::commands::folder_indexing_state,
            folder_monitor::commands::set_folder_monitoring,
            folder_monitor::commands::pick_broad_library_source,
            library::list_library_sources,
            library::pick_and_add_library_source,
            library::agent_plans_source_status,
            library::enable_agent_plans_source,
            library::disable_agent_plans_source,
            library::reconcile_library_restore,
            library::watch_library_source,
            library::grant_library_source,
            library::resolve_library_document_edit,
            library::mark_library_source_active,
            library::forget_library_source,
            library::close_and_forget_library_source,
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
            reader_related::reader_related_request,
            reader_agent_projects::agent_projects_list,
            reader_agent_projects::agent_project_sessions,
            reader_agent_projects::agent_project_documents,
            reader_agents::agent_capture_state,
            reader_agents::set_agent_capture,
            reader_backfill::agent_capture_backfill,
            reader_agents::agent_integrations_state,
            reader_agents::prepare_agent_integration,
            reader_agents::apply_agent_integration,
            reader_agents::uninstall_agent_integration,
            editor_drafts::native_editor_drafts,
            reader_agent_setup::agent_setup_state,
            reader_agent_setup::agent_setup_install,
            reader_agent_setup::agent_setup_set_mcp,
            reader_agent_setup::agent_setup_set_permissions,
            reader_agent_setup::agent_setup_uninstall,
            updater::updater_available,
            updater::check_for_update,
            updater::install_update,
        ])
        .events(collect_events![
            commands::ShowSettings,
            commands::FsChanged,
            commands::WatchError
        ])
}

/// Mobile owns explicit private document/asset copies and reading state. It
/// cannot mutate provider originals, legacy approvals, source folders or updates.
#[cfg(mobile)]
fn mobile_specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new().commands(collect_commands![
        reader_accounts::reader_account,
        reader_external::reader_open_external,
        native_providers::native_provider_status,
        native_providers::native_provider_connect,
        native_providers::native_provider_reconnect,
        native_providers::native_provider_reconcile,
        native_providers::native_provider_request,
        commands::ping,
        profile_transition::persistence::commands::native_profile_persistence_host,
        mobile_reader::mobile_reader_list,
        mobile_reader::mobile_reader_discard_import,
        mobile_reader::mobile_reader_import,
        mobile_incoming::mobile_reader_import_incoming,
        mobile_reader::mobile_reader_read,
        mobile_reader::mobile_reader_search,
        mobile_search::mobile_search_settings,
        mobile_search::mobile_search_set_enabled,
        mobile_search::mobile_search_query,
        mobile_reader::mobile_reader_rebuild,
        mobile_reader::mobile_reader_pin,
        mobile_reader::mobile_reader_position,
        mobile_assets_commands::mobile_reader_assets,
        mobile_transfer::mobile_reader_transfer,
        mobile_transfer::provider::mobile_provider_transfer,
        mobile_provider_state::mobile_provider_reading_state,
        mobile_transfer::mobile_reader_transfer_capabilities,
        mobile_transfer::mobile_reader_take_reading_links,
    ])
}

#[cfg(mobile)]
fn specta_builder() -> Builder<tauri::Wry> {
    mobile_specta_builder()
}

fn runtime_specta_builder() -> Builder<tauri::Wry> {
    specta_builder()
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

/// Construct every native owner. Failures are reported by the caller, which
/// never returns an error to Tauri.
fn configure_app(
    app: &mut tauri::App,
    builder: &tauri_specta::Builder<tauri::Wry>,
    profile: Option<&std::path::Path>,
    selection: Option<&reader_profile::SelectionLaunch>,
    controller: Option<&std::path::Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(not(mobile))]
    app.manage(profile_transition::offline::commands::MigrationState::default());

    #[cfg(not(mobile))]
    app.manage(reader_export::CopyRuntime::default());
    reader_profile::configure(app.handle(), profile, selection, controller)
        .map_err(std::io::Error::other)?;
    let reader_backend = reader::ReaderBackend::open(
        reader_profile::library_directory(app.handle()).map_err(std::io::Error::other)?,
    )?;
    match native_providers::NativeProviders::new(app.handle(), reader_backend.clone()) {
        Ok(providers) => {
            app.manage(std::sync::Arc::new(providers));
        }
        Err(error) => eprintln!("Cloud source controls unavailable: {error}"),
    }
    #[cfg(mobile)]
    if mobile_accounts::configure(app.handle(), &reader_backend).is_err() {
        eprintln!("Mobile account owner is unavailable; local reading remains available");
    }
    app.manage(reader_backend);
    folder_monitor::start(app.handle());
    #[cfg(all(windows, not(mobile)))]
    {
        let directory = reader_profile::directory(app.handle()).map_err(std::io::Error::other)?;
        profile_transition::persistence::windows::configure(app.handle(), &directory)
            .map_err(|_| std::io::Error::other("Native preferences owner is unavailable"))?;
    }
    #[cfg(all(unix, not(mobile)))]
    {
        let directory = reader_profile::directory(app.handle()).map_err(std::io::Error::other)?;
        std::fs::create_dir_all(&directory)?;
        let directory = directory.canonicalize()?;
        profile_transition::startup::configure(app.handle(), &directory)
            .map_err(|_| std::io::Error::other("Native preferences owner is unavailable"))?;
    }
    #[cfg(all(feature = "e2e-build", not(mobile)))]
    {
        library::seed_e2e_local_membership(app.handle())?;
    }
    let diagnostics = DiagnosticsState::new(diagnostics::diagnostics_directory(app.handle())?);
    app.manage(diagnostics);
    builder.mount_events(app);
    Ok(())
}

/// Show the startup error natively, log it, and exit 1 once dismissed. A
/// headless agent setup gets the error in its result file instead.
fn report_setup_failure(app: &tauri::AppHandle, message: &str) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    eprintln!("MDReader could not start: {message}");
    diagnostics::record_startup_failure(app);
    #[cfg(not(mobile))]
    if agent_bootstrap::report_startup_failure(&std::env::args().collect::<Vec<_>>(), message) {
        std::process::exit(1);
    }
    for window in app.webview_windows().values() {
        let _ = window.hide();
    }
    // The dialog runs on the main thread after setup returns; blocking here
    // would deadlock the launch callback.
    app.dialog()
        .message(message)
        .title("MDReader could not start")
        .kind(MessageDialogKind::Error)
        .show(|_| std::process::exit(1));
}

fn launch_application(terminal_runner: impl FnOnce(tauri::Builder<tauri::Wry>)) {
    launch_application_with_profile(None, terminal_runner);
}
fn launch_application_with_profile(
    profile: Option<std::path::PathBuf>,
    terminal_runner: impl FnOnce(tauri::Builder<tauri::Wry>),
) {
    launch_application_with_selection(profile, None, terminal_runner);
}
fn launch_application_with_selection(
    profile: Option<std::path::PathBuf>,
    selection: Option<reader_profile::SelectionLaunch>,
    terminal_runner: impl FnOnce(tauri::Builder<tauri::Wry>),
) {
    launch_application_with_controller(profile, selection, None, terminal_runner);
}
fn launch_application_with_controller(
    profile: Option<std::path::PathBuf>,
    selection: Option<reader_profile::SelectionLaunch>,
    controller: Option<std::path::PathBuf>,
    terminal_runner: impl FnOnce(tauri::Builder<tauri::Wry>),
) {
    std::hint::black_box(release_identity());
    let builder = runtime_specta_builder();

    // Internal profile relaunches overlap their predecessor by design; only an
    // ordinary launch defers to a running copy.
    #[cfg(all(not(mobile), not(feature = "e2e-build")))]
    let single_instance = profile.is_none() && selection.is_none() && controller.is_none();
    #[allow(unused_mut)]
    let mut tauri_builder = tauri::Builder::default();
    // Registered first: a second copy forwards its argv and exits before any
    // other plugin or setup contends for the running copy's library owners.
    #[cfg(all(not(mobile), not(feature = "e2e-build")))]
    if single_instance {
        tauri_builder = tauri_builder.plugin(tauri_plugin_single_instance::init(
            agent_bootstrap::on_second_instance,
        ));
    }
    #[allow(unused_mut)]
    let mut tauri_builder = tauri_builder
        .plugin(folder_monitor::plugin())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(mdreader_mobile_transfer::init())
        .manage(WorkspaceState::default())
        .manage(WatcherState::default())
        .manage(LibraryState::default())
        .manage(mobile_assets::AssetRuntime::default())
        .manage(mobile_transfer::TransferRuntime::default())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            let started = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                configure_app(
                    app,
                    &builder,
                    profile.as_deref(),
                    selection.as_ref(),
                    controller.as_deref(),
                )
            }));
            let failure = match started {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error.to_string()),
                Err(_) => Some("MDReader could not start".to_string()),
            };
            match failure {
                Some(message) => report_setup_failure(app.handle(), &message),
                #[cfg(not(mobile))]
                None => {
                    agent_bootstrap::handle(app.handle(), &std::env::args().collect::<Vec<_>>())
                }
                #[cfg(mobile)]
                None => {}
            }
            // Never Err: Tauri panics on a setup error inside the macOS launch
            // callback, which cannot unwind and aborts the process.
            Ok(())
        });

    #[cfg(mobile)]
    {
        tauri_builder = tauri_builder.on_window_event(|window, event| {
            let active = match event {
                tauri::WindowEvent::Suspended => false,
                tauri::WindowEvent::Resumed => true,
                _ => return,
            };
            if let Some(backend) = window.try_state::<reader::ReaderBackend>() {
                backend.set_mobile_active(active);
            }
        });
    }

    // These hosts retain the existing store-backed persistence. Desktop
    // has one strict native JSON owner and never installs an exit-time writer.
    #[cfg(any(not(any(unix, windows)), mobile))]
    {
        tauri_builder = tauri_builder.plugin(tauri_plugin_store::Builder::new().build());
    }

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
                use tauri_specta::Event as _;
                match event.id().0.as_str() {
                    MENU_SETTINGS => {
                        let _ = commands::ShowSettings {}.emit(app);
                    }
                    MENU_CLOSE_WINDOW => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.close();
                        }
                    }
                    _ => {}
                }
            });
    }

    terminal_runner(tauri_builder);
}

fn run_tauri(tauri_builder: tauri::Builder<tauri::Wry>) {
    tauri_builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Desktop (and, later, mobile) entry point.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    launch_application(run_tauri);
}

#[cfg(test)]
mod generated_artifacts;

/// Native CLI entry; never accepts a renderer path or activates an old profile.
pub fn run_prepared_profile(directory: std::path::PathBuf) -> Result<(), String> {
    if !directory.is_absolute() {
        return Err("Prepared profile must be absolute".into());
    }
    // Setup validates the immutable archive before constructing any owner.
    launch_application_with_profile(Some(directory), run_tauri);
    Ok(())
}

#[cfg(all(any(unix, windows), not(mobile)))]
pub fn run_return_controller(candidate: std::path::PathBuf) -> Result<(), String> {
    if !candidate.is_absolute() || candidate.as_os_str().len() > 4096 {
        return Err("Return selection unavailable".into());
    }
    launch_application_with_controller(None, None, Some(candidate), run_tauri);
    Ok(())
}

#[cfg(all(any(unix, windows), not(mobile)))]
pub fn run_profile_selection(journal: std::path::PathBuf) -> Result<(), String> {
    let selection = reader_profile::selection_launch(journal)?;
    launch_application_with_selection(None, Some(selection), run_tauri);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::{
        generated_artifacts::export_bindings, launch_application, release_identity, specta_builder,
    };

    /// Regenerates `apps/mdreader/ui/src/bindings.ts` headlessly (no window, no
    /// webview). `scripts/check-bindings.sh` runs this then `git diff
    /// --exit-code` to fail CI on drift.
    #[test]
    fn export_bindings_up_to_date() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/src/bindings.ts");
        let before = std::fs::read(&path).ok().zip(std::fs::metadata(&path).ok());
        export_bindings(&specta_builder()).expect("failed to export typescript bindings");
        if let Some((bytes, metadata)) = before {
            if bytes == std::fs::read(&path).unwrap() {
                assert_eq!(
                    metadata.modified().unwrap(),
                    std::fs::metadata(&path).unwrap().modified().unwrap(),
                    "identical generated bindings must not invalidate Rust's include_str inputs"
                );
            }
        }
    }

    #[test]
    fn production_entrypoint_prelaunch_never_exports_bindings() {
        const CHILD_PROCESS: &str = "MDREADER_TEST_PRELAUNCH_CHILD";

        if std::env::var_os(CHILD_PROCESS).is_some() {
            launch_application(|_tauri_builder| {
                std::fs::write("terminal-runner-called", b"").expect("record terminal runner");
            });
            return;
        }

        let launch_root = tempfile::tempdir().expect("create isolated launch root");
        let launch_directory = launch_root.path().join("finder-cwd");
        let profile_directory = launch_root.path().join("profile");
        std::fs::create_dir(&launch_directory).expect("create isolated launch directory");
        std::fs::create_dir(&profile_directory).expect("create isolated profile directory");

        let status = Command::new(std::env::current_exe().expect("resolve test executable"))
            .args([
                "--exact",
                "tests::production_entrypoint_prelaunch_never_exports_bindings",
                "--nocapture",
            ])
            .env(CHILD_PROCESS, "1")
            .env("HOME", &profile_directory)
            .env("USERPROFILE", &profile_directory)
            .env("XDG_CACHE_HOME", profile_directory.join("cache"))
            .env("XDG_CONFIG_HOME", profile_directory.join("config"))
            .env("XDG_DATA_HOME", profile_directory.join("data"))
            .current_dir(&launch_directory)
            .status()
            .expect("launch production-prelaunch child process");

        assert!(
            status.success(),
            "production prelaunch depended on the launch working directory"
        );
        assert!(
            launch_directory.join("terminal-runner-called").exists(),
            "production prelaunch did not reach the injected terminal runner"
        );
        assert!(
            !launch_root.path().join("ui/src/bindings.ts").exists(),
            "production prelaunch wrote generated bindings during launch"
        );
    }

    #[test]
    fn ios_and_android_e2e_setup_excludes_desktop_keyring_seeders() {
        let source = include_str!("lib.rs");
        let desktop_only_seed = concat!(
            "#[cfg(all(feature = \"e2e-build\", not(mobile)))]\n",
            "    {\n",
            "        library::seed_e2e_local_membership(app.handle())?;\n",
        );

        assert!(
            source.contains(desktop_only_seed),
            "mobile e2e-build setup can still compile calls to desktop keyring seeders"
        );
    }

    #[test]
    fn native_settings_menu_is_typed_and_routes_to_the_renderer() {
        let source = include_str!("lib.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production source");
        assert!(
            source.contains(".id(MENU_SETTINGS)")
                && source.contains("commands::ShowSettings {}.emit(app)"),
            "the macOS application menu must expose Settings and route it through a typed event"
        );
        assert!(
            source.contains("commands::ShowSettings"),
            "the Settings event must be registered in the Specta event surface"
        );
        let renderer = include_str!("../../ui/src/shell/AppShell.tsx")
            .split_whitespace()
            .collect::<String>();
        let backend = include_str!("../../ui/src/platform/tauri-backend.ts")
            .split_whitespace()
            .collect::<String>();
        assert!(
            renderer.contains("readerBackend().app.onShowSettings(")
                && backend.contains("events.mdreaderShowSettings.listen"),
            "the renderer must listen for the native Settings request"
        );
    }

    #[test]
    fn desktop_window_starts_resizable_with_its_lower_edge_on_a_small_display() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("valid Tauri config");
        let window = config["app"]["windows"]
            .as_array()
            .and_then(|windows| windows.first())
            .expect("main desktop window");
        assert_eq!(window["resizable"], serde_json::Value::Bool(true));
        assert!(
            window["height"].as_u64().is_some_and(|height| height <= 700),
            "the initial content height must leave the lower native resize edge reachable on a 768px display"
        );
    }

    #[test]
    fn webview_ships_a_content_security_policy_without_script_eval() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("valid Tauri config");
        let security = &config["app"]["security"];
        for key in ["csp", "devCsp"] {
            let csp = security[key].as_str().expect("CSP is a policy string");
            let script_src = csp
                .split(';')
                .map(str::trim)
                .find(|directive| directive.starts_with("script-src "))
                .expect("script-src directive");
            assert!(
                !script_src.contains("'unsafe-eval'") && !script_src.contains("'unsafe-inline'"),
                "{key} must not allow string eval or inline script: {script_src}"
            );
            assert!(
                csp.contains("object-src 'none'"),
                "{key} must block plugins"
            );
        }
        // Tauri appends hashes/nonces to style-src when it finds inline styles,
        // which would silently disable the 'unsafe-inline' that Mermaid SVG
        // <style> blocks and React style attributes rely on.
        assert_eq!(
            security["dangerousDisableAssetCspModification"],
            serde_json::json!(["style-src"])
        );
    }

    #[test]
    fn shared_mobile_library_carries_the_configured_release_identity() {
        let identity = release_identity().as_bytes();
        assert!(identity.starts_with(b"MDREADER_RELEASE_IDENTITY_V1\0"));
        let bundle = if cfg!(feature = "e2e-build") {
            b"bundleIdentifier=dev.mdreader.app.e2e\0".as_slice()
        } else {
            b"bundleIdentifier=dev.mdreader.app\0".as_slice()
        };
        assert!(identity
            .windows(bundle.len())
            .any(|window| window == bundle));
        assert!(identity
            .windows(b"sourceCommit=".len() + 40 + 1)
            .any(|window| window
                == concat!(
                    "sourceCommit=",
                    env!("MDREADER_RELEASE_SOURCE_COMMIT"),
                    "\0"
                )
                .as_bytes()));
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
            ("mobile_reader.rs", include_str!("mobile_reader.rs")),
            ("reader_workload.rs", include_str!("reader_workload.rs")),
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
