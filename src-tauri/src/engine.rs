//! The *engine* half of the command surface: the embedded CatDB index the shell
//! reads its sources, live import progress, and wikilink resolution from.
//!
//! ## What this owns (and what it deliberately does not)
//!
//! This module wires the shell to `catdb-engine`'s **existing** public API — it
//! never reaches into the engine's internals (a parallel lane owns those). It
//! opens (or creates) one [`EmbeddedCatDb`] per granted workspace, runs the
//! MDReader ingest pipeline (`mdreader-ingest`'s registered domain) in the
//! background, streams coarse progress to the StatusBar, and answers two reads:
//! the file listing (index-backed once an import finishes, live fs scan before
//! then) and wikilink resolution (filename then title match, ambiguity → the
//! candidate set).
//!
//! ## Where the database lives
//!
//! Not inside the user's notes. Each workspace's index is a private SQLite file
//! at `<app-data-dir>/workspaces/<blake3(canonical root)>/catdb.db`, so a corpus
//! is never littered with an engine sidecar and two aliases of one folder share
//! one index. [`read_doc`](crate::commands::read_doc) still reads straight off
//! disk — the index is for *finding*, never the source of truth for *content*.
//!
//! ## Embedder (fastembed by default)
//!
//! The app builds the real semantic embedder — [`FastembedEmbedder`]
//! (`bge-small-en-v1.5`, 384-d, via ONNX Runtime) — so an index is search-ready
//! and the first-run model download is a visible, one-time cost. Construction is
//! blocking (it deserializes the ONNX graph, downloading weights on first use),
//! so it runs off the command thread via [`FastembedEmbedder::load`] and is
//! surfaced as a distinct `"loading"` import phase before scanning begins.
//!
//! The deterministic [`HashEmbedder`] is used **only** when the
//! `MDREADER_DEV_HASH_EMBED` environment variable is set — an explicit dev/CI
//! escape hatch that keeps imports offline and instant (no model download).
//! Unit/e2e tests run under that flag (or drive the engine directly with a
//! `HashEmbedder`), so CI never downloads a model.
//!
//! ## Progress granularity (honest)
//!
//! `catdb-engine`'s `import` is one atomic call — it exposes no per-file
//! callback — so progress is coarse by construction: a `"loading"` phase covers
//! model construction, a pre-scan yields the file *total*, the StatusBar spins
//! through the `importing` phase, and the final report fills in files/chunks on
//! `done` (or the message on `error`). Finer streaming would need engine
//! changes, which are out of this lane's scope.
//!
//! ## Workspace epoch (the revocation barrier)
//!
//! Switching workspaces is a *revocation barrier*, not a set of independent
//! field updates. A workspace is one immutable [`Epoch`] — its canonical root,
//! generation, engine, per-epoch import status, and coalescing flag — held as an
//! `Arc<Epoch>` behind a single lock in [`WorkspaceState`]. A switch **creates a
//! new epoch and swaps it in**; that swap is the barrier.
//!
//! Every command captures the current `Arc<Epoch>` **once** at entry, so root,
//! generation, engine, and status always travel together — never sampled at
//! different instants (the race this design closes). At each commit point
//! (publishing status, installing the engine after the async model load,
//! returning a listing/resolution) it re-checks [`WorkspaceState::is_current`]
//! (`Arc::ptr_eq`); a stale epoch discards its work silently-clean. An old
//! import's progress lands only in its **own** epoch's status slot, which
//! nobody reads once superseded — [`import_status`] reads the *current* epoch's
//! slot only. Foreign roots (outside the epoch's grant) are a hard error.

use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use catdb_embed::{Embedder, FastembedEmbedder, HashEmbedder};
use catdb_engine::{EmbeddedCatDb, EngineConfig, ImportOptions, ImportReport};
use catdb_storage::DocumentRow;
use mdreader_ingest::{registration, scan_root, ScanOptions};

use crate::commands::{
    canonical_dir, mtime_millis, resolve_dir, resolve_within, FileEntry, WorkspaceState,
};

/// Default per-file size ceiling the ingest scan honors (mirrors the engine's
/// `ImportOptions` default of 2 MiB); the fs-fallback listing uses the same
/// bound so the two views agree on what counts as a document.
const MAX_FILE_SIZE: u64 = 2 * 1024 * 1024;

/// Deadline for the one-time embedding-model construction (download + ONNX
/// graph deserialize). A never-finishing download must surface a timeout error
/// the user can retry, not hang the import forever.
const MODEL_LOAD_DEADLINE: Duration = Duration::from_secs(120);

// --------------------------------------------------------------------------
// State — the workspace epoch
// --------------------------------------------------------------------------

/// One immutable workspace era. Everything scoped to a granted workspace lives
/// here so a command that captures an `Arc<Epoch>` at entry holds a *consistent*
/// snapshot (root + generation + engine + status), and a switch — which swaps in
/// a fresh epoch — is a single atomic revocation barrier. Held as `Arc<Epoch>`
/// so identity comparison (`Arc::ptr_eq`) is the "still current?" test.
pub struct Epoch {
    /// Canonical granted root (the workspace the engine keys and imports on).
    pub(crate) root: PathBuf,
    /// Monotonic era number (advances on every switch); for logs/observability —
    /// `Arc::ptr_eq` is the authoritative identity.
    pub(crate) generation: u64,
    /// The opened engine, installed once (after the async model load). Per-epoch,
    /// so a stale epoch installing its engine can never leak into the current one.
    engine: OnceLock<Arc<EmbeddedCatDb>>,
    /// This epoch's import status. A superseded import writes only here — nobody
    /// reads a stale epoch's slot ([`import_status`] reads the current epoch's).
    status: Mutex<ImportProgress>,
    /// Coalescing flag: whether an import for THIS epoch is already in flight.
    active: Mutex<bool>,
}

