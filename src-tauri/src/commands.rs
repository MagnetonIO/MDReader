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

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// (Commands read `epoch.root` off their captured epoch; this is a test
    /// convenience for asserting the active grant.)
    #[cfg(test)]
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
/// Captures the epoch once and re-checks it is current before returning, so a
/// switch during the read never hands back a previous workspace's bytes.
#[tauri::command]
#[specta::specta]
pub fn read_doc(
    path: String,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<DocPayload, String> {
    read_doc_guarded(&state, &path, || {})
}

/// The read core, with a `gate` hook invoked between the read and the identity
/// re-check (a no-op in production; tests inject a switch into that window).
fn read_doc_guarded(
    state: &WorkspaceState,
    path: &str,
    gate: impl FnOnce(),
) -> Result<DocPayload, String> {
    // Capture the epoch ONCE; validate + read against its grant.
    let epoch = state
        .current()
        .ok_or_else(|| "no workspace has been opened".to_string())?;
    // TOCTOU: the read below runs after this containment check (accepted — see the
    // module-level "TOCTOU" note; local single-user reader).
    let real = resolve_doc(&epoch.root, Path::new(path))?;

    let bytes = std::fs::read(&real).map_err(|err| format!("read {path}: {err}"))?;
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let mtime = mtime_millis(
        std::fs::metadata(&real)
            .ok()
            .and_then(|m| m.modified().ok()),
    );

    let payload = DocPayload {
        content: String::from_utf8_lossy(&bytes).into_owned(),
        mtime,
        hash,
    };

    gate();
    // EPOCH-GUARDED: a switch during the read supersedes this epoch — discard the
    // (old-root) bytes rather than surfacing them under the new workspace.
    epoch
        .finish(state, payload)
        .map_err(|_| crate::engine::StaleEpoch::message())
}

// --------------------------------------------------------------------------
// save_doc
// --------------------------------------------------------------------------

/// The outcome of a [`save_doc`]. A tagged struct (mirroring [`crate::engine::WikiResolution`]'s
/// shape rather than a serde-tagged Rust enum, which specta emits less ergonomically):
/// `status == "saved"` carries the fresh provenance the buffer re-anchors to;
/// `status == "conflict"` carries the on-disk hash the file *actually* has, and
/// guarantees NOTHING was written. A conflict is an expected outcome, not an
/// error, so it rides the `Ok` channel — only hard failures (containment, IO)
/// surface as `Err(String)`.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SaveOutcome {
    /// `"saved"` (the write landed) or `"conflict"` (the on-disk hash did not
    /// match `expected_hash`; the file was left untouched).
    pub status: String,
    /// blake3 hex of the freshly-written bytes — the anchor the buffer adopts so
    /// the watcher's echo of this very write is recognised as a self-write
    /// (`saved` only).
    pub new_hash: Option<String>,
    /// Modification time of the freshly-written file, ms since the Unix epoch
    /// (`saved` only).
    pub mtime: Option<f64>,
    /// The hash the file *currently* holds on disk when `status == "conflict"` —
    /// the anchor a subsequent forced overwrite (or a reload) re-bases onto.
    pub current_hash: Option<String>,
}

impl SaveOutcome {
    fn saved(new_hash: String, mtime: f64) -> Self {
        Self {
            status: "saved".to_string(),
            new_hash: Some(new_hash),
            mtime: Some(mtime),
            current_hash: None,
        }
    }

    fn conflict(current_hash: String) -> Self {
        Self {
            status: "conflict".to_string(),
            new_hash: None,
            mtime: None,
            current_hash: Some(current_hash),
        }
    }
}

/// A monotonic counter making each staged temp filename unique within a process.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// A unique, hidden temp filename for staging a save inside the destination's own
/// directory. Combines pid + a nanosecond stamp + a process-local counter so two
/// concurrent saves (or a save racing a previous save's orphaned temp) never
/// collide.
fn temp_filename() -> String {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_nanos())
        .unwrap_or(0);
    format!(".mdreader-save-{}-{nanos}-{seq}.tmp", std::process::id())
}

