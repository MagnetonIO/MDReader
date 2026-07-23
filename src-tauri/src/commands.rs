//! Tauri command surface for the MDReader shell.
//!
//! P2 keeps every command deliberately THIN: filesystem reads, a folder picker,
//! and a workspace watcher. No business logic lives here — the real
//! ingest/query engine (P3) will replace this temporary `fs` command layer. Each
//! command derives `specta::Type` so tauri-specta emits matching TypeScript into
//! `apps/mdreader/ui/src/bindings.ts`.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, DebouncedEventKind, Debouncer};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;
use tauri_specta::Event;
use walkdir::{DirEntry, WalkDir};

/// Max directory depth walked by [`list_workspace_files`]. A sanity bound so a
/// pathological or symlinked tree cannot stall the walk.
const MAX_WALK_DEPTH: usize = 12;

/// Debounce window for the workspace watcher. Collapses editor save-storms (a
/// single logical edit often lands as several `write` syscalls) into one event.
const WATCH_DEBOUNCE_MS: u64 = 500;

/// True for the markdown extensions the reader understands.
fn is_markdown(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("md") | Some("markdown")
    )
}

/// Prune hidden directories (`.git`, `.obsidian`, …) from the walk while still
/// descending the workspace root itself.
fn is_hidden_dir(entry: &DirEntry) -> bool {
    entry.depth() > 0
        && entry.file_type().is_dir()
        && entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with('.'))
}

/// Milliseconds since the Unix epoch for a filesystem timestamp, or `0.0` when
/// the platform cannot report one.
fn mtime_millis(modified: Option<std::time::SystemTime>) -> f64 {
    modified
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|delta| delta.as_millis() as f64)
        .unwrap_or(0.0)
}

// --------------------------------------------------------------------------
// ping — retained from the P0 scaffold as a liveness probe for the IPC path.
// --------------------------------------------------------------------------

/// Reply returned by [`ping`].
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PingResponse {
    /// The message echoed back to the caller.
    pub echoed: String,
    /// The desktop shell's crate version.
    pub version: String,
}

