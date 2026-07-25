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
//! ## Embedder (fastembed by default), and why it is NOT on the read path
//!
//! The app builds the real semantic embedder — [`FastembedEmbedder`]
//! (`bge-small-en-v1.5`, 384-d, via ONNX Runtime) — so an index is search-ready
//! and the first-run model download is a visible, one-time cost. Construction is
//! blocking (it deserializes the ONNX graph, downloading weights on first use),
//! so it runs off the command thread via [`FastembedEmbedder::load`] and is
//! surfaced as a distinct `"loading"` import phase before scanning begins.
//!
//! **Reading a workspace does not need vectors.** Opening the folder, listing
//! the tree, and reading a document are pure filesystem work; wikilink
//! resolution and the lexical (FTS) search arm are index reads that never touch
//! the embedder. Only *indexing* and the *semantic* search arm do. So the model
//! load is deliberately **not** a precondition for opening a workspace: the
//! index is opened first, through a [`DeferredEmbedder`] that knows the
//! configured model's identity (which is static) while its weights are still
//! loading — or never load at all. A model that cannot load therefore costs the
//! semantic arm and the index build, and nothing else; it is reported as the
//! distinct [`"model-unavailable"`](ImportProgress::model_unavailable) phase,
//! never as an empty workspace and never as a generic index failure.
//!
//! The deterministic [`HashEmbedder`] is used **only** when the
//! `MDREADER_DEV_HASH_EMBED` environment variable is set — an explicit dev/CI
//! escape hatch that keeps imports offline and instant (no model download).
//! Unit/e2e tests run under that flag (or drive the engine directly with a
//! `HashEmbedder`), so CI never downloads a model. It is **never** a fallback:
//! [`DeferredEmbedder::install`] refuses any model whose identity differs from
//! the configured one, so a failed fastembed load can never be quietly served by
//! test vectors.
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
//! One import is not like the others, though, and says so: when the workspace
//! was embedded by a *different* model than the one now configured, the engine
//! rotates the index — re-embedding the entire corpus, a one-time cost
//! proportional to the workspace rather than to what changed on disk. That pass
//! publishes the distinct `"reembedding"` phase (see
//! [`EmbeddedCatDb::pending_model_rotation`]) so the StatusBar can say what is
//! actually happening instead of an understated `Indexing N files…`.
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
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use catdb_embed::{EmbedError, EmbedResult, Embedder, FastembedEmbedder, HashEmbedder, ModelInfo};
use catdb_engine::{EmbeddedCatDb, EngineConfig, ImportOptions, ImportReport};
use catdb_storage::DocumentRow;
use mdreader_ingest::{registration_with_proposals, scan_root, ProposalStore, ScanOptions};

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
// The deferred embedder — what decouples opening a workspace from the model
// --------------------------------------------------------------------------

/// Whether the semantic (vector) modality can run right now, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Semantic {
    /// A model is installed: the vector arm can run.
    Ready,
    /// The model is still being constructed/downloaded.
    Loading,
    /// The model could not be loaded; the string is the reason, already phrased
    /// for a human ("the embedding model could not be loaded — …").
    Unavailable(String),
}

/// The state a [`DeferredEmbedder`] is in.
enum ModelSlot {
    Loading,
    Ready(Arc<dyn Embedder>),
    Failed(String),
}

/// An [`Embedder`] whose *identity* is known up front but whose *weights* may
/// arrive later — or never.
///
/// The app is configured for exactly one model, and that model's name and
/// dimensionality are static constants; only producing vectors requires the
/// downloaded weights. Splitting the two is what lets the index be opened (and
/// therefore read: wikilinks, lexical search) the moment a workspace is granted,
/// with the model installed afterwards. Because the identity handed to the
/// engine is the real model's, the index this opens is keyed exactly as it would
/// have been had the model loaded first — no placeholder model name ever reaches
/// the catalog's index versions.
///
/// It is **not** a fallback embedder. With no model installed, [`Embedder::embed`]
/// returns an error; it never invents a vector, and [`Self::install`] refuses any
/// model whose [`ModelInfo`] differs from the configured one — so the test-only
/// [`HashEmbedder`] cannot be slipped in behind the real model's name.
pub(crate) struct DeferredEmbedder {
    info: ModelInfo,
    slot: RwLock<ModelSlot>,
}

impl DeferredEmbedder {
    /// A deferred embedder for `info`, with no weights yet.
    pub(crate) fn loading(info: ModelInfo) -> Self {
        Self {
            info,
            slot: RwLock::new(ModelSlot::Loading),
        }
    }

