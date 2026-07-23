//! Tauri command surface for the MDReader shell.
//!
//! P2 keeps every command deliberately THIN: filesystem reads, a folder picker,
//! and a workspace watcher. No business logic lives here — the real
//! ingest/query engine (P3) will replace this temporary `fs` command layer. Each
//! command derives `specta::Type` so tauri-specta emits matching TypeScript into
//! `apps/mdreader/ui/src/bindings.ts`.
//!
//! ## Workspace containment
//!
//! Filesystem access is confined to a single *granted* workspace root, held in
//! [`WorkspaceState`]. The grant is established by the native folder picker
//! ([`pick_workspace_folder`]) or restored explicitly ([`set_workspace`], used on
//! session restore). Every path-taking command canonicalizes both the granted
//! root and the candidate path and rejects anything that does not
//! `starts_with(root)` *after* canonicalization. Canonicalization resolves `..`
//! segments and symlinks, so this single check defeats path traversal AND
//! symlink-escape (a symlink living inside the workspace but pointing outside it
//! resolves to its real, outside target and is rejected).

use std::path::{Path, PathBuf};
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
// Workspace containment
// --------------------------------------------------------------------------

/// The single granted workspace root (canonicalized), or `None` before a folder
/// has been picked/restored. Every path-taking command validates its argument
/// against this root.
#[derive(Default)]
pub struct WorkspaceState(pub Mutex<Option<PathBuf>>);

impl WorkspaceState {
    /// The current granted root, or an error when no workspace has been granted.
    fn root(&self) -> Result<PathBuf, String> {
        self.0
            .lock()
            .map_err(|_| "workspace state poisoned".to_string())?
            .clone()
            .ok_or_else(|| "no workspace has been opened".to_string())
    }

    /// Replace the granted root with `root` (already canonicalized).
    fn set(&self, root: PathBuf) -> Result<(), String> {
        *self
            .0
            .lock()
            .map_err(|_| "workspace state poisoned".to_string())? = Some(root);
        Ok(())
    }
}

/// Canonicalize `candidate` and confirm it stays within the (already
/// canonical) `root`. Returns the resolved, real path on success. This is the
/// one check that defeats both `../` traversal and symlink-escape: canonicalize
/// resolves every `..` and symlink, so `starts_with` is evaluated on real,
/// fully-resolved paths.
fn resolve_within(root: &Path, candidate: &Path) -> Result<PathBuf, String> {
    let real = std::fs::canonicalize(candidate)
        .map_err(|err| format!("cannot resolve {}: {err}", candidate.display()))?;
    if real.starts_with(root) {
        Ok(real)
    } else {
        Err(format!(
            "path escapes the workspace: {}",
            candidate.display()
        ))
    }
}

/// Resolve a *readable document* path: contained in `root`, an existing file,
/// and a markdown document. Rejects everything else (directories, non-`.md`).
fn resolve_doc(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let real = resolve_within(root, path)?;
    if !real.is_file() {
        return Err(format!("not a file: {}", path.display()));
    }
    if !is_markdown(&real) {
        return Err(format!("not a markdown document: {}", path.display()));
    }
    Ok(real)
}