impl Epoch {
    /// A fresh epoch for `root` at `generation`, with no engine yet and an idle
    /// status.
    pub(crate) fn new(root: PathBuf, generation: u64) -> Arc<Self> {
        Arc::new(Self {
            root,
            generation,
            engine: OnceLock::new(),
            status: Mutex::new(ImportProgress::default()),
            active: Mutex::new(false),
        })
    }

    /// This epoch's current import status.
    fn status(&self) -> ImportProgress {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default()
    }

    /// The installed engine, if the model has finished loading for this epoch.
    #[cfg(test)]
    fn has_engine(&self) -> bool {
        self.engine.get().is_some()
    }

    /// Test helper: install `db` as this epoch's engine (simulates the post-load
    /// engine install).
    #[cfg(test)]
    fn install_engine_for_test(&self, db: Arc<EmbeddedCatDb>) {
        let _ = self.engine.set(db);
    }
}

/// Records `progress` into `epoch`'s OWN status slot (always — it is per-epoch,
/// so a stale epoch's write is invisible to [`import_status`]).
fn record_status(epoch: &Epoch, progress: &ImportProgress) {
    if let Ok(mut slot) = epoch.status.lock() {
        *slot = progress.clone();
    }
}

/// The current epoch's import status, or `idle` when no workspace is open.
fn current_status(workspace: &WorkspaceState) -> ImportProgress {
    workspace
        .current()
        .map(|epoch| epoch.status())
        .unwrap_or_default()
}

/// Try to claim `epoch`'s in-flight-import slot. `true` → the caller owns the
/// import; `false` → an import for this epoch is already running (coalesce onto
/// it and return the live status).
fn claim_active(epoch: &Epoch) -> Result<bool, String> {
    let mut active = epoch
        .active
        .lock()
        .map_err(|_| "engine state poisoned".to_string())?;
    if *active {
        return Ok(false);
    }
    *active = true;
    Ok(true)
}

/// Releases `epoch`'s active-import claim on drop. Per-epoch, so no cross-epoch
/// bookkeeping is needed — a superseded epoch's flag is simply never read again.
struct ActiveGuard {
    epoch: Arc<Epoch>,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = self.epoch.active.lock() {
            *active = false;
        }
    }
}

// --------------------------------------------------------------------------
// Wire types
// --------------------------------------------------------------------------

/// A snapshot of index-build progress: the payload streamed on the import
/// [`Channel`] *and* the value [`import_status`] returns. Deliberately coarse —
/// see the module docs on progress granularity.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ImportProgress {
    /// `"idle"` (no import yet), `"loading"` (constructing/downloading the
    /// embedding model), `"importing"` (in flight), `"done"`, or `"error"`.
    pub phase: String,
    /// Files processed so far (equals `files_total` on `done`; the atomic import
    /// gives no intermediate count).
    pub files_done: u32,
    /// Files the pre-scan found (the denominator the spinner shows).
    pub files_total: u32,
    /// Chunks embedded in this pass (0 when a re-import finds nothing changed).
    pub chunks_embedded: u32,
    /// The failure message when `phase == "error"`.
    pub error: Option<String>,
}

impl Default for ImportProgress {
    fn default() -> Self {
        Self {
            phase: "idle".to_string(),
            files_done: 0,
            files_total: 0,
            chunks_embedded: 0,
            error: None,
        }
    }
}

impl ImportProgress {
    /// The model is being constructed (deserialized, and downloaded on first
    /// run). Published before the scan so the first-run download is visible.
    fn loading() -> Self {
        Self {
            phase: "loading".to_string(),
            files_done: 0,
            files_total: 0,
            chunks_embedded: 0,
            error: None,
        }
    }

    fn importing(files_total: u32) -> Self {
        Self {
            phase: "importing".to_string(),
            files_done: 0,
            files_total,
            chunks_embedded: 0,
            error: None,
        }
    }

    fn done(report: &ImportReport) -> Self {
        let files = report.files_scanned as u32;
        Self {
            phase: "done".to_string(),
            files_done: files,
            files_total: files,
            chunks_embedded: report.chunks_embedded as u32,
            error: None,
        }
    }

    fn failed(files_total: u32, message: String) -> Self {
        Self {
            phase: "error".to_string(),
            files_done: 0,
            files_total,
            chunks_embedded: 0,
            error: Some(message),
        }
    }

    fn is_done(&self) -> bool {
        self.phase == "done"
    }
}

/// A file listing plus which side produced it: `true` = engine index (post
/// import), `false` = the live filesystem fallback (before the first import
/// finishes). The StatusBar surfaces the distinction.
#[derive(Debug, Clone, Serialize, Type)]
pub struct WorkspaceListing {
    /// The rows the sidebar tree renders.
    pub entries: Vec<FileEntry>,
    /// Whether `entries` came from the engine's index.
    pub from_index: bool,
}

/// One candidate when a wikilink target is ambiguous.
#[derive(Debug, Clone, Serialize, Type)]
pub struct WikiCandidate {
    /// Absolute path to the candidate document.
    pub path: String,
    /// The candidate's file name.
    pub name: String,
}

/// The outcome of resolving a `[[wikilink]]` against the index.
#[derive(Debug, Clone, Serialize, Type)]
pub struct WikiResolution {
    /// `"resolved"` (exactly one match), `"ambiguous"` (several — see
    /// `candidates`), or `"unresolved"` (none).
    pub status: String,
    /// Absolute path of the resolved document (`resolved` only).
    pub path: Option<String>,
    /// File name of the resolved document (`resolved` only).
    pub name: Option<String>,
    /// 1-based source line of the requested heading within the resolved
    /// document, when a heading was given and matched.
    pub line: Option<u32>,
    /// The matches when `status == "ambiguous"`.
    pub candidates: Vec<WikiCandidate>,
}

impl WikiResolution {
    fn unresolved() -> Self {
        Self {
            status: "unresolved".to_string(),
            path: None,
            name: None,
            line: None,
            candidates: Vec::new(),
        }
    }

