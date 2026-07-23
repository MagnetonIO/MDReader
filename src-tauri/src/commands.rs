//! Tauri command surface for the MDReader shell — the *filesystem* half.
//!
//! This module owns the thin, containment-guarded fs layer: a folder picker, a
//! workspace watcher, and live-from-disk document reads. Reading is always done
//! straight off disk (never a stale index copy), so [`read_doc`] stays here. The
//! *engine* half — the embedded CatDB index, live import progress, engine-backed
//! file listing, and wikilink resolution — lives in [`crate::engine`] and reuses
//! this module's containment helpers. Each command derives `specta::Type` so
//! tauri-specta emits matching TypeScript into `apps/mdreader/ui/src/bindings.ts`.
//!
//! ## Workspace containment
//!
//! Filesystem access is confined to a single *granted* workspace root, held in
//! [`WorkspaceState`]. The grant is established by [`set_workspace`] (session
//! restore, and to read back the canonical path) and by [`watch_workspace`],
//! which adopts its target root as the grant *atomically with* arming the
//! watcher — so the stored root and the live watcher can never diverge. The
//! native folder picker ([`pick_workspace_folder`]) only *returns* a path; it
//! never mutates the grant, so a pick that is later abandoned (e.g. the watcher
//! fails to arm) leaves the active workspace untouched. Every path-taking command
//! canonicalizes both the granted root and the candidate path and rejects
//! anything that does not `starts_with(root)` *after* canonicalization.
//! Canonicalization resolves `..` segments and symlinks, so this single check
//! defeats path traversal AND symlink-escape (a symlink living inside the
//! workspace but pointing outside it resolves to its real, outside target and is
//! rejected).
//!
//! ## TOCTOU (accepted)
//!
//! Containment canonicalizes the path, then the command reads/walks/watches the
//! resolved path in a *separate* step, so a path could in principle be swapped
//! between the check and the use. For a LOCAL single-user desktop reader (not a
//! multi-tenant service) this race is proportionately minor — there is no other
//! tenant to win it — so it is accepted rather than locked against. Each affected
//! command ([`read_doc`], [`watch_workspace`], and the engine module's listing)
//! notes it at the use site.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{
    new_debouncer, DebounceEventHandler, DebounceEventResult, DebouncedEventKind, Debouncer,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;
use tauri_specta::Event;

/// Debounce window for the workspace watcher. Collapses editor save-storms (a
/// single logical edit often lands as several `write` syscalls) into one event.
const WATCH_DEBOUNCE_MS: u64 = 500;

/// True for the markdown extensions the reader understands.
pub(crate) fn is_markdown(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("md") | Some("markdown")
    )
}

/// Milliseconds since the Unix epoch for a filesystem timestamp, or `0.0` when
/// the platform cannot report one.
pub(crate) fn mtime_millis(modified: Option<std::time::SystemTime>) -> f64 {
    modified
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|delta| delta.as_millis() as f64)
        .unwrap_or(0.0)
}

// --------------------------------------------------------------------------
// Workspace containment
// --------------------------------------------------------------------------

/// The active workspace, held as a single immutable [`Epoch`](crate::engine::Epoch)
/// (`None` before a folder has been picked/restored). A switch **creates a new
/// epoch and swaps it in** — that swap is the one revocation barrier: root,
/// generation, engine, and import status all move together, never sampled at
/// different instants. Commands capture the current `Arc<Epoch>` once and compare
/// identity via [`is_current`](Self::is_current) at their commit points.
#[derive(Default)]
pub struct WorkspaceState {
    current: Mutex<Option<Arc<crate::engine::Epoch>>>,
}

impl WorkspaceState {
    /// The current epoch, or `None` before any workspace has been granted.
    pub(crate) fn current(&self) -> Option<Arc<crate::engine::Epoch>> {
        self.current.lock().ok().and_then(|slot| slot.clone())
    }

    /// The current granted root, or an error when no workspace has been granted.
    pub(crate) fn root(&self) -> Result<PathBuf, String> {
        self.current()
            .map(|epoch| epoch.root.clone())
            .ok_or_else(|| "no workspace has been opened".to_string())
    }

    /// Whether `epoch` is the currently-active era — the `Arc::ptr_eq` identity
    /// check every command uses at its commit points to discard stale work.
    pub(crate) fn is_current(&self, epoch: &Arc<crate::engine::Epoch>) -> bool {
        self.current()
            .is_some_and(|current| Arc::ptr_eq(&current, epoch))
    }