/// Stage `bytes` into a fresh temp file in `dir` (a sibling of the destination,
/// so the follow-up rename is a same-filesystem atomic swap), fsync it durably,
/// and return the temp path. Split out from the rename so the "crash before the
/// rename" case is directly testable: the original stays intact while only the
/// temp is present.
fn stage_temp(dir: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let tmp = dir.join(temp_filename());
    let mut file = std::fs::File::create(&tmp).map_err(|err| format!("stage temp file: {err}"))?;
    file.write_all(bytes)
        .map_err(|err| format!("write temp file: {err}"))?;
    // fsync the bytes before the rename so a power-loss after the rename can't
    // surface a renamed-but-empty file (the rename only publishes durable bytes).
    file.sync_all()
        .map_err(|err| format!("fsync temp file: {err}"))?;
    Ok(tmp)
}

/// Rename a staged temp over `dest` — the atomic half of a save. The rename is
/// atomic on a single filesystem, so a reader (or a crash) only ever sees the
/// whole old file or the whole new one, never a half-written document. A rename
/// failure removes the orphaned temp rather than leaking it.
fn commit_rename(tmp: &Path, dest: &Path) -> Result<(), String> {
    match std::fs::rename(tmp, dest) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_file(tmp);
            Err(format!("commit save for {}: {err}", dest.display()))
        }
    }
}

/// Reads `path`'s current bytes and returns their blake3 hex digest — the digest
/// [`read_doc`]/[`save_doc`] agree on.
fn hash_on_disk(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

/// Holds the exclusive advisory `flock` on a save target for the whole
/// check→write→swap window, releasing it on drop (mirrors the engine's import
/// lease). Opening a fresh handle and dropping it closes the fd, which alone
/// releases the lock; the explicit `unlock` matches the engine and is belt-and-
/// suspenders.
struct SaveLock(std::fs::File);

impl SaveLock {
    /// Open `target` and block until its exclusive advisory lock is held.
    fn acquire(target: &Path) -> Result<Self, String> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(target)
            .map_err(|err| format!("open {} for save: {err}", target.display()))?;
        fs2::FileExt::lock_exclusive(&file)
            .map_err(|err| format!("lock {} for save: {err}", target.display()))?;
        Ok(Self(file))
    }
}

impl Drop for SaveLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