    fn resolved(path: String, name: String, line: Option<u32>) -> Self {
        Self {
            status: "resolved".to_string(),
            path: Some(path),
            name: Some(name),
            line,
            candidates: Vec::new(),
        }
    }

    fn ambiguous(candidates: Vec<WikiCandidate>) -> Self {
        Self {
            status: "ambiguous".to_string(),
            path: None,
            name: None,
            line: None,
            candidates,
        }
    }
}

// --------------------------------------------------------------------------
// Engine lifecycle helpers
// --------------------------------------------------------------------------

/// The private index path for `canonical_root` under `base` (the app data dir):
/// `base/workspaces/<blake3(root)>/catdb.db`. Pure so it is unit-testable and so
/// the same root always maps to the same file.
pub(crate) fn db_path_under(base: &Path, canonical_root: &Path) -> PathBuf {
    let hash = blake3::hash(canonical_root.to_string_lossy().as_bytes())
        .to_hex()
        .to_string();
    base.join("workspaces").join(hash).join("catdb.db")
}

/// Whether the deterministic [`HashEmbedder`] dev/CI escape hatch is engaged
/// (`MDREADER_DEV_HASH_EMBED` is present in the environment). Off by default, so
/// the shipping app builds the real semantic model.
fn dev_hash_embed() -> bool {
    std::env::var_os("MDREADER_DEV_HASH_EMBED").is_some()
}

/// Build the embedder for the app, off the command thread. Default: the local
/// fastembed model (`bge-small-en-v1.5`), constructed — and downloaded on first
/// run — via [`FastembedEmbedder::load`] inside `spawn_blocking`. Only when the
/// `MDREADER_DEV_HASH_EMBED` flag is set does it fall back to the deterministic
/// [`HashEmbedder`] (no model download; dev/CI only).
async fn build_embedder() -> Result<Arc<dyn Embedder>, String> {
    if dev_hash_embed() {
        return Ok(Arc::new(HashEmbedder::new()) as Arc<dyn Embedder>);
    }
    let embedder = FastembedEmbedder::load(None)
        .await
        .map_err(|err| format!("load embedding model: {err}"))?;
    Ok(Arc::new(embedder) as Arc<dyn Embedder>)
}

/// Open (creating if needed) the engine at `db_path`, composed with the MDReader
/// domain and `embedder`.
fn open_engine(db_path: &Path, embedder: Arc<dyn Embedder>) -> Result<Arc<EmbeddedCatDb>, String> {
    let mut config = EngineConfig::new(db_path, embedder).with_actor("mdreader-app");
    config.domains = vec![registration()];
    EmbeddedCatDb::open(config)
        .map(Arc::new)
        .map_err(|err| format!("open index: {err}"))
}

/// Run the (blocking, possibly downloading) embedder construction under a
/// deadline. A build that never finishes — e.g. a stalled first-run download —
/// resolves to a distinct, user-retryable timeout error instead of hanging the
/// import forever. Generic over the loader so it is testable with an injected
/// never-resolving future.
async fn load_embedder_with_deadline<F, Fut>(
    deadline: Duration,
    load: F,
) -> Result<Arc<dyn Embedder>, String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Arc<dyn Embedder>, String>>,
{
    match tokio::time::timeout(deadline, load()).await {
        Ok(result) => result,
        Err(_elapsed) => {
            Err("model download timed out — check your connection; retry from the status bar"
                .to_string())
        }
    }
}

/// Ensure `epoch`'s engine is installed, opening it (after the async, deadlined
/// model load — visible as `"loading"`) if this is the first import. Re-checks
/// the epoch is still current across the slow load: `Ok(Some(db))` to proceed,
/// `Ok(None)` when a switch superseded this epoch mid-load (abandon clean),
/// `Err` on a model/open failure.
async fn ensure_engine(
    epoch: &Arc<Epoch>,
    workspace: &WorkspaceState,
    app: &AppHandle,
    channel: &Channel<ImportProgress>,
) -> Result<Option<Arc<EmbeddedCatDb>>, String> {
    // Fast path: this epoch's engine is already installed.
    if let Some(db) = epoch.engine.get() {
        return Ok(Some(Arc::clone(db)));
    }

    let base = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("no app data dir: {err}"))?;
    let db_path = db_path_under(&base, &epoch.root);

    // Publish "loading" and build the model off the command thread, deadlined.
    publish(epoch, workspace, channel, ImportProgress::loading());
    let embedder = load_embedder_with_deadline(MODEL_LOAD_DEADLINE, build_embedder).await?;

    // Commit point: a switch during the (slow) load supersedes this epoch — drop
    // the freshly-built model and abandon rather than install into a dead era.
    if !workspace.is_current(epoch) {
        return Ok(None);
    }

    let db = open_engine(&db_path, embedder)?;
    // Install into THIS epoch's slot (idempotent; a coalesced racer that beat us
    // wins and we adopt its engine).
    match epoch.engine.set(Arc::clone(&db)) {
        Ok(()) => Ok(Some(db)),
        Err(_) => Ok(epoch.engine.get().map(Arc::clone)),
    }
}

/// Record `progress` into `epoch`'s own status slot and, while the epoch is
/// still current, stream it on the import channel (a closed channel is ignored —
/// the frontend also gets the final value from `start_import`'s return). A stale
/// epoch's status is written but never surfaced ([`import_status`] reads the
/// current epoch's slot only).
fn publish(
    epoch: &Arc<Epoch>,
    workspace: &WorkspaceState,
    channel: &Channel<ImportProgress>,
    progress: ImportProgress,
) {
    record_status(epoch, &progress);
    if workspace.is_current(epoch) {
        let _ = channel.send(progress);
    }
}

/// Validate an import `root` against `epoch`'s grant (containment). A foreign
/// path is a hard error; the engine always imports the epoch's own grant, never
/// an arbitrary caller-supplied path, so one workspace maps to exactly one index.
fn validate_import_root(epoch: &Epoch, root: &str) -> Result<(), String> {
    let candidate = canonical_dir(root)?;
    if !candidate.starts_with(&epoch.root) {
        return Err(format!("path escapes the workspace: {root}"));
    }
    Ok(())
}