    /// Adopt `model` as the loaded weights. Refused — and recorded as a failure —
    /// when `model` is not the model this embedder promised the engine, because
    /// installing it would mean vectors from one model being read out of an index
    /// keyed to another.
    pub(crate) fn install(&self, model: Arc<dyn Embedder>) -> Result<(), String> {
        if model.info() != self.info {
            let reason = format!(
                "loaded model `{}` ({}-d) is not the configured model `{}` ({}-d)",
                model.info().name,
                model.info().dim,
                self.info.name,
                self.info.dim
            );
            self.fail(reason.clone());
            return Err(reason);
        }
        if let Ok(mut slot) = self.slot.write() {
            *slot = ModelSlot::Ready(model);
        }
        Ok(())
    }

    /// Record that the model could not be loaded. A later [`Self::install`]
    /// still succeeds — a retry genuinely re-attempts the load.
    pub(crate) fn fail(&self, reason: String) {
        if let Ok(mut slot) = self.slot.write() {
            *slot = ModelSlot::Failed(reason);
        }
    }

    /// Whether the semantic modality can run, and if not, why.
    pub(crate) fn semantic(&self) -> Semantic {
        match self.slot.read() {
            Ok(slot) => match &*slot {
                ModelSlot::Ready(_) => Semantic::Ready,
                ModelSlot::Loading => Semantic::Loading,
                ModelSlot::Failed(reason) => Semantic::Unavailable(reason.clone()),
            },
            Err(_) => Semantic::Unavailable("embedding model state poisoned".to_string()),
        }
    }

    /// The installed model, if any (cloned out so no lock is held across an await).
    fn model(&self) -> Result<Arc<dyn Embedder>, String> {
        match self.slot.read() {
            Ok(slot) => match &*slot {
                ModelSlot::Ready(model) => Ok(Arc::clone(model)),
                ModelSlot::Loading => Err("the embedding model is still loading".to_string()),
                ModelSlot::Failed(reason) => Err(reason.clone()),
            },
            Err(_) => Err("embedding model state poisoned".to_string()),
        }
    }
}

#[async_trait]
impl Embedder for DeferredEmbedder {
    fn info(&self) -> ModelInfo {
        self.info.clone()
    }

    async fn embed(&self, texts: &[String]) -> EmbedResult<Vec<Vec<f32>>> {
        // The lock is released before the await: `model()` clones the handle out.
        let model = self.model().map_err(EmbedError::Init)?;
        model.embed(texts).await
    }
}

/// The identity of the model this app is configured for — known WITHOUT loading
/// it, which is exactly what lets a workspace open before (or without) the
/// weights. Mirrors [`build_embedder`]'s choice: the dev/CI hash embedder under
/// the flag, the real fastembed model otherwise.
fn configured_model_info() -> ModelInfo {
    model_info_for(dev_hash_embed())
}

/// The identity of the model each branch of [`build_embedder`] produces. Split
/// out from the environment read so both branches are testable without mutating
/// the process environment.
fn model_info_for(hash_embed: bool) -> ModelInfo {
    if hash_embed {
        return HashEmbedder::new().info();
    }
    ModelInfo {
        name: FastembedEmbedder::MODEL_NAME.to_string(),
        dim: FastembedEmbedder::DIM,
        max_tokens: FastembedEmbedder::MAX_TOKENS,
    }
}

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
    /// The opened engine, installed once — as soon as the *index* opens, which
    /// needs no model (see [`ensure_index`]). Per-epoch, so a stale epoch
    /// installing its engine can never leak into the current one.
    engine: OnceLock<Arc<EmbeddedCatDb>>,
    /// The [`DeferredEmbedder`] this epoch's engine was opened with, installed
    /// alongside it. The model is installed *into* it later (or never), so the
    /// engine handle never has to be replaced when the weights land — and the
    /// semantic modality's availability is one read away for [`crate::search`].
    embedder: OnceLock<Arc<DeferredEmbedder>>,
    /// This workspace's overlay **proposal** store, installed with the engine
    /// (ingest records into it, so the two must be the same pair for the whole
    /// epoch). It holds reviewable claims, never authoritative facts — see
    /// [`mdreader_ingest::proposals`].
    proposals: OnceLock<Arc<ProposalStore>>,
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
            embedder: OnceLock::new(),
            proposals: OnceLock::new(),
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

    /// This epoch's installed engine, if the model has finished loading — the
    /// accessor the sibling [`search`](crate::search) module reads through (the
    /// `engine` slot itself stays private to this module).
    pub(crate) fn db(&self) -> Option<Arc<EmbeddedCatDb>> {
        self.engine.get().map(Arc::clone)
    }

    /// This epoch's proposal store, installed alongside its engine — the
    /// accessor the sibling [`overlay`](crate::overlay) module reads through.
    pub(crate) fn proposals(&self) -> Option<Arc<ProposalStore>> {
        self.proposals.get().map(Arc::clone)
    }

    /// Whether this epoch's semantic (vector) modality can run, and if not, why —
    /// the read [`crate::search`] uses to decide whether to run the second arm,
    /// and what to tell the caller when it does not. An epoch with no engine yet
    /// reports [`Semantic::Loading`]; it has no index to search either, so the
    /// distinction is moot for the caller.
    pub(crate) fn semantic(&self) -> Semantic {
        self.embedder
            .get()
            .map_or(Semantic::Loading, |embedder| embedder.semantic())
    }

    /// Test helper: install `embedder` as this epoch's deferred embedder.
    #[cfg(test)]
    pub(crate) fn install_embedder_for_test(&self, embedder: Arc<DeferredEmbedder>) {
        let _ = self.embedder.set(embedder);
    }

    /// The installed engine, if the model has finished loading for this epoch.
    #[cfg(test)]
    fn has_engine(&self) -> bool {
        self.engine.get().is_some()
    }

    /// Test helper: install `db` as this epoch's engine (simulates the post-load
    /// engine install).
    #[cfg(test)]
    pub(crate) fn install_engine_for_test(&self, db: Arc<EmbeddedCatDb>) {
        let _ = self.engine.set(db);
    }

    /// Test helper: install `store` as this epoch's proposal store.
    #[cfg(test)]
    pub(crate) fn install_proposals_for_test(&self, store: Arc<ProposalStore>) {
        let _ = self.proposals.set(store);
    }

    /// THE choke-point. The single identity re-check every command / engine
    /// method funnels its workspace-derived result through as its final step:
    /// returns `value` while this epoch is still the current era, else
    /// `Err(StaleEpoch)` — a switch superseded it, so `value` belongs to a
    /// previous workspace and MUST be discarded. Making the barrier structural
    /// (one function, called at every return) is what closes the "forgot a
    /// return path" class of bug by construction; the `// EPOCH-GUARDED`
    /// convention marks each call site.
    pub(crate) fn finish<T>(
        self: &Arc<Self>,
        registry: &WorkspaceState,
        value: T,
    ) -> Result<T, StaleEpoch> {
        if registry.is_current(self) {
            Ok(value)
        } else {
            Err(StaleEpoch)
        }
    }
}