/// Save `content` to a markdown document within the granted workspace under a
/// hash guard — the enforcement of the "never silently rewrite files" rule.
/// Resolves + validates the path exactly like [`read_doc`] (contained, an
/// existing file, `.md`/`.markdown` only), then — holding an exclusive advisory
/// lock on the target — reads its CURRENT bytes and compares their hash to
/// `expected_hash`. On a mismatch it writes NOTHING and returns a typed
/// [`SaveOutcome`] conflict carrying the real on-disk hash, so the UI can raise
/// its non-blocking banner instead of clobbering an external edit. On a match
/// (or when `force` overrides the guard — the user's explicit "overwrite the
/// on-disk changes") it writes atomically (temp + rename) and returns the fresh
/// hash + mtime. Epoch-guarded: a switch mid-save discards the returned outcome
/// (the write itself belongs to the still-correct old-workspace path), so the
/// superseding workspace never adopts a previous doc's provenance.
#[tauri::command]
#[specta::specta]
pub fn save_doc(
    path: String,
    content: String,
    expected_hash: String,
    force: bool,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<SaveOutcome, String> {
    save_doc_guarded(&state, &path, &content, &expected_hash, force, || {}, || {})
}

/// The save core, with two test hooks (both no-ops in production):
/// `before_rename` fires UNDER the lock after the hash check + temp staging but
/// before the swap (a test injects an interleaved external write there); and
/// `before_finish` fires just before the epoch identity re-check (a test injects
/// a workspace switch there).
fn save_doc_guarded(
    state: &WorkspaceState,
    path: &str,
    content: &str,
    expected_hash: &str,
    force: bool,
    before_rename: impl FnOnce(),
    before_finish: impl FnOnce(),
) -> Result<SaveOutcome, String> {
    // Capture the epoch ONCE; validate + guard + write against its grant.
    let epoch = state
        .current()
        .ok_or_else(|| "no workspace has been opened".to_string())?;
    // TOCTOU: the read/write below run after this containment check (accepted —
    // see the module-level "TOCTOU" note; local single-user reader).
    let real = resolve_doc(&epoch.root, Path::new(path))?;

    // Serialize the ENTIRE check → temp-write → rename window against other
    // cooperating writers (a second `save_doc`, the engine) with an exclusive
    // advisory `flock` held on the target. A non-cooperating external writer can
    // still ignore an advisory lock, so we ALSO re-verify the on-disk hash under
    // the lock immediately before the swap — together these close the
    // compare-then-rename TOCTOU: a write that lands between our check and the
    // rename is caught and the swap is refused (the external bytes stay intact).
    let _lock = SaveLock::acquire(&real)?;

    // Hash guard (under the lock): unless force-overwriting, the file's current
    // bytes must still hash to what the buffer was anchored to.
    let current_hash = hash_on_disk(&real)?;
    if !force && current_hash != expected_hash {
        before_finish();
        // EPOCH-GUARDED
        return epoch
            .finish(state, SaveOutcome::conflict(current_hash))
            .map_err(|_| crate::engine::StaleEpoch::message());
    }

    // Stage the new bytes into a sibling temp (still holding the lock).
    let bytes = content.as_bytes();
    let new_hash = blake3::hash(bytes).to_hex().to_string();
    let dir = real
        .parent()
        .ok_or_else(|| format!("no parent directory for {}", real.display()))?;
    let tmp = stage_temp(dir, bytes)?;

    before_rename();

    // Re-verify under the lock, right before the swap: a write that raced in
    // between our check and now (a non-cooperating external writer) changes the
    // hash → refuse the rename so its content survives, and report the conflict.
    if !force {
        let now_hash = hash_on_disk(&real)?;
        if now_hash != current_hash {
            let _ = std::fs::remove_file(&tmp);
            before_finish();
            // EPOCH-GUARDED
            return epoch
                .finish(state, SaveOutcome::conflict(now_hash))
                .map_err(|_| crate::engine::StaleEpoch::message());
        }
    }

    commit_rename(&tmp, &real)?;
    let mtime = mtime_millis(
        std::fs::metadata(&real)
            .ok()
            .and_then(|meta| meta.modified().ok()),
    );
    let outcome = SaveOutcome::saved(new_hash, mtime);

    before_finish();
    // EPOCH-GUARDED: a switch during the save supersedes this epoch — the write
    // landed on the (still-correct) old-workspace path, but its provenance must
    // not be handed to the new workspace, so discard the outcome.
    epoch
        .finish(state, outcome)
        .map_err(|_| crate::engine::StaleEpoch::message())
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
    /// The workspace generation this watcher was armed under. The frontend
    /// discards events whose generation is not the current one, so a *previous*
    /// watcher's still-draining events can never drive the new workspace.
    pub generation: u32,
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
    /// The workspace generation this watcher was armed under (see
    /// [`FsChanged::generation`]). An *old* watcher's error must not mark the new
    /// watcher failed, so the frontend discards generation mismatches.
    pub generation: u32,
}

/// The canonical grant plus the generation the watcher was armed under, returned
/// by [`watch_workspace`]. The frontend adopts `generation` as the reference it
/// filters `mdreader://fs-changed` / `mdreader://watch-error` events against.
#[derive(Debug, Clone, Serialize, Type)]
pub struct WorkspaceGrant {
    /// Canonical granted root.
    pub root: String,
    /// The epoch generation this grant + watcher belong to.
    pub generation: u32,
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
) -> Result<WorkspaceGrant, String> {
    let emit_app = app.clone();
    // The generation the watcher is armed under — filled by `commit_workspace`
    // (under the lock, matching the new epoch exactly) and read by the emit
    // closure so every event is tagged with the era it belongs to.
    let armed_gen = Arc::new(AtomicU64::new(0));
    let gen_for_closure = Arc::clone(&armed_gen);
    let (granted, generation) = watch_and_commit(
        &root,
        &workspace,
        &state,
        &armed_gen,
        move |result: DebounceEventResult| {
            let generation = gen_for_closure.load(Ordering::Acquire) as u32;
            match result {
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
                            generation,
                        }
                        .emit(&emit_app);
                    }
                }
                Err(error) => {
                    let _ = WatchError {
                        message: error.to_string(),
                        generation,
                    }
                    .emit(&emit_app);
                }
            }
        },
    )?;
    Ok(WorkspaceGrant {
        root: granted.to_string_lossy().into_owned(),
        generation,
    })
}