/// Best-effort pre-scan for the file total shown while importing. A scan failure
/// (e.g. permissions) just yields an unknown total, never an import failure.
fn scan_count(root: &Path) -> u32 {
    scan_root(root, &scan_options(true))
        .map(|files| files.len() as u32)
        .unwrap_or(0)
}

fn scan_options(respect_gitignore: bool) -> ScanOptions {
    ScanOptions {
        include_mdx: false,
        max_file_size: MAX_FILE_SIZE,
        respect_gitignore,
    }
}

// --------------------------------------------------------------------------
// start_import / import_status
// --------------------------------------------------------------------------

/// Open (or reuse) the workspace index and run an incremental import, streaming
/// coarse progress on `on_event` (`loading` → `importing` → `done`/`error`) and
/// returning the final status.
/// Captures the current [`Epoch`] once at entry — root, generation, engine, and
/// status travel together. `root` is validated against the epoch's grant (a
/// foreign path is a hard error). Coalesced (#4): a second import for the same
/// epoch returns the live status instead of spawning a duplicate pass. At each
/// commit point (model install, every publish) it re-checks the epoch is still
/// current; a switch mid-import abandons silently-clean, its late status landing
/// only in its own (superseded, unread) slot. The frontend fires this without
/// awaiting (progress flows on the channel) but the resolved value is
/// authoritative, so a mocked IPC (which need not drive the channel) still lands
/// the StatusBar on `done`.
#[tauri::command]
#[specta::specta]
pub async fn start_import(
    app: AppHandle,
    root: String,
    on_event: Channel<ImportProgress>,
    workspace: State<'_, WorkspaceState>,
) -> Result<ImportProgress, String> {
    // Capture the epoch ONCE; validate the target against its grant.
    let epoch = workspace
        .current()
        .ok_or_else(|| "no workspace has been opened".to_string())?;
    validate_import_root(&epoch, &root)?;

    // Coalesce onto an in-flight import for this epoch (#4).
    if !claim_active(&epoch)? {
        return Ok(epoch.status());
    }
    let _guard = ActiveGuard {
        epoch: Arc::clone(&epoch),
    };

    let db = match ensure_engine(&epoch, &workspace, &app, &on_event).await {
        Ok(Some(db)) => db,
        // A switch superseded this epoch during the model load — abandon clean.
        Ok(None) => return Ok(epoch.status()),
        Err(err) => {
            // A model-load / open failure (incl. the load deadline) surfaces as an
            // `error` status the StatusBar shows (not a hard command error).
            let failed = ImportProgress::failed(0, err);
            publish(&epoch, &workspace, &on_event, failed.clone());
            return Ok(failed);
        }
    };

    let total = scan_count(&epoch.root);
    publish(&epoch, &workspace, &on_event, ImportProgress::importing(total));

    // The engine serializes imports internally (single-writer); a concurrent
    // call simply queues, so re-firing on a rapid re-grant is safe.
    let outcome = db.import(&epoch.root, ImportOptions::default()).await;
    let final_status = match outcome {
        Ok(report) => ImportProgress::done(&report),
        Err(err) => ImportProgress::failed(total, err.to_string()),
    };
    publish(&epoch, &workspace, &on_event, final_status.clone());
    Ok(final_status)
}

/// The latest import status for the CURRENT workspace epoch — for the StatusBar's
/// initial render and any poll. A superseded epoch's status is never read, so a
/// switch leaves this reading `idle` until the new epoch's import publishes.
#[tauri::command]
#[specta::specta]
pub fn import_status(workspace: State<'_, WorkspaceState>) -> Result<ImportProgress, String> {
    Ok(current_status(&workspace))
}

// --------------------------------------------------------------------------
// list_workspace_files (engine-backed, fs fallback)
// --------------------------------------------------------------------------

/// List the workspace's markdown files. Once an import has finished the rows come
/// from the engine's index (gitignored files already dropped, so none are dimmed);
/// before then — or if the index is somehow empty — a live filesystem scan fills
/// in, marking gitignored files so the tree can dim them. `root` must be within
/// the granted workspace.
#[tauri::command]
#[specta::specta]
pub fn list_workspace_files(
    root: String,
    workspace: State<'_, WorkspaceState>,
) -> Result<WorkspaceListing, String> {
    list_workspace_files_guarded(&workspace, &root, || {})
}

/// The listing core, with a `gate` hook invoked between building the listing and
/// the identity re-check (no-op in production; tests inject a switch there). Both
/// return paths — index-backed and fs-fallback — pass through the SAME re-check,
/// so neither can hand a previous workspace's listing to the new one.
fn list_workspace_files_guarded(
    workspace: &WorkspaceState,
    root: &str,
    gate: impl FnOnce(),
) -> Result<WorkspaceListing, String> {
    // Capture the epoch ONCE — root + engine + status are one consistent snapshot.
    let epoch = workspace
        .current()
        .ok_or_else(|| "no workspace has been opened".to_string())?;
    // TOCTOU: the reads below run after this containment check (accepted — see
    // the commands module's "TOCTOU" note; local single-user reader).
    let root_path = resolve_dir(&epoch.root, Path::new(root))?;

    // Serve from the index once THIS epoch's import has finished and its engine is
    // installed (both from the captured epoch, so no stale-engine window). Once
    // built the index is authoritative: report `from_index` even with zero docs
    // (an empty workspace), rather than masquerading as the fs fallback (#7).
    let listing = match (epoch.status().is_done(), epoch.engine.get()) {
        (true, Some(db)) => WorkspaceListing {
            entries: index_listing(db, &epoch.root, &root_path)?,
            from_index: true,
        },
        _ => WorkspaceListing {
            entries: fs_listing(&root_path)?,
            from_index: false,
        },
    };

    gate();
    // Commit point (both paths): a switch during the read supersedes this epoch —
    // discard rather than surface a previous workspace's listing.
    if !workspace.is_current(&epoch) {
        return Err("workspace changed during listing".to_string());
    }
    Ok(listing)
}