/// The stale-epoch error returned by [`Epoch::finish`] — a switch superseded the
/// epoch mid-command, so the value it guarded is a previous workspace's.
pub(crate) struct StaleEpoch;

impl StaleEpoch {
    /// The message a command surfaces when it discards a stale read/listing
    /// (commands that instead fall back to a benign value ignore this).
    pub(crate) fn message() -> String {
        "workspace changed during the operation".to_string()
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
/// (`import_status` inlines this through the choke-point; kept for tests.)
#[cfg(test)]
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
    /// embedding model), `"importing"` (in flight), `"reembedding"` (in
    /// flight, and re-embedding the whole workspace because the embedding
    /// model changed), `"done"`, `"model-unavailable"` (the embedding model
    /// could not be loaded — the workspace is open and readable, but nothing
    /// could be indexed and the semantic arm is off), or `"error"` (the index
    /// build itself failed).
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

    /// The same in-flight pass, reported as what it actually is: the embedding
    /// model changed, so the engine rotates the index and every file is
    /// embedded again. Its own phase because the ordinary `importing` label
    /// would understate a corpus-sized, one-time cost.
    fn reembedding(files_total: u32) -> Self {
        Self {
            phase: "reembedding".to_string(),
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

    /// The embedding model could not be loaded. Its **own** phase, distinct from
    /// both `done` and `error`: the workspace is open, its files are listed and
    /// its documents readable — what is missing is the ability to embed, so
    /// nothing could be indexed and the semantic search arm is off. `reason` is
    /// the underlying condition (a load timeout, a download failure, …), carried
    /// verbatim so the status bar can say what actually happened.
    fn model_unavailable(reason: String) -> Self {
        Self {
            phase: "model-unavailable".to_string(),
            files_done: 0,
            files_total: 0,
            chunks_embedded: 0,
            error: Some(reason),
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

/// The overlay **proposal** store for the index at `db_path`: a sibling file,
/// `proposals.db`, in the same per-workspace directory.
///
/// Deliberately a separate database, not a table inside the index. The index
/// holds facts the corpus mechanically implies; this holds claims a human has
/// not ruled on yet. Keeping them in one file would invite exactly the
/// conflation the approval model exists to prevent — and a reviewer's pending
/// decisions must survive an index rebuild, which drops and recreates the
/// other file's contents.
pub(crate) fn proposals_path_beside(db_path: &Path) -> PathBuf {
    db_path.with_file_name("proposals.db")
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

/// Open (creating if needed) the engine at `db_path` and the overlay proposal
/// store beside it, composed with the MDReader domain and `embedder`.
///
/// The two are opened together and returned together because ingest writes to
/// both: the index gets the corpus, the proposal store gets what the overlay
/// extractors *propose* over it. Nothing extraction produces is asserted into
/// the graph here — the engine would refuse it, and rightly, until a human
/// approves it through [`crate::overlay::approve_fact`].
fn open_engine(
    db_path: &Path,
    embedder: Arc<dyn Embedder>,
) -> Result<(Arc<EmbeddedCatDb>, Arc<ProposalStore>), String> {
    let proposals = Arc::new(
        ProposalStore::open(proposals_path_beside(db_path))
            .map_err(|err| format!("open proposal store: {err}"))?,
    );
    let mut config = EngineConfig::new(db_path, embedder).with_actor("mdreader-app");
    config.domains = vec![registration_with_proposals(Arc::clone(&proposals))];
    let db = EmbeddedCatDb::open(config)
        .map(Arc::new)
        .map_err(|err| format!("open index: {err}"))?;
    Ok((db, proposals))
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
        Err(_elapsed) => Err(format!(
            "timed out after {}s (the first run downloads the model)",
            deadline.as_secs()
        )),
    }
}

/// The two handles an opened index yields: the engine, and the deferred embedder
/// it was opened through (the one the model is installed into later).
type OpenIndex = (Arc<EmbeddedCatDb>, Arc<DeferredEmbedder>);

/// Ensure `epoch`'s **index** is open and installed. Deliberately independent of
/// the embedding model: opening the database, listing its documents, resolving
/// wikilinks and running the lexical arm need no vectors, so this step must never
/// be gated on a download. It is opened through a [`DeferredEmbedder`] carrying
/// the configured model's identity, so the index is keyed exactly as it would be
/// had the model loaded first, and the weights can be installed into it later.
///
/// `Ok(Some(pair))` to proceed, `Ok(None)` when a switch superseded this epoch
/// (abandon clean), `Err` when the index itself could not be opened.
fn ensure_index(
    epoch: &Arc<Epoch>,
    workspace: &WorkspaceState,
    app: &AppHandle,
) -> Result<Option<OpenIndex>, String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("no app data dir: {err}"))?;
    open_index_at(epoch, workspace, &db_path_under(&base, &epoch.root))
}

/// [`ensure_index`] with the database path already resolved — the app-handle-free
/// core, so a test drives exactly the same open + install sequence.
fn open_index_at(
    epoch: &Arc<Epoch>,
    workspace: &WorkspaceState,
    db_path: &Path,
) -> Result<Option<OpenIndex>, String> {
    // Fast path: this epoch's engine is already installed (with its embedder).
    if let (Some(db), Some(embedder)) = (epoch.engine.get(), epoch.embedder.get()) {
        return Ok(Some((Arc::clone(db), Arc::clone(embedder))));
    }

    let embedder = Arc::new(DeferredEmbedder::loading(configured_model_info()));
    let (db, proposals) = open_engine(db_path, Arc::clone(&embedder) as Arc<dyn Embedder>)?;

    // EPOCH-GUARDED commit point: a switch during the open supersedes this epoch
    // — drop the freshly-opened index rather than install into a dead era.
    if epoch.finish(workspace, ()).is_err() {
        return Ok(None);
    }

    // Install into THIS epoch's slots (idempotent; `claim_active` serializes
    // imports per epoch, so a racer here would have to be a coalesced straggler
    // and we simply adopt what it installed). The embedder and proposal store go
    // in FIRST, so no reader can ever see an installed engine without the
    // embedder whose availability describes it or the proposal store beside it.
    // Every handle is derived from this epoch's own `db_path`, so even an adopted
    // pair cannot point at another workspace.
    let _ = epoch.embedder.set(Arc::clone(&embedder));
    let _ = epoch.proposals.set(proposals);
    let _ = epoch.engine.set(Arc::clone(&db));
    Ok(Some((
        epoch.engine.get().map_or(db, Arc::clone),
        epoch.embedder.get().map_or(embedder, Arc::clone),
    )))
}

/// Ensure the embedding model is loaded into `embedder`, publishing the
/// `"loading"` phase across the (slow, possibly downloading) construction.
/// Returns the failure reason — already phrased for a human — when it cannot be
/// loaded; a subsequent call genuinely re-attempts the load, which is what makes
/// the status bar's retry mean something.
async fn ensure_model(
    epoch: &Arc<Epoch>,
    workspace: &WorkspaceState,
    channel: &Channel<ImportProgress>,
    embedder: &Arc<DeferredEmbedder>,
) -> Result<(), String> {
    if embedder.semantic() == Semantic::Ready {
        return Ok(());
    }
    publish(epoch, workspace, channel, ImportProgress::loading());
    let loaded = load_embedder_with_deadline(MODEL_LOAD_DEADLINE, build_embedder).await;
    match loaded {
        // `install` is the choke-point that refuses a model which is not the one
        // the index was opened under — the structural reason a failed load can
        // never be served by the test-only hash embedder.
        Ok(model) => embedder.install(model).map_err(model_load_failure),
        Err(reason) => {
            let reason = model_load_failure(reason);
            embedder.fail(reason.clone());
            Err(reason)
        }
    }
}

/// The one phrasing of a model-load failure, so the status bar, the search
/// degradation notice and the log all say the same true thing.
fn model_load_failure(reason: String) -> String {
    format!("the embedding model could not be loaded — {reason}")
}

/// The progress a starting import should publish: the ordinary `importing`
/// phase, or the distinct `reembedding` phase when this workspace's index was
/// built by a different embedding model and the engine is about to rotate it
/// (re-embedding every file — see the module docs on progress granularity).
///
/// The rotation question is asked of the engine, which answers with the same
/// comparison its import then makes, so the label cannot claim a rotation the
/// import does not perform. A failed probe is not itself an import failure:
/// it falls back to the ordinary label and lets the import surface any real
/// error.
async fn starting_progress(db: &EmbeddedCatDb, total: u32) -> ImportProgress {
    match db.pending_model_rotation().await {
        Ok(Some(_)) => ImportProgress::reembedding(total),
        Ok(None) | Err(_) => ImportProgress::importing(total),
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
    // EPOCH-GUARDED: stream to the UI only while this epoch is current.
    if epoch.finish(workspace, ()).is_ok() {
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
/// coarse progress on `on_event` (`loading` → `importing` → `done`, or
/// `model-unavailable`/`error`) and returning the final status.
///
/// The two preconditions are separate on purpose: the **index** opens first and
/// needs no model, so the workspace is readable from that moment on; the
/// **model** is loaded second and only gates the import itself and the semantic
/// search arm. A model that cannot load therefore yields `model-unavailable`
/// (with the reason) over a fully open workspace — never an empty one, and never
/// the generic `error` an actual index-build failure reports.
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

    // Coalesce onto an in-flight import for this epoch (#4). Its return, like
    // every other, funnels through the choke-point.
    if !claim_active(&epoch)? {
        // EPOCH-GUARDED
        return Ok(epoch
            .finish(&workspace, epoch.status())
            .unwrap_or_else(|_| epoch.status()));
    }
    let _guard = ActiveGuard {
        epoch: Arc::clone(&epoch),
    };

    // Step 1 — the index, which needs no model. Once this lands the workspace is
    // fully readable: the tree, the documents, wikilinks and the lexical arm.
    let (db, embedder) = match ensure_index(&epoch, &workspace, &app) {
        Ok(Some(pair)) => pair,
        // A switch superseded this epoch during the open — abandon clean.
        // EPOCH-GUARDED
        Ok(None) => {
            return Ok(epoch
                .finish(&workspace, epoch.status())
                .unwrap_or_else(|_| epoch.status()))
        }
        Err(err) => {
            // The index itself could not be opened — a genuine index failure.
            let failed = ImportProgress::failed(0, err);
            publish(&epoch, &workspace, &on_event, failed.clone());
            // EPOCH-GUARDED
            return Ok(epoch
                .finish(&workspace, failed)
                .unwrap_or_else(|_| epoch.status()));
        }
    };

    // Step 2 — the model, which ONLY indexing and the semantic arm need. A
    // failure here is reported as itself (`model-unavailable`, carrying the
    // reason) and stops the import: the workspace stays open and readable, and
    // the semantic arm reports its own unavailability at search time.
    if let Err(reason) = ensure_model(&epoch, &workspace, &on_event, &embedder).await {
        let unavailable = ImportProgress::model_unavailable(reason);
        publish(&epoch, &workspace, &on_event, unavailable.clone());
        // EPOCH-GUARDED
        return Ok(epoch
            .finish(&workspace, unavailable)
            .unwrap_or_else(|_| epoch.status()));
    }

    let total = scan_count(&epoch.root);
    publish(
        &epoch,
        &workspace,
        &on_event,
        starting_progress(&db, total).await,
    );

    // The engine serializes imports internally (single-writer); a concurrent
    // call simply queues, so re-firing on a rapid re-grant is safe.
    let outcome = db.import(&epoch.root, ImportOptions::default()).await;
    let final_status = match outcome {
        Ok(report) => ImportProgress::done(&report),
        Err(err) => ImportProgress::failed(total, err.to_string()),
    };
    publish(&epoch, &workspace, &on_event, final_status.clone());
    // EPOCH-GUARDED
    Ok(epoch
        .finish(&workspace, final_status)
        .unwrap_or_else(|_| epoch.status()))
}

/// The latest import status for the CURRENT workspace epoch — for the StatusBar's
/// initial render and any poll. A superseded epoch's status is never read, so a
/// switch leaves this reading `idle` until the new epoch's import publishes.
#[tauri::command]
#[specta::specta]
pub fn import_status(workspace: State<'_, WorkspaceState>) -> Result<ImportProgress, String> {
    match workspace.current() {
        Some(epoch) => {
            let status = epoch.status();
            // EPOCH-GUARDED
            Ok(epoch.finish(&workspace, status).unwrap_or_default())
        }
        None => Ok(ImportProgress::default()),
    }
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
    // EPOCH-GUARDED (both paths funnel through the ONE choke-point): a switch
    // during the read supersedes this epoch — discard rather than surface a
    // previous workspace's listing.
    epoch
        .finish(workspace, listing)
        .map_err(|_| StaleEpoch::message())
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
/// there). EVERY exit — no-engine, empty-target, no-match, resolved, ambiguous —
/// is one computed value funneled through the SINGLE [`Epoch::finish`] at the
/// return; there are no early raw returns of resolution data.
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

    let resolution = resolve_against(&epoch, target, heading)?;

    gate();
    // EPOCH-GUARDED: a switch during resolution supersedes this epoch — its result
    // is discarded to `unresolved` (silently-clean) rather than surfacing a
    // previous workspace's document.
    Ok(epoch
        .finish(workspace, resolution)
        .unwrap_or_else(|_| WikiResolution::unresolved()))
}

/// Compute the resolution of `target` against `epoch`'s index (the no-engine,
/// empty-target, and no-match cases all fold to `unresolved`). Kept separate so
/// the command has exactly one guarded return.
fn resolve_against(
    epoch: &Epoch,
    target: &str,
    heading: Option<&str>,
) -> Result<WikiResolution, String> {
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

    Ok(match matches.as_slice() {
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
    })
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
        if stem == wanted || path_forms.contains(&rel) {
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
    use mdreader_ingest::registration;
    use std::fs;
    use tempfile::TempDir;
    use uuid::Uuid;

    fn doc(path: &str) -> DocumentRow {
        DocumentRow {
            id: Uuid::new_v4(),
            source_id: Some(Uuid::new_v4()),
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

    /// The reported bug's user-visible half: a workspace embedded by another
    /// model is not merely re-indexed, it is re-embedded end to end, and the
    /// StatusBar must be told so rather than shown a generic `Indexing N
    /// files…`.
    ///
    /// Revert-RED: have `starting_progress` always return
    /// `ImportProgress::importing(total)` and the phase assertion below fails
    /// with `reembedding` vs `importing`.
    #[test]
    fn a_workspace_embedded_by_another_model_starts_in_the_reembedding_phase() {
        use catdb_catalog::{IndexState, IndexVersionRow};
        use catdb_storage::CatalogStorage;

        let corpus = TempDir::new().unwrap();
        let root = fs::canonicalize(corpus.path()).unwrap();
        fs::write(root.join("guide.md"), "# Guide\n\nIntro.\n").unwrap();
        let data = TempDir::new().unwrap();
        let db_path = data.path().join("catdb.db");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let embedder = Arc::new(HashEmbedder::new()) as Arc<dyn Embedder>;
        let (db, _proposals) = open_engine(&db_path, embedder).expect("open engine");
        rt.block_on(db.import(&root, ImportOptions::default()))
            .expect("import corpus");

        // The ordinary pass keeps the ordinary label.
        assert_eq!(rt.block_on(starting_progress(&db, 7)).phase, "importing");

        // Manufacture the reported situation through the storage lifecycle:
        // the ACTIVE version was embedded by a different model at the same
        // width (staged -> built -> flipped, the only legal route in).
        let active = rt
            .block_on(db.store().active_index_version())
            .unwrap()
            .unwrap();
        let other = IndexVersionRow {
            id: Uuid::new_v4(),
            embedding_model: "some-other-model/v1".to_string(),
            dim: active.dim,
            chunker_version: active.chunker_version.clone(),
            state: IndexState::Staged,
        };
        rt.block_on(db.store().put_index_version(other.clone()))
            .unwrap();
        rt.block_on(db.store().mark_index_built(other.id)).unwrap();
        db.store().activate_index_version_now(other.id).unwrap();

        let progress = rt.block_on(starting_progress(&db, 7));
        assert_eq!(
            progress.phase, "reembedding",
            "a model change reports itself instead of an understated index pass"
        );
        assert_eq!(
            progress.files_total, 7,
            "the file total still drives the spinner"
        );
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
    fn status_publish_is_discarded_when_a_switch_lands_in_the_publish_window() {
        let (_a, root_a) = workspace_dir();
        let (_b, root_b) = workspace_dir();
        let ws = WorkspaceState::default();
        let e1 = ws.switch(root_a.clone()).unwrap();

        // The import records progress into E1's OWN slot; import_status surfaces it.
        record_status(&e1, &ImportProgress::importing(3));
        assert_eq!(current_status(&ws).phase, "importing");

        // GATE: a switch lands INSIDE the publish window — between recording the
        // status and the "surface it to the UI" commit.
        let gate = || {
            let _ = ws.switch(root_b.clone());
        };
        gate();

        // The surface commit funnels through the choke-point: `finish` fails for
        // the superseded epoch, so the late status is NOT streamed. The current
        // epoch reads idle; E1's own (unread) slot still holds its last value.
        record_status(&e1, &ImportProgress::failed(0, "late".to_string()));
        assert!(e1.finish(&ws, ()).is_err(), "publish surface discarded");
        assert_eq!(current_status(&ws).phase, "idle");
        assert_eq!(e1.status().phase, "error");
    }

    #[test]
    fn engine_install_is_abandoned_when_a_switch_lands_in_the_load_window() {
        let (_a, root_a) = workspace_dir();
        let (_b, root_b) = workspace_dir();
        let ws = WorkspaceState::default();
        let e1 = ws.switch(root_a).unwrap(); // captured at import entry
        let db = in_memory_engine();

        // GATE: a switch lands INSIDE the model-load window, before the install.
        let gate = || {
            let _ = ws.switch(root_b.clone());
        };
        gate();

        // The install commit funnels through the choke-point: `finish` fails for
        // the superseded epoch, so the freshly-built model is DROPPED — nothing
        // installs into either epoch.
        assert!(e1.finish(&ws, db).is_err(), "install abandoned");
        assert!(!e1.has_engine(), "superseded epoch installed nothing");
        let current = ws.current().unwrap();
        assert!(!current.has_engine(), "current epoch unaffected");

        // Without a switch, the SAME commit installs into the current epoch.
        let db2 = in_memory_engine();
        if let Ok(db2) = current.finish(&ws, db2) {
            current.install_engine_for_test(db2);
        }
        assert!(
            current.has_engine(),
            "current epoch installs when not superseded"
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

    /// THE decoupling, at the level the bug was reported: a workspace whose
    /// embedding model cannot load still OPENS. Its index is installed, its files
    /// are listed and its documents readable — and the condition is reported as
    /// itself, not as an empty workspace and not as a generic index failure.
    ///
    /// Revert-RED: make `start_import` require the model before the index (open
    /// the index only after `ensure_model` succeeds, as it did before) and the
    /// `db()` / listing assertions below fail — the workspace has no engine and
    /// nothing to serve.
    #[test]
    fn a_workspace_whose_model_cannot_load_still_opens_and_lists_its_files() {
        let corpus = TempDir::new().unwrap();
        let root = fs::canonicalize(corpus.path()).unwrap();
        fs::write(root.join("keep.md"), "# Keep\n\nBody.\n").unwrap();
        let data = TempDir::new().unwrap();
        let db_path = data.path().join("catdb.db");

        let ws = WorkspaceState::granted(root.clone());
        let epoch = ws.current().unwrap();

        // Step 1 of `start_import`: the index opens with NO model at all.
        let (db, embedder) = open_index_at(&epoch, &ws, &db_path)
            .expect("the index opens without an embedding model")
            .expect("not superseded");
        assert!(
            epoch.has_engine(),
            "the engine is installed by step 1 alone"
        );
        assert!(
            epoch.proposals().is_some(),
            "the proposal store is installed beside it"
        );

        // Step 2 fails: the model cannot be loaded.
        embedder.fail(model_load_failure("timed out after 120s".to_string()));

        // The workspace is open: the listing serves the corpus (from the live fs
        // scan, since no import could run) rather than an empty tree.
        let listing = list_workspace_files_guarded(&ws, root.to_str().unwrap(), || {}).unwrap();
        assert!(
            listing.entries.iter().any(|entry| entry.name == "keep.md"),
            "the file tree is populated with no embedder: {:?}",
            listing.entries
        );
        assert!(!listing.from_index, "and says it is reading local files");

        // The engine is readable — the index-backed reads that need no vectors.
        assert!(db.store().list_documents().is_ok());

        // The condition is nameable, and names the real reason.
        match epoch.semantic() {
            Semantic::Unavailable(reason) => {
                assert!(
                    reason.contains("the embedding model could not be loaded"),
                    "got: {reason}"
                );
                assert!(reason.contains("timed out"), "carries the reason: {reason}");
            }
            other => panic!("expected an unavailable model, got {other:?}"),
        }

        // ...and it is its OWN status phase, distinct from both `done` and `error`.
        let progress = ImportProgress::model_unavailable(match epoch.semantic() {
            Semantic::Unavailable(reason) => reason,
            other => panic!("expected an unavailable model, got {other:?}"),
        });
        assert_eq!(progress.phase, "model-unavailable");
        assert_ne!(progress.phase, ImportProgress::failed(0, "x".into()).phase);
        assert!(!progress.is_done());
        assert!(
            progress.error.is_some(),
            "the reason travels with the phase"
        );
    }

    /// The test-only [`HashEmbedder`] can never stand in for a model that failed
    /// to load: the deferred embedder refuses any model that is not the one the
    /// index was opened under, and reports the refusal.
    ///
    /// Revert-RED: drop the identity check in `DeferredEmbedder::install` (just
    /// store the model) and the `is_err()` + `Unavailable` assertions fail — the
    /// hash embedder installs and the workspace starts answering searches with
    /// test vectors.
    #[test]
    fn a_failed_model_is_never_replaced_by_the_test_embedder() {
        let deferred = DeferredEmbedder::loading(model_info_for(false));
        deferred.fail(model_load_failure("network is unreachable".to_string()));

        let refused = deferred.install(Arc::new(HashEmbedder::new()) as Arc<dyn Embedder>);
        assert!(refused.is_err(), "the test embedder is refused");
        let message = refused.unwrap_err();
        assert!(message.contains(HashEmbedder::MODEL_NAME), "got: {message}");
        assert!(
            message.contains(FastembedEmbedder::MODEL_NAME),
            "got: {message}"
        );

        // Still unavailable — a refused install does not quietly become a success.
        assert!(matches!(deferred.semantic(), Semantic::Unavailable(_)));

        // And with no model installed, embedding ERRORS rather than inventing a
        // vector: nothing downstream can mistake a stand-in for a real embedding.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert!(rt
            .block_on(deferred.embed(&["anything".to_string()]))
            .is_err());
    }

    /// A retry genuinely re-attempts the load: after a failure the same deferred
    /// embedder still accepts the configured model, and the engine it was opened
    /// with becomes fully semantic without being reopened.
    ///
    /// Revert-RED: make `DeferredEmbedder::install` refuse once `Failed` (an
    /// `if matches!(*slot, ModelSlot::Failed(_)) { return Ok(()) }` early return)
    /// and the `Semantic::Ready` assertion fails — Retry would be decorative.
    #[test]
    fn a_retry_after_a_failed_load_installs_the_model_into_the_open_index() {
        let deferred = Arc::new(DeferredEmbedder::loading(model_info_for(true)));
        deferred.fail(model_load_failure("timed out after 120s".to_string()));
        assert!(matches!(deferred.semantic(), Semantic::Unavailable(_)));

        deferred
            .install(Arc::new(HashEmbedder::new()) as Arc<dyn Embedder>)
            .expect("the configured model installs");
        assert_eq!(deferred.semantic(), Semantic::Ready);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let vectors = rt
            .block_on(deferred.embed(&["semantic search".to_string()]))
            .expect("the installed model embeds");
        assert_eq!(vectors.len(), 1);
        assert_eq!(vectors[0].len(), HashEmbedder::DEFAULT_DIM as usize);
    }

    /// The configured model's identity is knowable WITHOUT loading it — the fact
    /// that lets the index open first. It must match what `build_embedder`
    /// actually produces on each branch, or the index would be keyed to a model
    /// name no embedder ever answers to.
    #[test]
    fn the_configured_model_identity_matches_the_embedder_each_branch_builds() {
        assert_eq!(model_info_for(true), HashEmbedder::new().info());

        let real = model_info_for(false);
        assert_eq!(real.name, FastembedEmbedder::MODEL_NAME);
        assert_eq!(real.dim, FastembedEmbedder::DIM);
        assert_eq!(real.max_tokens, FastembedEmbedder::MAX_TOKENS);
        assert_ne!(
            real.name,
            HashEmbedder::MODEL_NAME,
            "the shipping app is not configured for the test embedder"
        );

        // The identity the engine sees is the configured one, weights or not.
        assert_eq!(DeferredEmbedder::loading(real.clone()).info(), real);
    }

    #[test]
    fn model_load_hits_its_deadline_and_reports_a_retryable_timeout() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        // A loader that never resolves trips the deadline into a retryable error.
        // (`Arc<dyn Embedder>` is not `Debug`, so match rather than `unwrap_err`.)
        let timed_out = rt.block_on(load_embedder_with_deadline(
            Duration::from_millis(20),
            std::future::pending::<Result<Arc<dyn Embedder>, String>>,
        ));
        match timed_out {
            Err(err) => assert!(err.contains("timed out"), "got: {err}"),
            Ok(_) => panic!("expected a model-load timeout"),
        }

        // A loader that resolves in time passes its result through unchanged.
        let ok = rt.block_on(load_embedder_with_deadline(
            Duration::from_secs(5),
            || async { Ok(Arc::new(HashEmbedder::new()) as Arc<dyn Embedder>) },
        ));
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