/// The workspace-switch core, factored out of [`watch_workspace`] so it is
/// exercisable without an `AppHandle` (the handle is only needed to build the
/// production `on_event` sink; tests pass a no-op). Resolves `root`, arms a fresh
/// watcher into a *local*, and — only on success — commits the new grant + new
/// watcher together, filling `armed_gen` with the new epoch's generation. Any
/// earlier failure returns with nothing mutated. Returns the canonical root and
/// its generation.
fn watch_and_commit<F: DebounceEventHandler>(
    root: &str,
    workspace: &WorkspaceState,
    watcher: &WatcherState,
    armed_gen: &AtomicU64,
    on_event: F,
) -> Result<(PathBuf, u32), String> {
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

    let generation = commit_workspace(workspace, watcher, new_root.clone(), debouncer, armed_gen)?;
    Ok((new_root, generation))
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
    armed_gen: &AtomicU64,
) -> Result<u32, String> {
    let mut watcher_slot = watcher
        .0
        .lock()
        .map_err(|_| "watcher state poisoned".to_string())?;
    let mut epoch_slot = workspace
        .current
        .lock()
        .map_err(|_| "workspace state poisoned".to_string())?;
    // Both locks held — neither write below can fail, so the swap is atomic. The
    // watcher's armed generation is published to match the new epoch exactly, so
    // its events are tagged with the era they belong to.
    let generation = epoch_slot.as_ref().map_or(0, |epoch| epoch.generation) + 1;
    armed_gen.store(generation, Ordering::Release);
    *epoch_slot = Some(crate::engine::Epoch::new(new_root, generation));
    *watcher_slot = Some(debouncer);
    Ok(generation as u32)
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_dir, read_doc_guarded, resolve_dir, resolve_doc, resolve_within,
        save_doc_guarded, watch_and_commit, DebounceEventResult, WatcherState, WorkspaceState,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// blake3 hex of `bytes` — the same digest `read_doc`/`save_doc` compute, so a
    /// test can anchor an `expected_hash` to a file's real contents.
    fn hash_of(bytes: &[u8]) -> String {
        blake3::hash(bytes).to_hex().to_string()
    }

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
        watch_and_commit(
            root_a.to_str().unwrap(),
            &workspace,
            &watcher,
            &AtomicU64::new(0),
            sink(),
        )
        .expect("arm A");
        assert_eq!(workspace.root().unwrap(), root_a);
        assert!(watcher.0.lock().unwrap().is_some());

        // A valid switch to B swaps BOTH the grant and the watcher together.
        watch_and_commit(
            root_b.to_str().unwrap(),
            &workspace,
            &watcher,
            &AtomicU64::new(0),
            sink(),
        )
        .expect("switch B");
        assert_eq!(workspace.root().unwrap(), root_b);
        assert!(watcher.0.lock().unwrap().is_some());

        // A switch whose target cannot be resolved fails BEFORE commit (the same
        // pre-commit `?`-return path an arm failure would take), leaving the OLD
        // root + watcher fully intact — no half-applied switch, no divergence.
        let bogus = root_b.join("does-not-exist");
        assert!(watch_and_commit(
            bogus.to_str().unwrap(),
            &workspace,
            &watcher,
            &AtomicU64::new(0),
            sink()
        )
        .is_err());
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
        assert!(watch_and_commit(
            root_b.to_str().unwrap(),
            &workspace,
            &watcher,
            &AtomicU64::new(0),
            sink
        )
        .is_err());
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
        watch_and_commit(
            root_b.to_str().unwrap(),
            &workspace,
            &watcher,
            &AtomicU64::new(0),
            sink(),
        )
        .expect("switch B");
        let g_b = workspace.generation();
        watch_and_commit(
            root_c.to_str().unwrap(),
            &workspace,
            &watcher,
            &AtomicU64::new(0),
            sink(),
        )
        .expect("switch C");
        let g_c = workspace.generation();

        assert_eq!(
            workspace.root().unwrap(),
            root_c,
            "converged on the last root"
        );
        assert!(
            watcher.0.lock().unwrap().is_some(),
            "the last watcher is live"
        );
        assert!(
            g_b > g_start && g_c > g_b,
            "each switch advances the generation ({g_start} -> {g_b} -> {g_c})"
        );
    }

    #[test]
    fn the_watcher_is_armed_with_its_epochs_generation() {
        let (_a, root_a) = workspace();
        let (_b, root_b) = workspace();
        let ws = WorkspaceState::granted(root_a);
        let watcher = WatcherState(Mutex::new(None));
        let armed_gen = AtomicU64::new(0);
        let sink = |_result: DebounceEventResult| {};

        let (_root, generation) =
            watch_and_commit(root_b.to_str().unwrap(), &ws, &watcher, &armed_gen, sink)
                .expect("switch B");

        // The returned generation, the armed cell (read by the emit closure), and
        // the live epoch's generation all agree — so events are tagged with the
        // era they belong to.
        assert_eq!(generation as u64, ws.generation());
        assert_eq!(armed_gen.load(Ordering::Acquire), ws.generation());
        assert!(generation >= 2, "advanced past the initial grant");
    }

    #[test]
    fn read_doc_discards_bytes_when_a_switch_lands_in_the_read_window() {
        let (_a, root_a) = workspace();
        let (_b, root_b) = workspace();
        let note = root_a.join("note.md");
        fs::write(&note, "# Note\n").unwrap();
        let ws = WorkspaceState::granted(root_a);

        // No switch → the read of the current workspace's file succeeds.
        let ok = read_doc_guarded(&ws, note.to_str().unwrap(), || {}).expect("read");
        assert!(ok.content.contains("Note"));

        // A switch INSIDE the window (injected between the read and the re-check)
        // supersedes the captured epoch → the old-root bytes are discarded, never
        // surfaced under the new workspace.
        let stale = read_doc_guarded(&ws, note.to_str().unwrap(), || {
            let _ = ws.switch(root_b.clone());
        });
        assert!(stale.is_err(), "stale read discarded");
        assert_eq!(ws.root().unwrap(), root_b);
    }

    #[test]
    fn save_doc_writes_atomically_and_returns_the_new_provenance() {
        let (_dir, root) = workspace();
        let note = root.join("note.md");
        fs::write(&note, "# Old\n").unwrap();
        let ws = WorkspaceState::granted(root.clone());
        let expected = hash_of(b"# Old\n");

        let outcome = save_doc_guarded(
            &ws,
            note.to_str().unwrap(),
            "# New\n",
            &expected,
            false,
            || {},
            || {},
        )
        .expect("guarded hash matches → the save lands");

        assert_eq!(outcome.status, "saved");
        assert_eq!(
            outcome.new_hash.as_deref(),
            Some(hash_of(b"# New\n").as_str())
        );
        assert!(outcome.mtime.is_some());
        assert!(outcome.current_hash.is_none());
        // The bytes on disk are exactly the new content — the write is real.
        assert_eq!(fs::read(&note).unwrap(), b"# New\n");
    }

    #[test]
    fn save_doc_conflict_leaves_the_file_untouched() {
        let (_dir, root) = workspace();
        let note = root.join("note.md");
        fs::write(&note, "# On disk\n").unwrap();
        let ws = WorkspaceState::granted(root);

        // The buffer was anchored to a STALE hash (the file changed since it was
        // opened). The guard must refuse and report the file's ACTUAL hash.
        let stale_anchor = hash_of(b"# What the buffer thinks\n");
        let outcome = save_doc_guarded(
            &ws,
            note.to_str().unwrap(),
            "# My edit\n",
            &stale_anchor,
            false,
            || {},
            || {},
        )
        .expect("a conflict is an Ok outcome, not an error");

        assert_eq!(outcome.status, "conflict");
        assert_eq!(
            outcome.current_hash.as_deref(),
            Some(hash_of(b"# On disk\n").as_str())
        );
        assert!(outcome.new_hash.is_none());
        // NOTHING was written — the sacred rule holds.
        assert_eq!(fs::read(&note).unwrap(), b"# On disk\n");
    }

    #[test]
    fn save_doc_force_overwrites_despite_a_hash_mismatch() {
        let (_dir, root) = workspace();
        let note = root.join("note.md");
        fs::write(&note, "# On disk\n").unwrap();
        let ws = WorkspaceState::granted(root);

        // force = the user's explicit "overwrite the on-disk changes"; the guard
        // is bypassed and the write lands even though the anchor is stale.
        let outcome = save_doc_guarded(
            &ws,
            note.to_str().unwrap(),
            "# Forced\n",
            "an-utterly-wrong-hash",
            true,
            || {},
            || {},
        )
        .expect("force skips the guard");

        assert_eq!(outcome.status, "saved");
        assert_eq!(fs::read(&note).unwrap(), b"# Forced\n");
    }

    #[test]
    fn save_doc_rejects_paths_outside_the_workspace_and_non_markdown() {
        let (_dir, root) = workspace();
        let (_other, other_root) = workspace();
        let ws = WorkspaceState::granted(root.clone());

        // Outside the grant.
        let outside = other_root.join("outside.md");
        fs::write(&outside, "# secret\n").unwrap();
        let before = fs::read(&outside).unwrap();
        assert!(save_doc_guarded(
            &ws,
            outside.to_str().unwrap(),
            "# hijacked\n",
            &hash_of(&before),
            true, // even forced, containment must reject
            || {},
            || {},
        )
        .is_err());
        assert_eq!(
            fs::read(&outside).unwrap(),
            before,
            "no write escaped the grant"
        );

        // Contained but not markdown.
        let data = root.join("data.json");
        fs::write(&data, "{}").unwrap();
        assert!(save_doc_guarded(
            &ws,
            data.to_str().unwrap(),
            "hijacked",
            &hash_of(b"{}"),
            true,
            || {},
            || {},
        )
        .is_err());
    }

    #[test]
    fn a_crash_before_the_rename_leaves_the_original_intact_and_only_orphans_a_temp() {
        // Crash simulation driving the REAL save path: the save stages its bytes
        // into a sibling temp and only THEN renames. We panic in `before_rename`
        // (fires after the temp is staged, before the swap) to model a crash at
        // exactly that point, and assert the destination is untouched. This is
        // revert-breaking: a truncating in-place write would (a) leave no temp and
        // (b) have already corrupted the destination here.
        let (_dir, root) = workspace();
        let note = root.join("note.md");
        fs::write(&note, "# Original\n").unwrap();
        let ws = WorkspaceState::granted(root.clone());
        let expected = hash_of(b"# Original\n");
        let note_path = note.to_str().unwrap().to_string();

        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = save_doc_guarded(
                &ws,
                &note_path,
                "# New bytes that must never replace the original\n",
                &expected,
                false,
                || panic!("crash between temp-write and rename"),
                || {},
            );
        }));
        std::panic::set_hook(prev);
        assert!(
            result.is_err(),
            "the injected crash unwound out of the save"
        );

        // The destination is byte-for-byte the ORIGINAL — the rename never ran.
        assert_eq!(fs::read(&note).unwrap(), b"# Original\n");

        // Exactly one staged temp is orphaned beside it — proof the write went to a
        // SIBLING (temp + rename), not in-place.
        let temps = fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains(".mdreader-save-")
            })
            .count();
        assert_eq!(temps, 1, "the crash orphaned exactly the staged temp");
    }

    #[test]
    fn save_doc_discards_the_outcome_when_a_switch_lands_in_the_save_window() {
        let (_a, root_a) = workspace();
        let (_b, root_b) = workspace();
        let note = root_a.join("note.md");
        fs::write(&note, "# A\n").unwrap();
        let ws = WorkspaceState::granted(root_a);

        // No switch → the save of the current workspace's file succeeds.
        let ok = save_doc_guarded(
            &ws,
            note.to_str().unwrap(),
            "# A2\n",
            &hash_of(b"# A\n"),
            false,
            || {},
            || {},
        )
        .expect("save");
        assert_eq!(ok.status, "saved");

        // A switch INSIDE the window (injected at `before_finish`, just before the
        // re-check) supersedes the captured epoch → the outcome is discarded, so
        // the new workspace never adopts this doc's fresh provenance. The write
        // itself landed on the (still-correct) old-workspace path.
        let stale = save_doc_guarded(
            &ws,
            note.to_str().unwrap(),
            "# A3\n",
            &hash_of(b"# A2\n"),
            false,
            || {},
            || {
                let _ = ws.switch(root_b.clone());
            },
        );
        assert!(stale.is_err(), "stale save outcome discarded");
        assert_eq!(
            fs::read(&note).unwrap(),
            b"# A3\n",
            "but the write did land"
        );
    }

    #[test]
    fn save_doc_refuses_when_the_file_is_rewritten_between_the_check_and_the_rename() {
        // The compare-then-rename TOCTOU: the buffer's anchor matches at the initial
        // hash check, but a racing external writer lands NEW bytes AFTER the check
        // and BEFORE the swap (injected via the `before_rename` hook — a
        // non-cooperating writer that ignores the advisory lock). The re-verify
        // under the lock must catch it: conflict, and the external content survives
        // (our staged edit is never renamed over it).
        let (_dir, root) = workspace();
        let note = root.join("note.md");
        fs::write(&note, "# Original\n").unwrap();
        let ws = WorkspaceState::granted(root);
        let anchored = hash_of(b"# Original\n");

        let external = b"# Written by someone else\n".to_vec();
        let external_for_hook = external.clone();
        let note_for_hook = note.clone();

        let outcome = save_doc_guarded(
            &ws,
            note.to_str().unwrap(),
            "# My edit\n",
            &anchored,
            false,
            move || {
                // Interleaved external write, after our check, before our rename.
                fs::write(&note_for_hook, &external_for_hook).unwrap();
            },
            || {},
        )
        .expect("a race is reported as a conflict, not an error");

        assert_eq!(outcome.status, "conflict");
        assert_eq!(
            outcome.current_hash.as_deref(),
            Some(hash_of(&external).as_str()),
            "the conflict reports the racing writer's actual on-disk hash"
        );
        assert!(outcome.new_hash.is_none());
        // The external content is intact — our edit never clobbered it.
        assert_eq!(fs::read(&note).unwrap(), external);
    }

    /// Structural guard (crude, but CI-catching): EVERY `#[tauri::command]` that
    /// returns workspace-derived data must funnel its result through the
    /// `Epoch::finish` choke-point. A new command that forgets it trips this — the
    /// barrier is enforced by construction, not by reviewer vigilance.
    #[test]
    fn every_workspace_command_funnels_through_the_epoch_choke_point() {
        // The barrier-creators and the non-workspace commands are exempt.
        const EXEMPT: &[&str] = &[
            "ping",                  // no workspace data
            "set_workspace",         // CREATES the epoch (the barrier itself)
            "pick_workspace_folder", // only returns a picked path
            "watch_workspace",       // CREATES the epoch + arms the watcher
        ];
        // Built at runtime so this literal never appears in the scanned source
        // (which would self-match); the test module is excluded regardless.
        let marker = format!("#[tauri::{}]", "command");
        for source in [
            include_str!("commands.rs"),
            include_str!("engine.rs"),
            include_str!("search.rs"),
        ] {
            // Scan only the non-test code (exclude the test module's string
            // literals, which would confuse the crude split).
            let code = source.split("\nmod tests").next().unwrap_or(source);
            for block in code.split(marker.as_str()).skip(1) {
                let name = block
                    .split_once("fn ")
                    .and_then(|(_, rest)| rest.split_once('('))
                    .map(|(name, _)| name.trim())
                    .unwrap_or("");
                if EXEMPT.contains(&name) {
                    continue;
                }
                assert!(
                    block.contains(".finish("),
                    "command `{name}` returns workspace data but never reaches the \
                     Epoch::finish choke-point — route its result through it (// EPOCH-GUARDED)"
                );
            }
        }
    }

    /// Tighter than the choke-point scan above (which is satisfied merely by the
    /// guarded core sitting in the same block): the PUBLIC `save_doc` command must
    /// do none of the guard/write itself — it may only delegate to
    /// `save_doc_guarded`, where the flock, the hash guard, and the atomic write
    /// live. This closes the "add a bypass path to the thin wrapper" hole: inlining
    /// any of the destructive work into the command trips this test.
    #[test]
    fn save_doc_command_only_delegates_to_its_guarded_core() {
        let src = include_str!("commands.rs");
        let start = src
            .find("pub fn save_doc(")
            .expect("the save_doc command exists");
        let rest = &src[start..];
        // The wrapper body is everything up to the first `\n}` that closes it (its
        // one-line delegation has only balanced inline `|| {}` braces before that).
        let end = rest.find("\n}\n").map(|i| i + 2).expect("wrapper closes");
        let wrapper = &rest[..end];

        assert!(
            wrapper.contains("save_doc_guarded("),
            "save_doc must route through save_doc_guarded"
        );
        for forbidden in [
            "SaveLock",
            "stage_temp",
            "commit_rename",
            "std::fs::",
            "blake3::hash",
        ] {
            assert!(
                !wrapper.contains(forbidden),
                "the save_doc command must not perform `{forbidden}` itself — the flock + \
                 guard + write belong ONLY in save_doc_guarded"
            );
        }
    }
}
