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
//! ## Grant discipline
//!
//! Every engine command validates its target against the current canonical
//! [`WorkspaceState`] grant, exactly like the fs commands: an import root must be
//! the granted workspace (or contained in it), a wikilink's origin document must
//! live inside it, and the cached engine is **keyed to and re-verified against**
//! the active grant so a stale engine from a previous workspace can never answer.
//! Foreign roots are a hard error.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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

// --------------------------------------------------------------------------
// State
// --------------------------------------------------------------------------

/// The engine for the currently granted workspace, plus the last import status
/// and the in-flight-import handle. One workspace is active at a time (mirroring
/// [`WorkspaceState`]'s single grant); switching folders replaces the cached
/// engine and advances the root generation.
#[derive(Default)]
pub struct EngineState {
    inner: Mutex<EngineInner>,
    /// The last import status, tagged with the root generation it belongs to, so
    /// an old import's late status can never overwrite the new workspace's.
    status: Mutex<GenStatus>,
    /// The currently-running import (root + generation) for coalescing: a second
    /// `start_import` for the same root+generation returns the live status
    /// instead of spawning a duplicate pass.
    active: Mutex<ActiveImport>,
}

#[derive(Default)]
struct EngineInner {
    /// The opened engine, or `None` before the first `start_import`.
    engine: Option<Arc<EmbeddedCatDb>>,
    /// Canonical source root the cached engine imports from (always the grant).
    root: Option<PathBuf>,
}

/// The last import status plus the root generation it was published under.
struct GenStatus {
    generation: u64,
    progress: ImportProgress,
}

impl Default for GenStatus {
    fn default() -> Self {
        Self {
            generation: 0,
            progress: ImportProgress::default(),
        }
    }
}

/// The root + generation of the import currently in flight (if any).
#[derive(Default)]
struct ActiveImport {
    root: Option<PathBuf>,
    generation: u64,
}

/// Releases the active-import claim on drop — but only if the slot still names
/// this pass's root+generation (a newer switch may have re-claimed it, and that
/// claim must survive this pass unwinding).
struct ActiveGuard<'a> {
    engine: &'a EngineState,
    root: PathBuf,
    generation: u64,
}

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.engine.active.lock() {
            if active.generation == self.generation
                && active.root.as_deref() == Some(self.root.as_path())
            {
                active.root = None;
            }
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

/// Validate an import `root` against the active grant and return the canonical
/// grant the engine keys and imports on. The target must be the granted
/// workspace root (or contained within it); a foreign path — or no grant at all
/// — is a hard error. The engine is always keyed to the *grant*, never to an
/// arbitrary caller-supplied path, so one workspace maps to exactly one index.
fn import_root_for(workspace: &WorkspaceState, root: &str) -> Result<PathBuf, String> {
    let granted = workspace.root()?;
    let candidate = canonical_dir(root)?;
    if !candidate.starts_with(&granted) {
        return Err(format!("path escapes the workspace: {root}"));
    }
    Ok(granted)
}

/// Return the cached engine, opening (and caching) it for `granted` if the grant
/// changed. On a fresh open the embedder is constructed off the command thread,
/// with a `"loading"` phase published first so the first-run model download is
/// visible. `granted` must already be the validated canonical grant.
async fn ensure_engine(
    engine: &EngineState,
    workspace: &WorkspaceState,
    app: &AppHandle,
    granted: &Path,
    channel: &Channel<ImportProgress>,
    generation: u64,
) -> Result<Arc<EmbeddedCatDb>, String> {
    // Fast path: the engine is already open for this exact grant.
    {
        let inner = engine
            .inner
            .lock()
            .map_err(|_| "engine state poisoned".to_string())?;
        if let (Some(db), Some(cached)) = (&inner.engine, &inner.root) {
            if cached == granted {
                return Ok(Arc::clone(db));
            }
        }
    }

    // Slow path: build the model (visible as "loading") and open the index. No
    // state lock is held across the (blocking, possibly network) construction.
    let base = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("no app data dir: {err}"))?;
    let db_path = db_path_under(&base, granted);
    publish(engine, workspace, channel, generation, ImportProgress::loading());
    let embedder = build_embedder().await?;
    let db = open_engine(&db_path, embedder)?;

    let mut inner = engine
        .inner
        .lock()
        .map_err(|_| "engine state poisoned".to_string())?;
    inner.engine = Some(Arc::clone(&db));
    inner.root = Some(granted.to_path_buf());
    Ok(db)
}