/// Build sidebar rows from the index's documents, filtered to those under
/// `req_root`. Absolute paths are reconstructed from the source root + stored
/// (root-relative) document path.
fn index_listing(
    db: &EmbeddedCatDb,
    src_root: &Path,
    req_root: &Path,
) -> Result<Vec<FileEntry>, String> {
    let docs = db
        .store()
        .list_documents()
        .map_err(|err| format!("list documents: {err}"))?;
    let mut entries = Vec::new();
    for doc in docs {
        let abs = src_root.join(&doc.file_path);
        if !abs.starts_with(req_root) {
            continue;
        }
        entries.push(file_entry(&abs, false));
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

/// The pre-import listing: every markdown file under `root`, with `ignored`
/// flagged for gitignored ones. Computed by diffing two `ignore`-crate scans —
/// the gitignore-respecting scan (the *kept* set) against the gitignore-off scan
/// (all files, hidden/`.contextignore`/oversize still excluded) — so a file is
/// `ignored` exactly when a gitignore rule dropped it.
fn fs_listing(root: &Path) -> Result<Vec<FileEntry>, String> {
    let kept: HashSet<String> = scan_root(root, &scan_options(true))
        .map_err(|err| format!("scan: {err}"))?
        .into_iter()
        .map(|file| file.rel)
        .collect();
    let all = scan_root(root, &scan_options(false)).map_err(|err| format!("scan: {err}"))?;

    let mut entries = Vec::new();
    for file in all {
        let ignored = !kept.contains(&file.rel);
        entries.push(file_entry(&file.abs, ignored));
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

/// Assemble one [`FileEntry`] from an absolute path, statting it best-effort.
fn file_entry(abs: &Path, ignored: bool) -> FileEntry {
    let meta = std::fs::metadata(abs).ok();
    FileEntry {
        path: abs.to_string_lossy().into_owned(),
        name: abs
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string(),
        dir: abs
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
            .unwrap_or_default(),
        mtime: mtime_millis(meta.as_ref().and_then(|m| m.modified().ok())),
        size: meta.map(|m| m.len() as f64).unwrap_or(0.0),
        ignored,
    }
}

// --------------------------------------------------------------------------
// resolve_wikilink
// --------------------------------------------------------------------------

/// Validate a wikilink resolution request against `epoch`'s grant: the origin
/// document `from_doc` (when given) must live inside the granted workspace — a
/// foreign origin is a hard error. `from_doc` is otherwise reserved for future
/// same-directory disambiguation.
fn wikilink_guard(epoch: &Epoch, from_doc: &str) -> Result<(), String> {
    if !from_doc.is_empty() {
        resolve_within(&epoch.root, Path::new(from_doc))?;
    }
    Ok(())
}

/// Resolve a `[[wikilink]]` against the index. `from_doc` is the absolute path of
/// the document the link sits in — it must live inside the granted workspace;
/// `target` is the raw link text; `heading` (optional) is the `#heading` suffix,
/// resolved to a source line in the target document. Matching is filename-first
/// (stem, case-insensitive), then document title; several matches yield
/// `ambiguous` with the candidate set. Captures the epoch once (engine + root
/// consistent) and re-checks it is current AT THE RETURN — after the heading
/// lookup and result construction — so a switch mid-resolution can never surface
/// a previous workspace's document.
#[tauri::command]
#[specta::specta]
pub fn resolve_wikilink(
    from_doc: String,
    target: String,
    heading: Option<String>,
    workspace: State<'_, WorkspaceState>,
) -> Result<WikiResolution, String> {
    resolve_wikilink_guarded(&workspace, &from_doc, &target, heading.as_deref(), || {})
}

/// The resolution core, with a `gate` hook invoked between constructing the
/// result and the identity re-check (no-op in production; tests inject a switch
/// there). The re-check sits at the RETURN, after the (DB-touching) heading
/// lookup and result construction — the true commit point.
fn resolve_wikilink_guarded(
    workspace: &WorkspaceState,
    from_doc: &str,
    target: &str,
    heading: Option<&str>,
    gate: impl FnOnce(),
) -> Result<WikiResolution, String> {
    let epoch = workspace
        .current()
        .ok_or_else(|| "no workspace has been opened".to_string())?;
    wikilink_guard(&epoch, from_doc)?;
    let Some(db) = epoch.engine.get() else {
        return Ok(WikiResolution::unresolved());
    };
    let src_root = &epoch.root;

    let normalized = normalize_wiki_target(target);
    if normalized.is_empty() {
        return Ok(WikiResolution::unresolved());
    }

    let docs = db
        .store()
        .list_documents()
        .map_err(|err| format!("list documents: {err}"))?;

    // Filename match first; only on a miss pay for per-document title lookups.
    let mut matches = filename_matches(&docs, &normalized);
    if matches.is_empty() {
        matches = title_matches(db, &docs, &normalized)?;
    }

    let resolution = match matches.as_slice() {
        [] => WikiResolution::unresolved(),
        [only] => {
            let doc = &docs[*only];
            let abs = src_root.join(&doc.file_path);
            let line = match heading {
                Some(heading) => heading_line(db, doc.id, heading)?,
                None => None,
            };
            WikiResolution::resolved(
                abs.to_string_lossy().into_owned(),
                basename(&doc.file_path),
                line,
            )
        }
        many => {
            let candidates = many
                .iter()
                .map(|index| {
                    let doc = &docs[*index];
                    let abs = src_root.join(&doc.file_path);
                    WikiCandidate {
                        path: abs.to_string_lossy().into_owned(),
                        name: basename(&doc.file_path),
                    }
                })
                .collect();
            WikiResolution::ambiguous(candidates)
        }
    };

    gate();
    // Commit point: a switch during resolution supersedes this epoch — return
    // unresolved (silently-clean) rather than a foreign workspace's document.
    if !workspace.is_current(&epoch) {
        return Ok(WikiResolution::unresolved());
    }
    Ok(resolution)
}

/// Strip a `#heading`/`?query` suffix, decode `%20`, and trim — the target key
/// used for matching.
fn normalize_wiki_target(target: &str) -> String {
    target
        .split(['#', '?'])
        .next()
        .unwrap_or("")
        .replace("%20", " ")
        .trim()
        .to_string()
}

/// The final path component's stem (no directory, no extension).
fn file_stem(rel: &str) -> &str {
    let name = rel.rsplit(['/', '\\']).next().unwrap_or(rel);
    name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name)
}

/// The final path component (with extension).
fn basename(rel: &str) -> String {
    rel.rsplit(['/', '\\']).next().unwrap_or(rel).to_string()
}

/// Indices of documents whose filename matches `target` (case-insensitive stem;
/// a `/`-bearing target also matches a full root-relative path, with `.md`/
/// `.markdown` completion). Pure over the document set for unit testing.
fn filename_matches(docs: &[DocumentRow], target: &str) -> Vec<usize> {
    let wanted = target.to_lowercase();
    let path_forms: Vec<String> = if target.contains('/') {
        vec![
            wanted.clone(),
            format!("{wanted}.md"),
            format!("{wanted}.markdown"),
        ]
    } else {
        Vec::new()
    };
    let mut out = Vec::new();
    for (index, doc) in docs.iter().enumerate() {
        let stem = file_stem(&doc.file_path).to_lowercase();
        let rel = doc.file_path.to_lowercase();
        if stem == wanted || path_forms.iter().any(|form| *form == rel) {
            out.push(index);
        }
    }
    out
}

/// Indices of documents whose title (their first section's outermost heading)
/// matches `target`, case-insensitively.
fn title_matches(
    db: &EmbeddedCatDb,
    docs: &[DocumentRow],
    target: &str,
) -> Result<Vec<usize>, String> {
    let wanted = target.to_lowercase();
    let mut out = Vec::new();
    for (index, doc) in docs.iter().enumerate() {
        if let Some(title) = title_of(db, doc.id)? {
            if title.to_lowercase() == wanted {
                out.push(index);
            }
        }
    }
    Ok(out)
}

/// A document's title: the outermost heading of the first section that actually
/// carries one. The ingestor deliberately stores a headingless preamble as the
/// FIRST section (empty `heading_path`), so `sections.first()` would miss the
/// title on any doc that opens with prose before its heading — skip it and take
/// the first heading-bearing section instead.
fn title_of(db: &EmbeddedCatDb, document_id: uuid::Uuid) -> Result<Option<String>, String> {
    let sections = db
        .store()
        .sections_for_document(document_id)
        .map_err(|err| format!("sections: {err}"))?;
    Ok(sections
        .iter()
        .find(|section| !section.heading_path.is_empty())
        .and_then(|section| section.heading_path.first().cloned()))
}

/// The 1-based start line of the section whose heading matches `heading`
/// (case-insensitive; the section's own heading, i.e. the last breadcrumb
/// element), or `None` when no section carries that heading.
fn heading_line(
    db: &EmbeddedCatDb,
    document_id: uuid::Uuid,
    heading: &str,
) -> Result<Option<u32>, String> {
    let wanted = heading.trim().to_lowercase();
    if wanted.is_empty() {
        return Ok(None);
    }
    let sections = db
        .store()
        .sections_for_document(document_id)
        .map_err(|err| format!("sections: {err}"))?;
    Ok(sections
        .iter()
        .find(|section| {
            section
                .heading_path
                .last()
                .is_some_and(|last| last.to_lowercase() == wanted)
        })
        .map(|section| section.start_line))
}

#[cfg(test)]
mod tests {
    use super::*;
    use catdb_storage::DocumentRow;
    use std::fs;
    use tempfile::TempDir;
    use uuid::Uuid;

    fn doc(path: &str) -> DocumentRow {
        DocumentRow {
            id: Uuid::new_v4(),
            file_path: path.to_string(),
            repo: None,
            content_hash: "h".to_string(),
        }
    }

    #[test]
    fn db_path_is_deterministic_and_isolated_from_the_corpus() {
        let base = Path::new("/data");
        let root = Path::new("/Users/me/notes");
        let a = db_path_under(base, root);
        let b = db_path_under(base, root);
        assert_eq!(a, b, "same root maps to the same db file");
        assert!(a.starts_with("/data/workspaces"));
        assert!(a.ends_with("catdb.db"));
        // A different root maps somewhere else — never inside the corpus.
        let other = db_path_under(base, Path::new("/Users/me/other"));
        assert_ne!(a, other);
        assert!(!a.starts_with(root));
    }

    #[test]
    fn normalize_strips_heading_query_and_decodes_spaces() {
        assert_eq!(normalize_wiki_target("Node Storage"), "Node Storage");
        assert_eq!(
            normalize_wiki_target("Node%20Storage#Intro"),
            "Node Storage"
        );
        assert_eq!(normalize_wiki_target("  page  "), "page");
        assert_eq!(normalize_wiki_target("#anchor-only"), "");
    }

    #[test]
    fn filename_match_is_stem_based_and_case_insensitive() {
        let docs = vec![doc("notes/Node-Storage.md"), doc("index.md")];
        assert_eq!(filename_matches(&docs, "node-storage"), vec![0]);
        assert_eq!(filename_matches(&docs, "INDEX"), vec![1]);
        assert!(filename_matches(&docs, "missing").is_empty());
    }

    #[test]
    fn filename_match_reports_every_duplicate_for_ambiguity() {
        let docs = vec![doc("a/page.md"), doc("b/page.md"), doc("c/other.md")];
        assert_eq!(filename_matches(&docs, "page"), vec![0, 1]);
    }

    #[test]
    fn filename_match_accepts_a_path_bearing_target() {
        let docs = vec![doc("notes/zettel/node-storage.md"), doc("node-storage.md")];
        // `notes/zettel/node-storage` completes to `.md` and matches only the
        // nested doc by path (the bare stem would match both).
        assert_eq!(
            filename_matches(&docs, "notes/zettel/node-storage"),
            vec![0]
        );
    }

    #[test]
    fn fs_listing_flags_gitignored_files_without_hiding_them() {
        let dir = TempDir::new().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        // A git repo so .gitignore applies.
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".gitignore"), "generated/\n").unwrap();
        fs::write(root.join("keep.md"), "# Keep").unwrap();
        fs::create_dir_all(root.join("generated")).unwrap();
        fs::write(root.join("generated/skip.md"), "# Skip").unwrap();

        let entries = fs_listing(&root).unwrap();
        let by_name = |name: &str| entries.iter().find(|e| e.name == name).cloned();

        let keep = by_name("keep.md").expect("keep.md listed");
        assert!(!keep.ignored, "tracked file is not dimmed");
        let skip = by_name("skip.md").expect("gitignored file still listed (dimmed)");
        assert!(skip.ignored, "gitignored file is flagged");
    }

    /// Full-path exercise: build an in-memory engine, import a tiny corpus, and
    /// verify the reads the wikilink command relies on (documents present, title
    /// and heading lookups resolve) against the REAL engine + ingest pipeline.
    #[test]
    fn engine_import_supports_listing_and_heading_lookup() {
        let corpus = TempDir::new().unwrap();
        let root = fs::canonicalize(corpus.path()).unwrap();
        fs::write(
            root.join("guide.md"),
            "# Guide\n\nIntro.\n\n## Deep Dive\n\nDetails here.\n",
        )
        .unwrap();
        fs::write(root.join("index.md"), "# Home\n\nSee [[Guide]].\n").unwrap();

        let embedder = Arc::new(HashEmbedder::new()) as Arc<dyn Embedder>;
        let mut config = EngineConfig::in_memory(embedder).with_actor("test");
        config.domains = vec![registration()];
        let db = EmbeddedCatDb::open(config).expect("open in-memory engine");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let report = rt
            .block_on(db.import(&root, ImportOptions::default()))
            .expect("import corpus");
        assert_eq!(report.files_scanned, 2);

        // Index listing reconstructs absolute paths under the source root.
        let entries = index_listing(&db, &root, &root).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| !e.ignored));

        // Filename + title + heading lookups the resolve command chains through.
        let docs = db.store().list_documents().unwrap();
        let guide = filename_matches(&docs, "guide");
        assert_eq!(guide.len(), 1, "wikilink [[Guide]] resolves to one doc");
        let guide_id = docs[guide[0]].id;
        assert_eq!(title_of(&db, guide_id).unwrap().as_deref(), Some("Guide"));
        // The `## Deep Dive` heading resolves to its source line.
        let line = heading_line(&db, guide_id, "Deep Dive").unwrap();
        assert_eq!(line, Some(5));
        assert_eq!(heading_line(&db, guide_id, "nope").unwrap(), None);
    }

    /// A canonicalized temp workspace root.
    fn workspace_dir() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        (dir, root)
    }

    /// A real in-memory engine over a hash embedder (no model download).
    fn in_memory_engine() -> Arc<EmbeddedCatDb> {
        let embedder = Arc::new(HashEmbedder::new()) as Arc<dyn Embedder>;
        let mut config = EngineConfig::in_memory(embedder).with_actor("test");
        config.domains = vec![registration()];
        Arc::new(EmbeddedCatDb::open(config).expect("open in-memory engine"))
    }

    #[test]
    fn import_root_rejects_a_foreign_path_and_accepts_the_grant() {
        let (_g, granted) = workspace_dir();
        let (_f, foreign) = workspace_dir();
        let epoch = Epoch::new(granted.clone(), 1);

        // The granted root is accepted, as is a contained subdirectory.
        assert!(validate_import_root(&epoch, granted.to_str().unwrap()).is_ok());
        let sub = granted.join("sub");
        fs::create_dir_all(&sub).unwrap();
        assert!(validate_import_root(&epoch, sub.to_str().unwrap()).is_ok());

        // A foreign root is a hard error.
        assert!(validate_import_root(&epoch, foreign.to_str().unwrap()).is_err());
    }

    #[test]
    fn wikilink_guard_rejects_a_foreign_origin_document() {
        let (_g, granted) = workspace_dir();
        let (_f, foreign) = workspace_dir();
        let epoch = Epoch::new(granted.clone(), 1);

        // An origin document inside the grant (and the empty origin) are accepted.
        let inside = granted.join("note.md");
        fs::write(&inside, "# Note").unwrap();
        assert!(wikilink_guard(&epoch, inside.to_str().unwrap()).is_ok());
        assert!(wikilink_guard(&epoch, "").is_ok());

        // An origin document outside the grant is rejected.
        let outside = foreign.join("secret.md");
        fs::write(&outside, "# Secret").unwrap();
        assert!(wikilink_guard(&epoch, outside.to_str().unwrap()).is_err());
    }

    #[test]
    fn a_switch_is_a_revocation_barrier_isolating_per_epoch_status() {
        let (_a, root_a) = workspace_dir();
        let (_b, root_b) = workspace_dir();
        let ws = WorkspaceState::default();

        // Grant A: capture epoch E1 exactly as an in-flight import would at entry.
        let e1 = ws.switch(root_a.clone()).unwrap();
        assert!(ws.is_current(&e1));
        assert_eq!(e1.root, root_a);

        // The import publishes into E1's OWN slot; import_status surfaces it.
        record_status(&e1, &ImportProgress::importing(3));
        assert_eq!(current_status(&ws).phase, "importing");

        // A concurrent switch to B swaps in E2 — the revocation barrier. Root,
        // generation, engine, and status move together; E1 is instantly stale.
        let e2 = ws.switch(root_b.clone()).unwrap();
        assert!(ws.is_current(&e2) && !ws.is_current(&e1));
        assert_eq!(e2.root, root_b);
        assert_eq!(e2.generation, e1.generation + 1);

        // The window "between status publishes": E1's late error lands ONLY in
        // its own (now unread) slot; the user-visible status is E2's (idle).
        record_status(&e1, &ImportProgress::failed(0, "late".to_string()));
        assert_eq!(e1.status().phase, "error");
        assert_eq!(current_status(&ws).phase, "idle");
    }

    #[test]
    fn a_stale_epochs_engine_install_does_not_leak_into_the_current_one() {
        let (_a, root_a) = workspace_dir();
        let (_b, root_b) = workspace_dir();
        let ws = WorkspaceState::default();
        let e1 = ws.switch(root_a).unwrap(); // captured at import entry
        let e2 = ws.switch(root_b).unwrap(); // switch lands → E1 superseded

        // The window "between model-load and engine-install": E1 finished loading
        // but a switch preceded the install. is_current(E1) is false, so the
        // import abandons; even an install would touch only E1's OWN field.
        assert!(!ws.is_current(&e1));
        e1.install_engine_for_test(in_memory_engine());
        assert!(e1.has_engine());
        assert!(
            !e2.has_engine(),
            "the current epoch has no engine from the stale install"
        );
    }

    #[test]
    fn active_import_is_coalesced_per_epoch() {
        let (_g, granted) = workspace_dir();
        let epoch = Epoch::new(granted, 1);

        // The first claim wins; a second for the SAME epoch coalesces (false →
        // the caller returns the live status instead of spawning a duplicate).
        assert!(claim_active(&epoch).unwrap());
        assert!(!claim_active(&epoch).unwrap());

        // Dropping the pass's guard frees the slot so a later run can re-claim.
        {
            let _guard = ActiveGuard {
                epoch: Arc::clone(&epoch),
            };
        }
        assert!(claim_active(&epoch).unwrap());
    }

    #[test]
    fn model_load_hits_its_deadline_and_reports_a_retryable_timeout() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        // A loader that never resolves trips the deadline into a retryable error.
        // (`Arc<dyn Embedder>` is not `Debug`, so match rather than `unwrap_err`.)
        let timed_out = rt.block_on(load_embedder_with_deadline(Duration::from_millis(20), || {
            std::future::pending::<Result<Arc<dyn Embedder>, String>>()
        }));
        match timed_out {
            Err(err) => assert!(err.contains("timed out"), "got: {err}"),
            Ok(_) => panic!("expected a model-load timeout"),
        }

        // A loader that resolves in time passes its result through unchanged.
        let ok = rt.block_on(load_embedder_with_deadline(Duration::from_secs(5), || async {
            Ok(Arc::new(HashEmbedder::new()) as Arc<dyn Embedder>)
        }));
        assert!(ok.is_ok());
    }

    #[test]
    fn title_resolves_through_a_headingless_preamble() {
        let corpus = TempDir::new().unwrap();
        let root = fs::canonicalize(corpus.path()).unwrap();
        // Prose BEFORE the first heading → the ingestor stores a headingless
        // preamble section first; the title must still resolve to the real
        // heading rather than to the (empty) preamble.
        fs::write(
            root.join("note.md"),
            "Intro prose before any heading.\n\n# Real Title\n\nBody.\n",
        )
        .unwrap();

        let db = in_memory_engine();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(db.import(&root, ImportOptions::default()))
            .expect("import corpus");

        let docs = db.store().list_documents().unwrap();
        let id = docs[0].id;
        assert_eq!(title_of(&db, id).unwrap().as_deref(), Some("Real Title"));
        // The title-based wikilink match finds it too (filename stem is `note`).
        assert_eq!(title_matches(&db, &docs, "real title").unwrap().len(), 1);
    }

    #[test]
    fn listing_discards_when_a_switch_lands_in_the_window() {
        let (_a, root_a) = workspace_dir();
        let (_b, root_b) = workspace_dir();
        fs::write(root_a.join("keep.md"), "# Keep").unwrap();
        let ws = WorkspaceState::granted(root_a.clone());

        // No switch → the fs-fallback listing returns normally.
        let listing = list_workspace_files_guarded(&ws, root_a.to_str().unwrap(), || {}).unwrap();
        assert!(!listing.from_index);
        assert!(listing.entries.iter().any(|entry| entry.name == "keep.md"));

        // A switch injected between building the listing and the re-check
        // supersedes the captured epoch → the listing is discarded, never surfaced
        // under the new workspace.
        let stale = list_workspace_files_guarded(&ws, root_a.to_str().unwrap(), || {
            let _ = ws.switch(root_b.clone());
        });
        assert!(stale.is_err(), "stale listing discarded");
    }

    #[test]
    fn resolve_discards_when_a_switch_lands_in_the_window() {
        let corpus = TempDir::new().unwrap();
        let root_a = fs::canonicalize(corpus.path()).unwrap();
        fs::write(root_a.join("guide.md"), "# Guide\n").unwrap();
        let (_b, root_b) = workspace_dir();

        let ws = WorkspaceState::granted(root_a.clone());
        let db = in_memory_engine();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(db.import(&root_a, ImportOptions::default()))
            .expect("import corpus");
        ws.current().unwrap().install_engine_for_test(db);

        // No switch → the resolution (built after the DB heading lookup) returns.
        let ok = resolve_wikilink_guarded(&ws, "", "guide", None, || {}).unwrap();
        assert_eq!(ok.status, "resolved");

        // A switch injected after result construction, before the return re-check,
        // supersedes the captured epoch → unresolved, never a foreign document.
        let stale = resolve_wikilink_guarded(&ws, "", "guide", None, || {
            let _ = ws.switch(root_b.clone());
        })
        .unwrap();
        assert_eq!(stale.status, "unresolved");
    }
}