/// Echoes `msg` back to the webview alongside the shell version.
#[tauri::command]
#[specta::specta]
pub fn ping(msg: String) -> PingResponse {
    PingResponse {
        echoed: msg,
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

// --------------------------------------------------------------------------
// list_workspace_files
// --------------------------------------------------------------------------

/// One markdown file discovered under a workspace root.
#[derive(Debug, Clone, Serialize, Type)]
pub struct FileEntry {
    /// Absolute path to the file.
    pub path: String,
    /// File name including extension (e.g. `overview.md`).
    pub name: String,
    /// Absolute path of the containing directory.
    pub dir: String,
    /// Modification time in milliseconds since the Unix epoch.
    pub mtime: f64,
    /// File size in bytes (an `f64` so it round-trips through JSON losslessly for
    /// realistic document sizes).
    pub size: f64,
}

/// Recursively list the `.md`/`.markdown` files under `root` (hidden directories
/// pruned, depth-bounded). Thin: no parsing, no ranking — just the raw tree the
/// sidebar renders.
#[tauri::command]
#[specta::specta]
pub fn list_workspace_files(root: String) -> Result<Vec<FileEntry>, String> {
    let root_path = Path::new(&root);
    if !root_path.is_dir() {
        return Err(format!("not a directory: {root}"));
    }

    let mut entries = Vec::new();
    for entry in WalkDir::new(root_path)
        .max_depth(MAX_WALK_DEPTH)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_hidden_dir(entry))
    {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if !entry.file_type().is_file() || !is_markdown(path) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        entries.push(FileEntry {
            path: path.to_string_lossy().into_owned(),
            name: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string(),
            dir: path
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_default(),
            mtime: mtime_millis(meta.modified().ok()),
            size: meta.len() as f64,
        });
    }

    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

// --------------------------------------------------------------------------
// read_doc
// --------------------------------------------------------------------------

/// The bytes of a document plus the provenance a conflict guard needs.
#[derive(Debug, Clone, Serialize, Type)]
pub struct DocPayload {
    /// UTF-8 file contents (lossily decoded; invalid bytes become U+FFFD).
    pub content: String,
    /// Modification time in milliseconds since the Unix epoch.
    pub mtime: f64,
    /// blake3 hex digest of the raw file bytes — the anchor for the later
    /// external-edit conflict guard (P3).
    pub hash: String,
}

/// Read a single document. Thin: reads the bytes, hashes them, decodes to text.
#[tauri::command]
#[specta::specta]
pub fn read_doc(path: String) -> Result<DocPayload, String> {
    let bytes = std::fs::read(&path).map_err(|err| format!("read {path}: {err}"))?;
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let mtime = mtime_millis(std::fs::metadata(&path).ok().and_then(|m| m.modified().ok()));
    Ok(DocPayload {
        content: String::from_utf8_lossy(&bytes).into_owned(),
        mtime,
        hash,
    })
}

// --------------------------------------------------------------------------
// pick_workspace_folder
// --------------------------------------------------------------------------

/// Open the native folder picker and return the chosen absolute path, or `None`
/// when the user cancels. Async so blocking on the dialog never parks the main
/// thread; the dialog plugin pumps the picker on the event loop.
#[tauri::command]
#[specta::specta]
pub async fn pick_workspace_folder(app: AppHandle) -> Option<String> {
    app.dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|file| file.into_path().ok())
        .map(|path| path.to_string_lossy().into_owned())
}

// --------------------------------------------------------------------------
// watch_workspace
// --------------------------------------------------------------------------

/// Payload for the `mdreader://fs-changed` event. tauri-specta emits a typed
/// `events.fsChanged` listener into the bindings.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "mdreader://fs-changed")]
pub struct FsChanged {
    /// Absolute path of the changed file.
    pub path: String,
    /// Coarse change kind: `"any"` for a settled change, `"continuous"` while a
    /// path is still being written (the debouncer timed out mid-write).
    pub kind: String,
}

/// Holds the live workspace watcher. Parking the debouncer here keeps it alive;
/// replacing it (switching folders) drops the previous one, stopping that watch.
#[derive(Default)]
pub struct WatcherState(pub Mutex<Option<Debouncer<RecommendedWatcher>>>);

/// Begin watching `root` recursively, emitting a debounced
/// `mdreader://fs-changed` for every markdown file that changes. Replaces any
/// previous watch. Thin: it wires notify to the event bus, nothing more.
#[tauri::command]
#[specta::specta]
pub fn watch_workspace(
    app: AppHandle,
    root: String,
    state: tauri::State<'_, WatcherState>,
) -> Result<(), String> {
    let root_path = Path::new(&root);
    if !root_path.is_dir() {
        return Err(format!("not a directory: {root}"));
    }

    let emit_app = app.clone();
    let mut debouncer = new_debouncer(
        Duration::from_millis(WATCH_DEBOUNCE_MS),
        move |result: DebounceEventResult| {
            let Ok(events) = result else { return };
            for event in events {
                if !is_markdown(&event.path) {
                    continue;
                }
                let kind = match event.kind {
                    DebouncedEventKind::AnyContinuous => "continuous",
                    _ => "any",
                };
                let _ = FsChanged {
                    path: event.path.to_string_lossy().into_owned(),
                    kind: kind.to_string(),
                }
                .emit(&emit_app);
            }
        },
    )
    .map_err(|err| format!("watcher init failed: {err}"))?;

    debouncer
        .watcher()
        .watch(root_path, RecursiveMode::Recursive)
        .map_err(|err| format!("watch {root} failed: {err}"))?;

    // Park the debouncer (drops+stops any previous watch on reassignment).
    *state
        .0
        .lock()
        .map_err(|_| "watcher state poisoned".to_string())? = Some(debouncer);
    Ok(())
}