/// Try to claim the active-import slot for `granted`+`generation`. Returns
/// `true` when the caller now owns the slot (it should run the import) or
/// `false` when an import for the same root+generation is already in flight (the
/// caller coalesces onto it — returning the live status — instead of spawning a
/// duplicate pass). A *different* root/generation supersedes the previous claim.
fn claim_active(engine: &EngineState, granted: &Path, generation: u64) -> Result<bool, String> {
    let mut active = engine
        .active
        .lock()
        .map_err(|_| "engine state poisoned".to_string())?;
    if active.generation == generation && active.root.as_deref() == Some(granted) {
        return Ok(false);
    }
    active.generation = generation;
    active.root = Some(granted.to_path_buf());
    Ok(true)
}

/// The current import status (for coalescing returns and polling).
fn current_status(engine: &EngineState) -> Result<ImportProgress, String> {
    engine
        .status
        .lock()
        .map(|status| status.progress.clone())
        .map_err(|_| "engine state poisoned".to_string())
}

/// The status for the *current* generation: the stored progress when it belongs
/// to the active grant, else `idle` (a switch has moved on and the stored status
/// is a previous workspace's).
fn status_for(engine: &EngineState, workspace: &WorkspaceState) -> Result<ImportProgress, String> {
    let current = workspace.generation();
    let status = engine
        .status
        .lock()
        .map_err(|_| "engine state poisoned".to_string())?;
    Ok(if status.generation == current {
        status.progress.clone()
    } else {
        ImportProgress::default()
    })
}

/// Record `progress` as the status for `generation` — UNLESS a switch has
/// advanced the grant past it, in which case this is a stale pass and its status
/// is discarded (never overwriting the new workspace's). Returns whether the
/// write landed.
fn record_status(
    engine: &EngineState,
    workspace: &WorkspaceState,
    generation: u64,
    progress: &ImportProgress,
) -> bool {
    if workspace.generation() != generation {
        return false;
    }
    if let Ok(mut status) = engine.status.lock() {
        status.generation = generation;
        status.progress = progress.clone();
    }
    true
}