    /// The current epoch's generation (0 when no workspace is open).
    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.current().map_or(0, |epoch| epoch.generation)
    }

    /// Create a fresh epoch for `root` (already canonicalized) and swap it in,
    /// advancing the generation. THE revocation barrier: the returned `Arc` is the
    /// new current era; any epoch captured before this call is now stale. Returns
    /// the new epoch.
    pub(crate) fn switch(&self, root: PathBuf) -> Result<Arc<crate::engine::Epoch>, String> {
        let mut slot = self
            .current
            .lock()
            .map_err(|_| "workspace state poisoned".to_string())?;
        let generation = slot.as_ref().map_or(0, |epoch| epoch.generation) + 1;
        let epoch = crate::engine::Epoch::new(root, generation);
        *slot = Some(Arc::clone(&epoch));
        Ok(epoch)
    }

    /// Test helper: a state already granting `root` (generation 1).
    #[cfg(test)]
    pub(crate) fn granted(root: PathBuf) -> Self {
        let state = Self::default();
        state.switch(root).expect("grant test root");
        state
    }
}

/// Canonicalize `candidate` and confirm it stays within the (already
/// canonical) `root`. Returns the resolved, real path on success. This is the
/// one check that defeats both `../` traversal and symlink-escape: canonicalize
/// resolves every `..` and symlink, so `starts_with` is evaluated on real,
/// fully-resolved paths.
pub(crate) fn resolve_within(root: &Path, candidate: &Path) -> Result<PathBuf, String> {
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
pub(crate) fn resolve_dir(root: &Path, dir: &Path) -> Result<PathBuf, String> {
    let real = resolve_within(root, dir)?;
    if !real.is_dir() {
        return Err(format!("not a directory: {}", dir.display()));
    }
    Ok(real)
}

/// Canonicalize `root` and confirm it is an existing directory — the check
/// [`set_workspace`] and [`watch_workspace`] share to *establish* a workspace
/// grant (as opposed to validating a candidate against an existing grant).
pub(crate) fn canonical_dir(root: &str) -> Result<PathBuf, String> {
    let real =
        std::fs::canonicalize(root).map_err(|err| format!("cannot resolve {root}: {err}"))?;
    if !real.is_dir() {
        return Err(format!("not a directory: {root}"));
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
    let real = canonical_dir(&root)?;
    let granted = real.to_string_lossy().into_owned();
    // A grant change is a new epoch (the revocation barrier), even on the
    // degraded/no-watcher path.
    state.switch(real)?;
    Ok(granted)
}

// --------------------------------------------------------------------------
// FileEntry — the sidebar row shape, produced by the engine module's listing.
// --------------------------------------------------------------------------

/// One markdown file surfaced to the sidebar tree. Produced either from the
/// engine's index (after an import) or, before the first import finishes, from a
/// live filesystem scan — see [`crate::engine::list_workspace_files`].
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
    /// True when a gitignore/`.contextignore` rule matched the file (the tree
    /// dims it). Index-listed documents are never ignored (the ingest scan drops
    /// them before they reach the index); only the fs-fallback listing sets this.
    pub ignored: bool,
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
    // TOCTOU: the read below runs after this containment check (accepted — see the
    // module-level "TOCTOU" note; local single-user reader).
    let real = resolve_doc(&granted, Path::new(&path))?;

    let bytes = std::fs::read(&real).map_err(|err| format!("read {path}: {err}"))?;
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let mtime = mtime_millis(
        std::fs::metadata(&real)
            .ok()
            .and_then(|m| m.modified().ok()),
    );
    Ok(DocPayload {
        content: String::from_utf8_lossy(&bytes).into_owned(),
        mtime,
        hash,
    })
}

// --------------------------------------------------------------------------
// pick_workspace_folder
// --------------------------------------------------------------------------

/// Open the native folder picker and return the chosen absolute path
/// (canonicalized), or `None` when the user cancels. This does NOT mutate the
/// workspace grant: the grant is adopted only when the frontend commits to the
/// folder via [`watch_workspace`], so a pick that is later abandoned leaves the
/// active workspace untouched. Async so blocking on the dialog never parks the
/// main thread; the dialog plugin pumps the picker on the event loop.
#[tauri::command]
#[specta::specta]
pub async fn pick_workspace_folder(app: AppHandle) -> Result<Option<String>, String> {
    let Some(path) = app
        .dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|file| file.into_path().ok())
    else {
        return Ok(None);
    };

    let real = std::fs::canonicalize(&path)
        .map_err(|err| format!("cannot resolve {}: {err}", path.display()))?;
    Ok(Some(real.to_string_lossy().into_owned()))
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
/// `mdreader://fs-changed` for every markdown file that changes, adopt `root` as
/// the granted workspace, and **return its canonical path**. This is the ONE
/// atomic "switch workspace" operation the UI awaits: the new root becomes the
/// grant *and* the live watcher together (advancing the root generation), or
/// neither does — so the caller never has to sequence a separate `set_workspace`
/// against the watcher arming. A `root` that cannot be resolved, or whose watch
/// fails to arm, leaves BOTH the previous grant and the previous watcher fully
/// intact and returns the error — the caller stays on the old workspace, and
/// backend state can never diverge (a live watcher on one root while the grant
/// names another). Runtime failures *during* watching are surfaced separately
/// via `mdreader://watch-error`.
#[tauri::command]
#[specta::specta]
pub fn watch_workspace(
    app: AppHandle,
    root: String,
    workspace: tauri::State<'_, WorkspaceState>,
    state: tauri::State<'_, WatcherState>,
) -> Result<String, String> {
    let emit_app = app.clone();
    let granted = watch_and_commit(
        &root,
        &workspace,
        &state,
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
    )?;
    Ok(granted.to_string_lossy().into_owned())
}

/// The workspace-switch core, factored out of [`watch_workspace`] so it is
/// exercisable without an `AppHandle` (the handle is only needed to build the
/// production `on_event` sink; tests pass a no-op). Resolves `root`, arms a fresh
/// watcher into a *local*, and — only on success — commits the new grant + new
/// watcher together. Any earlier failure returns with nothing mutated.
fn watch_and_commit<F: DebounceEventHandler>(
    root: &str,
    workspace: &WorkspaceState,
    watcher: &WatcherState,
    on_event: F,
) -> Result<PathBuf, String> {
    // A workspace SWITCH targets a NEW root that is not inside the old grant, so
    // resolve it independently: this command is the one that ESTABLISHES the grant.
    // TOCTOU: the watch below arms after this check (accepted — see the
    // module-level "TOCTOU" note; local single-user reader).
    let new_root = canonical_dir(root)?;

    // Build + arm the new watcher into a LOCAL, holding NO state lock across
    // construction. A failure here changes nothing — the previous grant + watcher
    // stay live, so the caller simply stays on the old workspace.
    let mut debouncer = new_debouncer(Duration::from_millis(WATCH_DEBOUNCE_MS), on_event)
        .map_err(|err| format!("watcher init failed: {err}"))?;
    debouncer
        .watcher()
        .watch(&new_root, RecursiveMode::Recursive)
        .map_err(|err| format!("watch {root} failed: {err}"))?;

    commit_workspace(workspace, watcher, new_root.clone(), debouncer)?;
    Ok(new_root)
}

/// Swap in a fresh [`Epoch`](crate::engine::Epoch) for `new_root` and `debouncer`
/// as the live watcher, dropping (and thereby stopping) the previous watcher.
/// Reached only after the new watch is armed. Acquires BOTH state locks up front
/// and only then performs the two (now-infallible) writes, so there is no window
/// in which the new epoch is installed while the watcher swap is still pending: a
/// poisoned lock aborts before ANY state is touched (epoch + watcher stay
/// intact), and once both locks are held the new epoch and the watcher move as
/// one and can never diverge. Lock order is watcher→epoch everywhere both are
/// held (this is the only such site), so no deadlock. The epoch swap IS the
/// revocation barrier for any import still running on the old root.
fn commit_workspace(
    workspace: &WorkspaceState,
    watcher: &WatcherState,
    new_root: PathBuf,
    debouncer: Debouncer<RecommendedWatcher>,
) -> Result<(), String> {
    let mut watcher_slot = watcher
        .0
        .lock()
        .map_err(|_| "watcher state poisoned".to_string())?;
    let mut epoch_slot = workspace
        .current
        .lock()
        .map_err(|_| "workspace state poisoned".to_string())?;
    // Both locks held — neither write below can fail, so the swap is atomic.
    let generation = epoch_slot.as_ref().map_or(0, |epoch| epoch.generation) + 1;
    *epoch_slot = Some(crate::engine::Epoch::new(new_root, generation));
    *watcher_slot = Some(debouncer);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_dir, resolve_dir, resolve_doc, resolve_within, watch_and_commit,
        DebounceEventResult, WatcherState, WorkspaceState,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;
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

    #[test]
    fn canonical_dir_accepts_a_directory_and_rejects_a_file() {
        let (_dir, root) = workspace();
        assert_eq!(canonical_dir(root.to_str().unwrap()).unwrap(), root);

        let file = root.join("note.md");
        fs::write(&file, "# hi").unwrap();
        assert!(canonical_dir(file.to_str().unwrap()).is_err());
        assert!(canonical_dir(root.join("nope").to_str().unwrap()).is_err());
    }

    #[test]
    fn watch_switch_swaps_atomically_and_failure_preserves_old_state() {
        let (_a, root_a) = workspace();
        let (_b, root_b) = workspace();
        let workspace = WorkspaceState::granted(root_a.clone());
        let watcher = WatcherState(Mutex::new(None));

        // The `on_event` sink is irrelevant to the swap logic — a no-op stands in
        // for the production emit closure so no AppHandle is needed here.
        let sink = || |_result: DebounceEventResult| {};

        // Arm the initial watch on A.
        watch_and_commit(root_a.to_str().unwrap(), &workspace, &watcher, sink()).expect("arm A");
        assert_eq!(workspace.root().unwrap(), root_a);
        assert!(watcher.0.lock().unwrap().is_some());

        // A valid switch to B swaps BOTH the grant and the watcher together.
        watch_and_commit(root_b.to_str().unwrap(), &workspace, &watcher, sink()).expect("switch B");
        assert_eq!(workspace.root().unwrap(), root_b);
        assert!(watcher.0.lock().unwrap().is_some());

        // A switch whose target cannot be resolved fails BEFORE commit (the same
        // pre-commit `?`-return path an arm failure would take), leaving the OLD
        // root + watcher fully intact — no half-applied switch, no divergence.
        let bogus = root_b.join("does-not-exist");
        assert!(watch_and_commit(bogus.to_str().unwrap(), &workspace, &watcher, sink()).is_err());
        assert_eq!(workspace.root().unwrap(), root_b);
        assert!(watcher.0.lock().unwrap().is_some());
    }

    #[test]
    fn commit_failure_leaves_the_grant_unadvanced() {
        let (_a, root_a) = workspace();
        let (_b, root_b) = workspace();
        let workspace = WorkspaceState::granted(root_a.clone());
        let watcher = WatcherState(Mutex::new(None));

        // Poison the watcher lock (the FIRST lock commit_workspace acquires) by
        // panicking while it is held. commit_workspace must then abort BEFORE the
        // grant is touched — the whole point of locking watcher first.
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = watcher.0.lock().unwrap();
            panic!("poison the watcher lock");
        }));
        std::panic::set_hook(prev_hook);

        // root_b is perfectly valid and the watcher would arm fine; the ONLY
        // failure is the poisoned commit lock. The grant must stay on A, not
        // advance to B while the watcher swap never happened (no split commit).
        let sink = |_result: DebounceEventResult| {};
        assert!(watch_and_commit(root_b.to_str().unwrap(), &workspace, &watcher, sink).is_err());
        assert_eq!(workspace.root().unwrap(), root_a);
    }

    #[test]
    fn a_rapid_double_switch_converges_to_the_last_root_and_advances_generation() {
        let (_a, root_a) = workspace();
        let (_b, root_b) = workspace();
        let (_c, root_c) = workspace();
        let workspace = WorkspaceState::granted(root_a.clone());
        let watcher = WatcherState(Mutex::new(None));
        let sink = || |_result: DebounceEventResult| {};

        let g_start = workspace.generation();
        // Two switches back-to-back (B then C) converge on C, and EACH advances
        // the root generation — the hook an import uses to notice it is stale and
        // discard its late status rather than clobber the new workspace's.
        watch_and_commit(root_b.to_str().unwrap(), &workspace, &watcher, sink()).expect("switch B");
        let g_b = workspace.generation();
        watch_and_commit(root_c.to_str().unwrap(), &workspace, &watcher, sink()).expect("switch C");
        let g_c = workspace.generation();

        assert_eq!(workspace.root().unwrap(), root_c, "converged on the last root");
        assert!(watcher.0.lock().unwrap().is_some(), "the last watcher is live");
        assert!(
            g_b > g_start && g_c > g_b,
            "each switch advances the generation ({g_start} -> {g_b} -> {g_c})"
        );
    }
}