/// Resolve a *listable/watchable directory*: contained in `root` and an existing
/// directory.
fn resolve_dir(root: &Path, dir: &Path) -> Result<PathBuf, String> {
    let real = resolve_within(root, dir)?;
    if !real.is_dir() {
        return Err(format!("not a directory: {}", dir.display()));
    }
    Ok(real)
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
// set_workspace
// --------------------------------------------------------------------------

/// Grant (or re-grant) `root` as the active workspace, returning its
/// canonicalized absolute path. Called by the frontend when a workspace is
/// restored from a persisted session (the folder picker grants directly). Every
/// subsequent `read_doc`/`list_workspace_files`/`watch_workspace` is confined to
/// this root.
#[tauri::command]
#[specta::specta]
pub fn set_workspace(
    root: String,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<String, String> {
    let real = std::fs::canonicalize(&root).map_err(|err| format!("cannot resolve {root}: {err}"))?;
    if !real.is_dir() {
        return Err(format!("not a directory: {root}"));
    }
    let granted = real.to_string_lossy().into_owned();
    state.set(real)?;
    Ok(granted)
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
/// pruned, depth-bounded). `root` must be within the granted workspace. Symlinks
/// are never followed (`follow_links(false)`), so a symlinked subtree cannot
/// leak files from outside the workspace. Thin: no parsing, no ranking — just
/// the raw tree the sidebar renders.
#[tauri::command]
#[specta::specta]
pub fn list_workspace_files(
    root: String,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<Vec<FileEntry>, String> {
    let granted = state.root()?;
    let root_path = resolve_dir(&granted, Path::new(&root))?;

    let mut entries = Vec::new();
    for entry in WalkDir::new(&root_path)
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

/// Read a single markdown document from within the granted workspace. Thin:
/// resolves + validates the path, reads the bytes, hashes them, decodes to text.
/// Rejects paths outside the workspace, non-files, and non-markdown extensions.
#[tauri::command]
#[specta::specta]
pub fn read_doc(
    path: String,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<DocPayload, String> {
    let granted = state.root()?;
    let real = resolve_doc(&granted, Path::new(&path))?;

    let bytes = std::fs::read(&real).map_err(|err| format!("read {path}: {err}"))?;
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let mtime = mtime_millis(std::fs::metadata(&real).ok().and_then(|m| m.modified().ok()));
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
/// when the user cancels. The chosen folder is granted as the active workspace
/// (canonicalized) so subsequent reads are confined to it. Async so blocking on
/// the dialog never parks the main thread; the dialog plugin pumps the picker on
/// the event loop.
#[tauri::command]
#[specta::specta]
pub async fn pick_workspace_folder(
    app: AppHandle,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<Option<String>, String> {
    let Some(path) = app
        .dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|file| file.into_path().ok())
    else {
        return Ok(None);
    };

    // Grant the picked folder (canonicalized) as the workspace root.
    let real = std::fs::canonicalize(&path)
        .map_err(|err| format!("cannot resolve {}: {err}", path.display()))?;
    let granted = real.to_string_lossy().into_owned();
    state.set(real)?;
    Ok(Some(granted))
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

/// Payload for the `mdreader://watch-error` event — a *runtime* watcher failure
/// (raised from the notify callback, distinct from the command-time errors
/// `watch_workspace` returns). The frontend surfaces it as a degraded watcher
/// status in the StatusBar.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "mdreader://watch-error")]
pub struct WatchError {
    /// Human-readable description of the watcher failure.
    pub message: String,
}

/// Holds the live workspace watcher. Parking the debouncer here keeps it alive;
/// replacing it (switching folders) drops the previous one, stopping that watch.
#[derive(Default)]
pub struct WatcherState(pub Mutex<Option<Debouncer<RecommendedWatcher>>>);

/// Begin watching `root` recursively, emitting a debounced
/// `mdreader://fs-changed` for every markdown file that changes. `root` must be
/// within the granted workspace. The swap is atomic: a fresh debouncer is built
/// and its watch armed *before* the previous one is dropped, so a failed new
/// watch leaves the old workspace watched (consistent state either way).
/// Runtime failures during watching are surfaced via `mdreader://watch-error`.
#[tauri::command]
#[specta::specta]
pub fn watch_workspace(
    app: AppHandle,
    root: String,
    workspace: tauri::State<'_, WorkspaceState>,
    state: tauri::State<'_, WatcherState>,
) -> Result<(), String> {
    let granted = workspace.root()?;
    let root_path = resolve_dir(&granted, Path::new(&root))?;

    let emit_app = app.clone();
    let mut debouncer = new_debouncer(
        Duration::from_millis(WATCH_DEBOUNCE_MS),
        move |result: DebounceEventResult| match result {
            Ok(events) => {
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
            }
            Err(error) => {
                let _ = WatchError {
                    message: error.to_string(),
                }
                .emit(&emit_app);
            }
        },
    )
    .map_err(|err| format!("watcher init failed: {err}"))?;

    // Arm the new watch BEFORE parking it. On failure we return here with the
    // freshly-built debouncer dropped and the previously-parked watcher intact,
    // so the old workspace stays watched (no orphan, no gap).
    debouncer
        .watcher()
        .watch(&root_path, RecursiveMode::Recursive)
        .map_err(|err| format!("watch {root} failed: {err}"))?;

    // Park the debouncer (drops+stops the previous watch on reassignment).
    *state
        .0
        .lock()
        .map_err(|_| "watcher state poisoned".to_string())? = Some(debouncer);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{resolve_dir, resolve_doc, resolve_within};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// A workspace root plus the canonicalized path (macOS `/var` → `/private/var`).
    fn workspace() -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("tempdir");
        let root = fs::canonicalize(dir.path()).expect("canonicalize root");
        (dir, root)
    }

    #[test]
    fn allows_a_legit_nested_markdown_file() {
        let (_dir, root) = workspace();
        let nested = root.join("notes/deep");
        fs::create_dir_all(&nested).unwrap();
        let file = nested.join("page.md");
        fs::write(&file, "# hi").unwrap();

        let resolved = resolve_doc(&root, &file).expect("nested markdown allowed");
        assert!(resolved.starts_with(&root));
    }

    #[test]
    fn rejects_parent_traversal() {
        let (_dir, root) = workspace();
        // A sibling file outside the workspace, reached via `../`.
        let outside = root.parent().unwrap().join("outside.md");
        fs::write(&outside, "secret").unwrap();
        let traversal = root.join("../outside.md");

        assert!(resolve_doc(&root, &traversal).is_err());
        let _ = fs::remove_file(&outside);
    }

    #[test]
    fn rejects_absolute_path_outside_workspace() {
        let (_dir, root) = workspace();
        let (_other_dir, other_root) = workspace();
        let outside = other_root.join("elsewhere.md");
        fs::write(&outside, "secret").unwrap();

        assert!(resolve_doc(&root, &outside).is_err());
    }

    #[test]
    fn rejects_non_markdown_extension() {
        let (_dir, root) = workspace();
        let file = root.join("data.json");
        fs::write(&file, "{}").unwrap();

        assert!(resolve_doc(&root, &file).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_inside_pointing_outside() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = workspace();
        let (_other_dir, other_root) = workspace();
        let secret = other_root.join("secret.md");
        fs::write(&secret, "# secret").unwrap();

        // A symlink that lives INSIDE the workspace but resolves OUTSIDE it.
        let link = root.join("escape.md");
        symlink(&secret, &link).unwrap();

        // The symlink is contained by path, but canonicalization resolves it to
        // the outside target, so the containment check rejects it.
        assert!(resolve_doc(&root, &link).is_err());
    }

    #[test]
    fn rejects_listing_a_directory_outside_workspace() {
        let (_dir, root) = workspace();
        let (_other_dir, other_root) = workspace();

        assert!(resolve_dir(&root, &other_root).is_err());
    }

    #[test]
    fn allows_listing_a_nested_directory() {
        let (_dir, root) = workspace();
        let nested = root.join("sub");
        fs::create_dir_all(&nested).unwrap();

        let resolved = resolve_dir(&root, &nested).expect("nested dir allowed");
        assert!(resolved.starts_with(&root));
    }

    #[test]
    fn resolve_within_allows_the_root_itself() {
        let (_dir, root) = workspace();
        let resolved = resolve_within(&root, &root).expect("root allowed");
        assert_eq!(resolved, root);
    }
}