/// Record `progress` for `generation` and, when it is not stale, stream it on
/// the import channel (a closed channel is ignored — the frontend also gets the
/// final value from `start_import`'s return).
fn publish(
    engine: &EngineState,
    workspace: &WorkspaceState,
    channel: &Channel<ImportProgress>,
    generation: u64,
    progress: ImportProgress,
) {
    if record_status(engine, workspace, generation, &progress) {
        let _ = channel.send(progress);
    }
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

/// Open (or reuse) the workspace index for the granted root and run an
/// incremental import — streaming coarse progress on `on_event` (`loading` →
/// `importing` → `done`/`error`) and returning the final status. `root` is
/// validated against the active grant (a foreign path is a hard error); the
/// index is keyed to the grant. Coalesced (#4): a second call for the same
/// root+generation returns the live status instead of spawning a duplicate pass.
/// Generation-scoped (#3): if a workspace switch supersedes this import, its
/// status is discarded rather than published over the new workspace's. The
/// frontend fires this without awaiting (progress flows on the channel) but the
/// resolved value is authoritative, so a mocked IPC (which need not drive the
/// channel) still lands the StatusBar on `done`.
#[tauri::command]
#[specta::specta]
pub async fn start_import(
    app: AppHandle,
    root: String,
    on_event: Channel<ImportProgress>,
    workspace: State<'_, WorkspaceState>,
    engine: State<'_, EngineState>,
) -> Result<ImportProgress, String> {
    let granted = import_root_for(&workspace, &root)?;
    let generation = workspace.generation();

    // Coalesce onto an in-flight import for the same root+generation (#4).
    if !claim_active(&engine, &granted, generation)? {
        return current_status(&engine);
    }
    let _guard = ActiveGuard {
        engine: &engine,
        root: granted.clone(),
        generation,
    };

    let db = match ensure_engine(&engine, &workspace, &app, &granted, &on_event, generation).await {
        Ok(db) => db,
        Err(err) => {
            // A model-load / open failure is reported as an `error` status (not a
            // hard command error — the StatusBar surfaces the message).
            let failed = ImportProgress::failed(0, err);
            publish(&engine, &workspace, &on_event, generation, failed.clone());
            return Ok(failed);
        }
    };

    let total = scan_count(&granted);
    publish(&engine, &workspace, &on_event, generation, ImportProgress::importing(total));

    // The engine serializes imports internally (single-writer); a concurrent
    // call simply queues, so re-firing on a rapid re-grant is safe.
    let outcome = db.import(&granted, ImportOptions::default()).await;
    let final_status = match outcome {
        Ok(report) => ImportProgress::done(&report),
        Err(err) => ImportProgress::failed(total, err.to_string()),
    };
    publish(&engine, &workspace, &on_event, generation, final_status.clone());
    Ok(final_status)
}

/// The latest import status for the current workspace generation — for the
/// StatusBar's initial render and any poll. A status left over from a previous
/// workspace (superseded by a switch) reads as `idle`.
#[tauri::command]
#[specta::specta]
pub fn import_status(
    workspace: State<'_, WorkspaceState>,
    engine: State<'_, EngineState>,
) -> Result<ImportProgress, String> {
    status_for(&engine, &workspace)
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
    engine: State<'_, EngineState>,
) -> Result<WorkspaceListing, String> {
    let granted = workspace.root()?;
    // TOCTOU: the reads below run after this containment check (accepted — see
    // the commands module's "TOCTOU" note; local single-user reader).
    let root_path = resolve_dir(&granted, Path::new(&root))?;

    // Serve from the index only when the CURRENT generation's import has finished
    // (a status left from a previous workspace does not count).
    let index_done = {
        let current = workspace.generation();
        engine
            .status
            .lock()
            .map(|status| status.generation == current && status.progress.is_done())
            .unwrap_or(false)
    };
    if index_done {
        // The cached engine may only answer when it is keyed to the ACTIVE grant
        // (never a stale engine carried over from a previous workspace).
        let cached = {
            let inner = engine
                .inner
                .lock()
                .map_err(|_| "engine state poisoned".to_string())?;
            match (&inner.engine, &inner.root) {
                (Some(engine), Some(src_root)) if *src_root == granted => {
                    Some((Arc::clone(engine), src_root.clone()))
                }
                _ => None,
            }
        };
        if let Some((db, src_root)) = cached {
            // Once the index is built it is authoritative: report its provenance
            // (`from_index`) even when it legitimately holds no documents (an
            // empty workspace), rather than masquerading as the fs fallback (#7).
            let entries = index_listing(&db, &src_root, &root_path)?;
            return Ok(WorkspaceListing {
                entries,
                from_index: true,
            });
        }
    }

    Ok(WorkspaceListing {
        entries: fs_listing(&root_path)?,
        from_index: false,
    })
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

/// Validate a wikilink resolution request against the active grant: the origin
/// document `from_doc` (when given) must live inside the granted workspace — a
/// foreign origin (or no grant at all) is a hard error. Returns the canonical
/// grant on success. `from_doc` is otherwise reserved for future same-directory
/// disambiguation.
fn wikilink_guard(workspace: &WorkspaceState, from_doc: &str) -> Result<PathBuf, String> {
    let granted = workspace.root()?;
    if !from_doc.is_empty() {
        resolve_within(&granted, Path::new(from_doc))?;
    }
    Ok(granted)
}

/// Resolve a `[[wikilink]]` against the index. `from_doc` is the absolute path of
/// the document the link sits in — it must live inside the granted workspace;
/// `target` is the raw link text; `heading` (optional) is the `#heading` suffix,
/// resolved to a source line in the target document. Matching is filename-first
/// (stem, case-insensitive), then document title; several matches yield
/// `ambiguous` with the candidate set. Only the CURRENT grant's engine may
/// answer (a stale engine from a previous workspace never resolves).
#[tauri::command]
#[specta::specta]
pub fn resolve_wikilink(
    from_doc: String,
    target: String,
    heading: Option<String>,
    workspace: State<'_, WorkspaceState>,
    engine: State<'_, EngineState>,
) -> Result<WikiResolution, String> {
    let granted = wikilink_guard(&workspace, &from_doc)?;
    let cached = {
        let inner = engine
            .inner
            .lock()
            .map_err(|_| "engine state poisoned".to_string())?;
        match (&inner.engine, &inner.root) {
            (Some(engine), Some(src_root)) if *src_root == granted => {
                Some((Arc::clone(engine), src_root.clone()))
            }
            _ => None,
        }
    };
    let Some((db, src_root)) = cached else {
        return Ok(WikiResolution::unresolved());
    };

    let normalized = normalize_wiki_target(&target);
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
        matches = title_matches(&db, &docs, &normalized)?;
    }

    match matches.as_slice() {
        [] => Ok(WikiResolution::unresolved()),
        [only] => {
            let doc = &docs[*only];
            let abs = src_root.join(&doc.file_path);
            let line = match &heading {
                Some(heading) => heading_line(&db, doc.id, heading)?,
                None => None,
            };
            Ok(WikiResolution::resolved(
                abs.to_string_lossy().into_owned(),
                basename(&doc.file_path),
                line,
            ))
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
            Ok(WikiResolution::ambiguous(candidates))
        }
    }
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

    #[test]
    fn import_root_rejects_a_foreign_path_and_accepts_the_grant() {
        let (_g, granted) = workspace_dir();
        let (_f, foreign) = workspace_dir();
        let ws = WorkspaceState::granted(granted.clone());

        // The granted root is accepted and keys the engine to the grant itself.
        assert_eq!(
            import_root_for(&ws, granted.to_str().unwrap()).unwrap(),
            granted
        );
        // A nested subdirectory is contained, so it is accepted — still keyed to
        // the grant (one workspace → one index).
        let sub = granted.join("sub");
        fs::create_dir_all(&sub).unwrap();
        assert_eq!(import_root_for(&ws, sub.to_str().unwrap()).unwrap(), granted);

        // A foreign root is a hard error, as is having no grant at all.
        assert!(import_root_for(&ws, foreign.to_str().unwrap()).is_err());
        assert!(import_root_for(&WorkspaceState::default(), granted.to_str().unwrap()).is_err());
    }

    #[test]
    fn wikilink_guard_rejects_a_foreign_origin_document() {
        let (_g, granted) = workspace_dir();
        let (_f, foreign) = workspace_dir();
        let ws = WorkspaceState::granted(granted.clone());

        // An origin document inside the grant (and the empty origin) are accepted.
        let inside = granted.join("note.md");
        fs::write(&inside, "# Note").unwrap();
        assert!(wikilink_guard(&ws, inside.to_str().unwrap()).is_ok());
        assert!(wikilink_guard(&ws, "").is_ok());

        // An origin document outside the grant is rejected, as is no grant.
        let outside = foreign.join("secret.md");
        fs::write(&outside, "# Secret").unwrap();
        assert!(wikilink_guard(&ws, outside.to_str().unwrap()).is_err());
        assert!(wikilink_guard(&WorkspaceState::default(), inside.to_str().unwrap()).is_err());
    }

    #[test]
    fn a_superseded_imports_late_status_is_discarded() {
        let (_g, granted) = workspace_dir();
        let ws = WorkspaceState::granted(granted);
        let engine = EngineState::default();
        let gen = ws.generation();

        // A current-generation publish lands and is what the status reports.
        assert!(record_status(&engine, &ws, gen, &ImportProgress::importing(3)));
        assert_eq!(status_for(&engine, &ws).unwrap().phase, "importing");

        // A switch advances the generation. The OLD import's late status is now
        // stale: record_status refuses it, and the leftover reads as idle for the
        // NEW workspace (never the previous workspace's progress).
        let (_g2, granted2) = workspace_dir();
        ws.advance_for_test(granted2);
        assert!(!record_status(&engine, &ws, gen, &ImportProgress::importing(9)));
        assert_eq!(status_for(&engine, &ws).unwrap().phase, "idle");

        // A fresh publish under the CURRENT generation lands normally.
        assert!(record_status(&engine, &ws, ws.generation(), &ImportProgress::importing(2)));
        assert_eq!(status_for(&engine, &ws).unwrap().phase, "importing");
    }

    #[test]
    fn active_import_is_coalesced_per_root_and_generation() {
        let (_g, granted) = workspace_dir();
        let engine = EngineState::default();

        // The first claim for (root, generation) wins; a second for the SAME
        // root+generation coalesces (returns false → caller reuses the live pass).
        assert!(claim_active(&engine, &granted, 1).unwrap());
        assert!(!claim_active(&engine, &granted, 1).unwrap());

        // A newer generation supersedes the previous claim.
        assert!(claim_active(&engine, &granted, 2).unwrap());

        // Dropping the pass's guard frees the slot so a later run can re-claim.
        {
            let _guard = ActiveGuard {
                engine: &engine,
                root: granted.clone(),
                generation: 2,
            };
        }
        assert!(claim_active(&engine, &granted, 2).unwrap());
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

        let embedder = Arc::new(HashEmbedder::new()) as Arc<dyn Embedder>;
        let mut config = EngineConfig::in_memory(embedder).with_actor("test");
        config.domains = vec![registration()];
        let db = EmbeddedCatDb::open(config).expect("open in-memory engine");
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
}
