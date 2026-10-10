//! Tauri command surface for the MDReader shell — the *filesystem* half.
//!
//! This module owns the thin, containment-guarded fs layer: a folder picker, a
//! workspace watcher, and live-from-disk document reads. Reading is always done
//! straight off disk (never a stale index copy), so [`read_doc`] stays here. The
//! SQLite service adapter, indexing progress, source-backed
//! file listing and wikilink resolution live in [`crate::engine`] and reuse
//! this module's containment helpers. Each command derives `specta::Type` so
//! tauri-specta emits matching TypeScript into `apps/mdreader/ui/src/bindings.ts`.
//!
//! ## Workspace containment
//!
//! Filesystem access is confined to a single *granted* workspace root, held in
//! [`WorkspaceState`]. Only backend-owned library membership can establish that
//! grant. Admission retains the opened root authority and every read, save, and
//! import resolves beneath that handle without following descendant links.
//! Watching and epoch publication commit together; completion revalidates both
//! the epoch and the stored root identity. The legacy native picker
//! ([`pick_workspace_folder`]) only returns display text and can never grant it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{
    new_debouncer, DebounceEventHandler, DebounceEventResult, DebouncedEventKind, Debouncer,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use tauri_plugin_dialog::DialogExt;
use tauri_specta::Event;

use mdreader_core::{SourceRoot, SourceRootIdentity};

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
    generation: AtomicU64,
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

    /// The monotonic workspace generation high-water mark. It remains advanced
    /// while no workspace is open so stale watcher work can never collide with
    /// a later grant.
    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Create a fresh epoch for `root` (already canonicalized) and swap it in,
    /// advancing the generation. THE revocation barrier: the returned `Arc` is the
    /// new current era; any epoch captured before this call is now stale. Returns
    /// the new epoch.
    #[cfg(test)]
    pub(crate) fn switch(&self, root: PathBuf) -> Result<Arc<crate::engine::Epoch>, String> {
        let root = SourceRoot::capture(root).map_err(|error| error.to_string())?;
        self.switch_bound(root)
    }

    /// Install a root whose identity was captured by the owning admission path.
    /// Revalidation occurs while the epoch slot is held, immediately before
    /// publication, so a replacement cannot slip between capture and commit.
    #[cfg(test)]
    pub(crate) fn switch_bound(
        &self,
        root: SourceRoot,
    ) -> Result<Arc<crate::engine::Epoch>, String> {
        self.switch_bound_with_source(root, None)
    }

    /// Install an admitted root with an optional application-owned reader
    /// source identity.
    pub(crate) fn switch_bound_with_source(
        &self,
        root: SourceRoot,
        source_id: Option<uuid::Uuid>,
    ) -> Result<Arc<crate::engine::Epoch>, String> {
        switch_bound_with_hook(self, root, source_id, || {})
    }

    /// Test helper: a state already granting `root` (generation 1).
    #[cfg(test)]
    pub(crate) fn granted(root: PathBuf) -> Self {
        let state = Self::default();
        state.switch(root).expect("grant test root");
        state
    }
}

/// Publication core for an already-admitted root. The hook is a deterministic
/// test seam located exactly after the verification that formerly preceded the
/// epoch swap with no further check.
fn switch_bound_with_hook(
    state: &WorkspaceState,
    root: SourceRoot,
    source_id: Option<uuid::Uuid>,
    after_final_verification: impl FnOnce(),
) -> Result<Arc<crate::engine::Epoch>, String> {
    let mut slot = state
        .current
        .lock()
        .map_err(|_| "workspace state poisoned".to_string())?;
    root.verify().map_err(|error| error.to_string())?;
    after_final_verification();
    root.verify().map_err(|error| error.to_string())?;
    let generation = state
        .generation
        .load(Ordering::Acquire)
        .checked_add(1)
        .ok_or_else(|| "workspace generation overflow".to_string())?;
    u32::try_from(generation).map_err(|_| "workspace generation overflow".to_string())?;
    state.generation.store(generation, Ordering::Release);
    let epoch = crate::engine::Epoch::new_with_source(root, generation, source_id);
    *slot = Some(Arc::clone(&epoch));
    Ok(epoch)
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
#[cfg(test)]
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

/// Canonicalize `root` and confirm it is an existing directory. Used for picker
/// display and tests only; webview text never establishes a workspace grant.
#[cfg(test)]
pub(crate) fn canonical_dir(root: &str) -> Result<PathBuf, String> {
    canonical_dir_path(Path::new(root))
}

/// Native-path counterpart to [`canonical_dir`]. Backend-owned library roots
/// call this directly so their lossless OS representation is never converted to
/// a webview string and parsed back before authority is established.
pub(crate) fn canonical_dir_path(root: &Path) -> Result<PathBuf, String> {
    let real = std::fs::canonicalize(root)
        .map_err(|err| format!("cannot resolve {}: {err}", root.display()))?;
    if !real.is_dir() {
        return Err(format!("not a directory: {}", root.display()));
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
#[tauri::command(async)]
#[specta::specta]
pub fn ping(msg: String) -> PingResponse {
    PingResponse {
        echoed: msg,
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Establish a library grant only when the path still resolves to the exact
/// canonical identity admitted by the registry. This deliberately rejects
/// aliases: its input is already canonical, so a different result means the
/// directory was substituted after validation.
#[cfg(test)]
pub(crate) fn grant_workspace_path_exact(
    expected_root: &SourceRoot,
    state: &WorkspaceState,
) -> Result<PathBuf, String> {
    let root = prepare_source_root(&expected_root.path, Some(&expected_root.identity))?;
    state.switch_bound(root.clone())?;
    Ok(root.path)
}

/// Library-only grant carrying the backend-owned stable reader source identity.
pub(crate) fn grant_workspace_path_exact_with_source(
    expected_root: &SourceRoot,
    source_id: uuid::Uuid,
    state: &WorkspaceState,
) -> Result<PathBuf, String> {
    let root = prepare_source_root(&expected_root.path, Some(&expected_root.identity))?;
    state.switch_bound_with_source(root.clone(), Some(source_id))?;
    Ok(root.path)
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
    /// Saved-origin identity for bounded workspace assets; never a folder grant.
    #[serde(rename = "originReceipt")]
    pub origin_receipt: crate::reader_assets::WorkspaceOriginReceipt,
}

/// Read a single markdown document from within the granted workspace. Thin:
/// resolves + validates the path, reads the bytes, hashes them, decodes to text.
/// Rejects paths outside the workspace, non-files, and non-markdown extensions.
/// Captures the epoch once and re-checks it is current before returning, so a
/// switch during the read never hands back a previous workspace's bytes.
#[tauri::command(async)]
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
    read_doc_guarded_with_hooks(state, path, || {}, gate)
}

fn read_doc_guarded_with_hooks(
    state: &WorkspaceState,
    path: &str,
    before_read: impl FnOnce(),
    before_finish: impl FnOnce(),
) -> Result<DocPayload, String> {
    // Capture the epoch ONCE; validate + read against its grant.
    let epoch = state
        .current()
        .ok_or_else(|| "no workspace has been opened".to_string())?;
    epoch.validate_root()?;
    let source = epoch.source_root();
    let relative = source
        .relative_path(Path::new(path))
        .map_err(|error| error.to_string())?;
    if !is_markdown(&relative) {
        return Err(format!("not a markdown document: {path}"));
    }
    before_read();
    let read = source
        .read_relative(&relative)
        .map_err(|error| error.to_string())?;
    let hash = blake3::hash(&read.bytes).to_hex().to_string();
    let mtime = mtime_millis(read.modified);

    let payload = DocPayload {
        origin_receipt: crate::reader_assets::WorkspaceOriginReceipt::from_read(
            &epoch, &relative, &hash,
        )?,
        content: String::from_utf8_lossy(&read.bytes).into_owned(),
        mtime,
        hash,
    };

    before_finish();
    epoch.validate_root()?;
    // EPOCH-GUARDED: a switch during the read supersedes this epoch — discard the
    // (old-root) bytes rather than surfacing them under the new workspace.
    epoch
        .finish(state, payload)
        .map_err(|error| error.to_string())
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
fn stage_temp(root: &SourceRoot, dest: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let dir = dest
        .parent()
        .ok_or_else(|| format!("no parent directory for {}", dest.display()))?;
    let tmp = dir.join(temp_filename());
    let mut file = root
        .create_new_relative(&tmp)
        .map_err(|err| format!("stage temp file: {err}"))?;
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
fn commit_rename(root: &SourceRoot, tmp: &Path, dest: &Path) -> Result<(), String> {
    match root.rename_relative(tmp, dest) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = root.remove_file_relative(tmp);
            Err(format!(
                "commit save for {}: {err}",
                root.path.join(dest).display()
            ))
        }
    }
}

/// Reads `path`'s current bytes and returns their blake3 hex digest — the digest
/// [`read_doc`]/[`save_doc`] agree on.
fn hash_on_disk(root: &SourceRoot, path: &Path) -> Result<String, String> {
    let read = root
        .read_relative(path)
        .map_err(|err| format!("read {}: {err}", root.path.join(path).display()))?;
    Ok(blake3::hash(&read.bytes).to_hex().to_string())
}

/// The stable per-DIRECTORY save-lock sidecar: `<dir>/.mdreader.savelock`. ONE
/// hidden lock file per directory (created on demand, never removed — a stable
/// inode that survives the atomic rename that swaps a document's inode). Chosen
/// per-directory, NOT per-document, so a saved workspace never accretes a hidden
/// lock file per edited doc; the coarser granularity (concurrent saves to files in
/// the same directory serialize) is inconsequential for a local single-user reader
/// where saves are brief and uncontended. Hidden + non-markdown, so the listing /
/// watcher (which filter to `.md`/`.markdown`) never surface it.
fn savelock_path(target: &Path) -> Option<PathBuf> {
    Some(target.parent()?.join(".mdreader.savelock"))
}

/// Holds an exclusive advisory `flock` on the directory's STABLE sidecar lock for
/// the whole check→write→rename window, releasing it on drop (the engine's import-
/// lease pattern). The lock lives on the sidecar — NOT the target — because the
/// target's inode is replaced by the atomic rename, so a lock held on it could not
/// span the swap; the sidecar's inode never changes, so cooperating app saves (a
/// second `save_doc`) serialize correctly on it.
///
/// ## Residual (documented local-single-user limitation)
/// An advisory lock only serializes writers that COOPERATE by taking it. A foreign
/// editor that writes the target directly, in the same instant, is not excluded —
/// so `save_doc` additionally re-hashes the target under the lock immediately
/// before the rename (catching such a write → [`SaveOutcome`] conflict rather than
/// a clobber). The vanishingly small remaining window (a foreign write landing
/// between that final re-hash and the rename) is accepted for a local, single-user
/// reader; there is no other tenant to lose the race to.
struct SaveLock(std::fs::File);

impl SaveLock {
    /// Open (creating if needed) the target's sidecar lock and block until its
    /// exclusive advisory lock is held.
    fn acquire(root: &SourceRoot, target: &Path) -> Result<Self, String> {
        let sidecar = savelock_path(target)
            .ok_or_else(|| format!("no lock path for {}", target.display()))?;
        let file = root
            .open_lock_relative(&sidecar)
            .map_err(|err| format!("open save lock {}: {err}", sidecar.display()))?;
        fs2::FileExt::lock_exclusive(&file)
            .map_err(|err| format!("lock save for {}: {err}", target.display()))?;
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
#[tauri::command(async)]
#[specta::specta]
pub fn save_doc(
    app: AppHandle,
    path: String,
    content: String,
    expected_hash: String,
    force: bool,
    state: tauri::State<'_, WorkspaceState>,
) -> Result<SaveOutcome, String> {
    save_doc_guarded(
        &*crate::profile_transition::workspace::admit(&app, &state)?,
        &path,
        &content,
        &expected_hash,
        force,
        commit_rename,
        || {},
        || {},
    )
}

/// The save core. `commit` is the atomic PUBLISHER (production: [`commit_rename`],
/// temp→rename); a test can inject a non-atomic one to prove the atomicity
/// assertion has teeth. Two test hooks (both no-ops in production): `before_rename`
/// fires UNDER the lock after the hash check + temp staging but before the swap (a
/// test injects an interleaved external write there); `before_finish` fires just
/// before the epoch identity re-check (a test injects a workspace switch there).
// The 8 args are a deliberate test-injection seam (5 data params + the 3
// swappable hooks above), not incidental sprawl — bundling them into a struct
// would still have to satisfy `save_doc_command_body_is_a_single_bare_call_to_the_guarded_core`,
// which asserts this exact positional wiring (and ~a dozen test call sites) by
// design, so the lint is silenced narrowly here instead of restructuring that.
#[allow(clippy::too_many_arguments)]
fn save_doc_guarded(
    state: &WorkspaceState,
    path: &str,
    content: &str,
    expected_hash: &str,
    force: bool,
    commit: impl FnOnce(&SourceRoot, &Path, &Path) -> Result<(), String>,
    before_rename: impl FnOnce(),
    before_finish: impl FnOnce(),
) -> Result<SaveOutcome, String> {
    save_doc_guarded_with_hooks(
        state,
        path,
        content,
        expected_hash,
        force,
        commit,
        before_rename,
        || {},
        before_finish,
    )
}

#[allow(clippy::too_many_arguments)]
fn save_doc_guarded_with_hooks(
    state: &WorkspaceState,
    path: &str,
    content: &str,
    expected_hash: &str,
    force: bool,
    commit: impl FnOnce(&SourceRoot, &Path, &Path) -> Result<(), String>,
    before_rename: impl FnOnce(),
    after_final_verification: impl FnOnce(),
    before_finish: impl FnOnce(),
) -> Result<SaveOutcome, String> {
    // Capture the epoch ONCE; validate + guard + write against its grant.
    let epoch = state
        .current()
        .ok_or_else(|| "no workspace has been opened".to_string())?;
    epoch.validate_root()?;
    let source = epoch.source_root();
    let relative = source
        .relative_path(Path::new(path))
        .map_err(|error| error.to_string())?;
    if !is_markdown(&relative) {
        return Err(format!("not a markdown document: {path}"));
    }

    // Serialize the ENTIRE check → temp-write → rename window against other
    // cooperating writers (a second `save_doc`, the engine) with an exclusive
    // advisory `flock` held on the target. A non-cooperating external writer can
    // still ignore an advisory lock, so we ALSO re-verify the on-disk hash under
    // the lock immediately before the swap — together these close the
    // compare-then-rename TOCTOU: a write that lands between our check and the
    // rename is caught and the swap is refused (the external bytes stay intact).
    let _lock = SaveLock::acquire(&source, &relative)?;

    // Hash guard (under the lock): unless force-overwriting, the file's current
    // bytes must still hash to what the buffer was anchored to.
    let current_hash = hash_on_disk(&source, &relative)?;
    if !force && current_hash != expected_hash {
        before_finish();
        epoch.validate_root()?;
        // EPOCH-GUARDED
        return epoch
            .finish(state, SaveOutcome::conflict(current_hash))
            .map_err(|error| error.to_string());
    }

    // Stage the new bytes into a sibling temp (still holding the lock).
    let bytes = content.as_bytes();
    let new_hash = blake3::hash(bytes).to_hex().to_string();
    let tmp = stage_temp(&source, &relative, bytes)?;

    before_rename();
    if let Err(error) = epoch.validate_root() {
        let _ = source.remove_file_relative(&tmp);
        return Err(error);
    }

    // Re-verify under the lock, right before the swap: a write that raced in
    // between our check and now (a non-cooperating external writer) changes the
    // hash → refuse the rename so its content survives, and report the conflict.
    if !force {
        let now_hash = hash_on_disk(&source, &relative)?;
        if now_hash != current_hash {
            let _ = source.remove_file_relative(&tmp);
            before_finish();
            epoch.validate_root()?;
            // EPOCH-GUARDED
            return epoch
                .finish(state, SaveOutcome::conflict(now_hash))
                .map_err(|error| error.to_string());
        }
    }

    epoch.validate_root()?;
    after_final_verification();
    commit(&source, &tmp, &relative)?;
    let mtime = mtime_millis(
        source
            .read_relative(&relative)
            .ok()
            .and_then(|read| read.modified),
    );
    let outcome = SaveOutcome::saved(new_hash, mtime);

    before_finish();
    epoch.validate_root()?;
    // EPOCH-GUARDED: a switch during the save supersedes this epoch — the write
    // landed on the (still-correct) old-workspace path, but its provenance must
    // not be handed to the new workspace, so discard the outcome.
    epoch
        .finish(state, outcome)
        .map_err(|error| error.to_string())
}

// --------------------------------------------------------------------------
// pick_workspace_folder
// --------------------------------------------------------------------------

/// Open the native folder picker and return the chosen absolute path
/// (canonicalized), or `None` when the user cancels. This does NOT mutate the
/// workspace grant: product admission uses the library source command, so the
/// returned display path carries no authority. Async so blocking on the dialog never parks the
/// main thread; the dialog plugin pumps the picker on the event loop.
#[tauri::command]
#[specta::specta]
pub async fn pick_workspace_folder(app: AppHandle) -> Result<Option<String>, String> {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        // The dialog plugin does not expose directory selection on mobile. Do
        // not substitute file selection or an unscoped path: both would make a
        // path look authoritative without a durable OS directory grant.
        let _ = app;
        return Err("native-folder-picker-unavailable".to_owned());
    }

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    let Some(path) = app
        .dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|file| file.into_path().ok())
    else {
        return Ok(None);
    };

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let real = std::fs::canonicalize(&path)
            .map_err(|err| format!("cannot resolve {}: {err}", path.display()))?;
        Ok(Some(real.to_string_lossy().into_owned()))
    }
}

// --------------------------------------------------------------------------
// Library-source watcher
// --------------------------------------------------------------------------

/// Request emitted by the native application menu when the user chooses
/// Settings. The renderer owns the sheet; the native menu only requests it.
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "mdreader://show-settings")]
pub struct ShowSettings {}

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
/// the library watch command returns). The frontend surfaces it as a degraded watcher
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
/// by the library-source watch transaction. The frontend adopts `generation` as the reference it
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
struct ActiveWatcher {
    _debouncer: Debouncer<RecommendedWatcher>,
    _root: SourceRoot,
    _generation: u64,
}

/// The live watcher and the exact root object it was armed against.
#[derive(Default)]
pub struct WatcherState(Mutex<Option<ActiveWatcher>>);

/// Revoke one exact active workspace and stop its watcher as one transaction.
///
/// Lock order matches [`commit_workspace`]: watcher first, then epoch. The
/// generation advances even though the current epoch becomes `None`, so a later
/// open cannot reuse the generation carried by an already-draining old watcher.
pub(crate) fn revoke_workspace(
    expected_epoch: &Arc<crate::engine::Epoch>,
    workspace: &WorkspaceState,
    watcher: &WatcherState,
) -> Result<u32, String> {
    let mut watcher_slot = watcher
        .0
        .lock()
        .map_err(|_| "watcher state poisoned".to_string())?;
    let mut epoch_slot = workspace
        .current
        .lock()
        .map_err(|_| "workspace state poisoned".to_string())?;
    let epoch = epoch_slot
        .as_ref()
        .ok_or_else(|| "no workspace has been opened".to_string())?;
    if !Arc::ptr_eq(epoch, expected_epoch) {
        return Err("active workspace does not match the requested source".to_string());
    }
    let expected_root = expected_epoch.source_root();
    if watcher_slot.as_ref().is_some_and(|active| {
        active._root != expected_root || active._generation != expected_epoch.generation
    }) {
        return Err("watcher and workspace authority diverged".to_string());
    }

    let generation = workspace
        .generation
        .load(Ordering::Acquire)
        .checked_add(1)
        .ok_or_else(|| "workspace generation overflow".to_string())?;
    let wire_generation =
        u32::try_from(generation).map_err(|_| "workspace generation overflow".to_string())?;
    workspace.generation.store(generation, Ordering::Release);
    *epoch_slot = None;
    *watcher_slot = None;
    Ok(wire_generation)
}

/// Library-only watch carrying the backend-owned stable reader source identity.
pub(crate) fn watch_workspace_path_exact_with_source(
    app: &AppHandle,
    expected_root: &SourceRoot,
    source_id: uuid::Uuid,
    workspace: &WorkspaceState,
    state: &WatcherState,
) -> Result<WorkspaceGrant, String> {
    watch_workspace_path_checked(
        app,
        &expected_root.path,
        workspace,
        state,
        Some(&expected_root.identity),
        Some(source_id),
    )
}

fn watch_workspace_path_checked(
    app: &AppHandle,
    root: &Path,
    workspace: &WorkspaceState,
    state: &WatcherState,
    expected_identity: Option<&SourceRootIdentity>,
    source_id: Option<uuid::Uuid>,
) -> Result<WorkspaceGrant, String> {
    let source_root = prepare_source_root(root, expected_identity)?;
    let event_root = source_root.clone();
    let emit_app = app.clone();
    // The generation the watcher is armed under — filled by `commit_workspace`
    // (under the lock, matching the new epoch exactly) and read by the emit
    // closure so every event is tagged with the era it belongs to.
    let armed_gen = Arc::new(AtomicU64::new(0));
    let gen_for_closure = Arc::clone(&armed_gen);
    let declined = Arc::new(AtomicBool::new(false));
    let declined_for_closure = Arc::clone(&declined);
    let (granted, generation) = watch_and_commit_bound(
        source_root,
        source_id,
        workspace,
        state,
        &armed_gen,
        || {},
        || {},
        || {},
        move |result: DebounceEventResult| {
            let generation = gen_for_closure.load(Ordering::Acquire) as u32;
            if declined_for_closure.load(Ordering::Acquire) {
                return;
            }
            if let Err(error) = event_root.verify() {
                declined_for_closure.store(true, Ordering::Release);
                let _ = WatchError {
                    message: error.to_string(),
                    generation,
                }
                .emit(&emit_app);
                return;
            }
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

/// The workspace-switch core, factored out of the library watcher so it is
/// exercisable without an `AppHandle` (the handle is only needed to build the
/// production `on_event` sink; tests pass a no-op). Resolves `root`, arms a fresh
/// watcher into a *local*, and — only on success — commits the new grant + new
/// watcher together, filling `armed_gen` with the new epoch's generation. Any
/// earlier failure returns with nothing mutated. Returns the canonical root and
/// its generation.
#[cfg(test)]
fn watch_and_commit<F: DebounceEventHandler>(
    root: &str,
    workspace: &WorkspaceState,
    watcher: &WatcherState,
    armed_gen: &AtomicU64,
    on_event: F,
) -> Result<(PathBuf, u32), String> {
    watch_and_commit_path(Path::new(root), workspace, watcher, armed_gen, on_event)
}

#[cfg(test)]
fn watch_and_commit_path<F: DebounceEventHandler>(
    root: &Path,
    workspace: &WorkspaceState,
    watcher: &WatcherState,
    armed_gen: &AtomicU64,
    on_event: F,
) -> Result<(PathBuf, u32), String> {
    let source_root = prepare_source_root(root, None)?;
    watch_and_commit_bound(
        source_root,
        None,
        workspace,
        watcher,
        armed_gen,
        || {},
        || {},
        || {},
        on_event,
    )
}

#[cfg(all(test, unix))]
fn watch_and_commit_path_exact<F: DebounceEventHandler>(
    expected_root: &SourceRoot,
    workspace: &WorkspaceState,
    watcher: &WatcherState,
    armed_gen: &AtomicU64,
    on_event: F,
) -> Result<(PathBuf, u32), String> {
    watch_and_commit_path_exact_with_hooks(
        expected_root,
        workspace,
        watcher,
        armed_gen,
        || {},
        || {},
        || {},
        on_event,
    )
}

#[cfg(all(test, unix))]
// The three independent hooks are deterministic seams for the validation,
// watcher-arm, and publication race windows.
#[allow(clippy::too_many_arguments)]
fn watch_and_commit_path_exact_with_hooks<
    F: DebounceEventHandler,
    V: FnOnce(),
    A: FnOnce(),
    P: FnOnce(),
>(
    expected_root: &SourceRoot,
    workspace: &WorkspaceState,
    watcher: &WatcherState,
    armed_gen: &AtomicU64,
    after_validation: V,
    after_watcher_arm: A,
    before_epoch_publication: P,
    on_event: F,
) -> Result<(PathBuf, u32), String> {
    let source_root = prepare_source_root(&expected_root.path, Some(&expected_root.identity))?;
    watch_and_commit_bound(
        source_root,
        None,
        workspace,
        watcher,
        armed_gen,
        after_validation,
        after_watcher_arm,
        before_epoch_publication,
        on_event,
    )
}

fn prepare_source_root(
    root: &Path,
    expected_identity: Option<&SourceRootIdentity>,
) -> Result<SourceRoot, String> {
    // A workspace SWITCH targets a NEW root that is not inside the old grant, so
    // resolve it independently: this command is the one that ESTABLISHES the grant.
    let new_root = canonical_dir_path(root)?;
    if expected_identity.is_some() && new_root != root {
        return Err(format!(
            "workspace root identity changed since library admission: {}",
            root.display()
        ));
    }
    match expected_identity {
        Some(identity) => {
            SourceRoot::restore(new_root, identity.clone()).map_err(|error| error.to_string())
        }
        None => SourceRoot::capture(new_root).map_err(|error| error.to_string()),
    }
}

// The three independent hooks are deterministic seams for the validation,
// watcher-arm, and publication race windows.
#[allow(clippy::too_many_arguments)]
fn watch_and_commit_bound<F: DebounceEventHandler>(
    new_root: SourceRoot,
    source_id: Option<uuid::Uuid>,
    workspace: &WorkspaceState,
    watcher: &WatcherState,
    armed_gen: &AtomicU64,
    after_validation: impl FnOnce(),
    after_watcher_arm: impl FnOnce(),
    before_epoch_publication: impl FnOnce(),
    on_event: F,
) -> Result<(PathBuf, u32), String> {
    after_validation();
    // Build + arm the new watcher into a LOCAL, holding NO state lock across
    // construction. A failure here changes nothing — the previous grant + watcher
    // stay live, so the caller simply stays on the old workspace.
    let mut debouncer = new_debouncer(Duration::from_millis(WATCH_DEBOUNCE_MS), on_event)
        .map_err(|err| format!("watcher init failed: {err}"))?;
    debouncer
        .watcher()
        .watch(&new_root.path, RecursiveMode::Recursive)
        .map_err(|err| format!("watch {} failed: {err}", new_root.path.display()))?;

    after_watcher_arm();
    // The watcher may have armed after the path was substituted. Revalidate the
    // opened object now, then once more inside the atomic publication section.
    new_root.verify().map_err(|error| error.to_string())?;

    let granted_path = new_root.path.clone();
    let generation = commit_workspace(
        workspace,
        watcher,
        new_root,
        source_id,
        debouncer,
        armed_gen,
        before_epoch_publication,
    )?;
    Ok((granted_path, generation))
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
    new_root: SourceRoot,
    source_id: Option<uuid::Uuid>,
    debouncer: Debouncer<RecommendedWatcher>,
    armed_gen: &AtomicU64,
    after_final_verification: impl FnOnce(),
) -> Result<u32, String> {
    let mut watcher_slot = watcher
        .0
        .lock()
        .map_err(|_| "watcher state poisoned".to_string())?;
    let mut epoch_slot = workspace
        .current
        .lock()
        .map_err(|_| "workspace state poisoned".to_string())?;
    new_root.verify().map_err(|error| error.to_string())?;
    after_final_verification();
    new_root.verify().map_err(|error| error.to_string())?;
    // Both locks held — neither write below can fail, so the swap is atomic. The
    // watcher's armed generation is published to match the new epoch exactly, so
    // its events are tagged with the era they belong to.
    let generation = workspace
        .generation
        .load(Ordering::Acquire)
        .checked_add(1)
        .ok_or_else(|| "workspace generation overflow".to_string())?;
    let wire_generation =
        u32::try_from(generation).map_err(|_| "workspace generation overflow".to_string())?;
    workspace.generation.store(generation, Ordering::Release);
    armed_gen.store(generation, Ordering::Release);
    *epoch_slot = Some(crate::engine::Epoch::new_with_source(
        new_root.clone(),
        generation,
        source_id,
    ));
    *watcher_slot = Some(ActiveWatcher {
        _debouncer: debouncer,
        _root: new_root,
        _generation: generation,
    });
    Ok(wire_generation)
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_dir, commit_rename, grant_workspace_path_exact,
        grant_workspace_path_exact_with_source, read_doc_guarded, resolve_dir, resolve_doc,
        resolve_within, revoke_workspace, save_doc_guarded, switch_bound_with_hook,
        watch_and_commit, DebounceEventResult, WatcherState, WorkspaceState,
    };
    #[cfg(unix)]
    use super::{
        read_doc_guarded_with_hooks, save_doc_guarded_with_hooks, watch_and_commit_path_exact,
        watch_and_commit_path_exact_with_hooks,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use tempfile::TempDir;

    use mdreader_core::SourceRoot;

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
    fn revoking_a_workspace_drops_its_watcher_stales_the_epoch_and_keeps_generation_monotonic() {
        let (_a, root_a) = workspace();
        let (_b, root_b) = workspace();
        let workspace = WorkspaceState::default();
        let watcher = WatcherState::default();
        let armed_gen = AtomicU64::new(0);
        watch_and_commit(
            root_a.to_str().expect("utf8 root"),
            &workspace,
            &watcher,
            &armed_gen,
            |_result: DebounceEventResult| {},
        )
        .expect("arm active workspace");
        let old_epoch = workspace.current().expect("active epoch");
        let revoked_generation =
            revoke_workspace(&old_epoch, &workspace, &watcher).expect("revoke exact authority");

        assert!(
            workspace.current().is_none(),
            "the old epoch is no longer current"
        );
        assert!(
            !workspace.is_current(&old_epoch),
            "in-flight old work is stale"
        );
        assert!(watcher.0.lock().expect("watcher state").is_none());
        assert!(revoked_generation > old_epoch.generation as u32);

        let (_, next_generation) = watch_and_commit(
            root_b.to_str().expect("utf8 root"),
            &workspace,
            &watcher,
            &armed_gen,
            |_result: DebounceEventResult| {},
        )
        .expect("open after an empty workspace");
        assert!(
            next_generation > revoked_generation,
            "a close must not reset generations and let stale watcher events collide"
        );
    }

    #[test]
    fn revoking_a_degraded_grant_without_a_watcher_is_valid() {
        let (_a, root_a) = workspace();
        let workspace = WorkspaceState::granted(root_a);
        let watcher = WatcherState::default();
        let epoch = workspace.current().expect("degraded epoch");

        revoke_workspace(&epoch, &workspace, &watcher)
            .expect("the supported grant-only fallback has no watcher to drop");

        assert!(workspace.current().is_none());
        assert!(watcher.0.lock().expect("watcher state").is_none());
    }

    #[test]
    fn revoking_an_old_same_root_epoch_preserves_the_new_epoch_and_generation() {
        let (_a, root_a) = workspace();
        let workspace = WorkspaceState::default();
        let first = workspace
            .switch(root_a.clone())
            .expect("first same-root epoch");
        let current = workspace.switch(root_a).expect("new same-root epoch");
        let watcher = WatcherState::default();
        let generation = workspace.generation();

        assert!(revoke_workspace(&first, &workspace, &watcher).is_err());

        assert!(workspace.is_current(&current));
        assert_eq!(workspace.generation(), generation);
        assert!(watcher.0.lock().expect("watcher state").is_none());
    }

    #[test]
    fn a_mismatched_live_watcher_preserves_every_authority_on_revoke_failure() {
        let (_a, root_a) = workspace();
        let workspace = WorkspaceState::default();
        let watcher = WatcherState::default();
        watch_and_commit(
            root_a.to_str().expect("utf8 root"),
            &workspace,
            &watcher,
            &AtomicU64::new(0),
            |_result: DebounceEventResult| {},
        )
        .expect("arm workspace");
        let epoch = workspace.current().expect("active epoch");
        let generation = workspace.generation();
        watcher
            .0
            .lock()
            .expect("watcher state")
            .as_mut()
            .unwrap()
            ._generation += 1;

        assert!(revoke_workspace(&epoch, &workspace, &watcher).is_err());

        assert!(workspace.is_current(&epoch));
        assert_eq!(workspace.generation(), generation);
        assert!(watcher.0.lock().expect("watcher state").is_some());
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
        let epoch = ws.current().expect("active epoch");
        let watcher_slot = watcher.0.lock().expect("watcher state");
        let active_watcher = watcher_slot.as_ref().expect("live watcher");
        assert_eq!(active_watcher._root.identity, epoch.root_identity);
        assert_eq!(active_watcher._generation, epoch.generation);
    }

    #[test]
    fn a_new_generation_preserves_the_mandatory_root_identity() {
        let (_dir, root) = workspace();
        let ws = WorkspaceState::granted(root.clone());
        let first = ws.current().expect("first epoch");
        let second = ws.switch(root).expect("second epoch");

        assert!(second.generation > first.generation);
        assert_eq!(second.root_identity, first.root_identity);
        assert!(second.validate_root().is_ok());
    }

    #[test]
    fn switch_bound_revalidates_after_its_final_verification_before_publication() {
        use std::sync::Arc;

        let temp = TempDir::new().expect("tempdir");
        let prior = temp.path().join("prior");
        let admitted = temp.path().join("admitted");
        fs::create_dir(&prior).expect("create prior root");
        fs::create_dir(&admitted).expect("create admitted root");
        let prior = fs::canonicalize(prior).expect("canonical prior");
        let admitted = fs::canonicalize(admitted).expect("canonical admitted");
        let expected = SourceRoot::capture(admitted.clone()).expect("capture admitted root");
        let moved = temp.path().join("moved-admitted");
        let workspace = WorkspaceState::granted(prior.clone());
        let before = workspace.current().expect("prior epoch");

        let error = switch_bound_with_hook(&workspace, expected, None, || {
            fs::rename(&admitted, &moved).expect("move admitted object after verification");
            fs::create_dir(&admitted).expect("create same-path replacement");
        })
        .err()
        .expect("a substitution in the final publication window must be refused");

        assert!(error.contains("identity"), "explicit refusal: {error}");
        let after = workspace.current().expect("prior epoch remains");
        assert!(Arc::ptr_eq(&before, &after));
        assert_eq!(after.root, prior);
    }

    #[cfg(unix)]
    #[test]
    fn exact_library_grant_refuses_a_post_validation_symlink_swap_without_advancing_epoch() {
        use std::os::unix::fs::symlink;
        use std::sync::Arc;

        let temp = TempDir::new().expect("tempdir");
        let prior = temp.path().join("prior");
        let admitted = temp.path().join("admitted");
        let replacement = temp.path().join("replacement");
        fs::create_dir_all(&prior).expect("prior root");
        fs::create_dir_all(&admitted).expect("admitted root");
        fs::create_dir_all(&replacement).expect("replacement root");
        let prior = fs::canonicalize(prior).expect("canonical prior");
        let admitted = fs::canonicalize(admitted).expect("canonical admitted");
        let replacement = fs::canonicalize(replacement).expect("canonical replacement");
        let admitted_root =
            SourceRoot::capture(admitted.clone()).expect("capture admitted identity");
        let workspace = WorkspaceState::granted(prior.clone());
        let before = workspace.current().expect("prior epoch");

        fs::remove_dir(&admitted).expect("remove admitted root");
        symlink(&replacement, &admitted).expect("substitute symlink");

        let error = grant_workspace_path_exact(&admitted_root, &workspace)
            .expect_err("changed canonical identity must not be granted");
        assert!(error.contains("identity changed"));
        let after = workspace.current().expect("prior epoch remains");
        assert!(Arc::ptr_eq(&before, &after));
        assert_eq!(after.root, prior);
        assert_eq!(after.generation, before.generation);
        assert!(
            !crate::engine::db_path_under(
                &temp.path().join("app-data"),
                &SourceRoot::capture(replacement.clone()).expect("capture replacement identity")
            )
            .exists(),
            "a refused target never reaches index creation"
        );
    }

    #[test]
    fn degraded_exact_grant_refuses_a_different_object_at_the_same_path() {
        use std::sync::Arc;

        let temp = TempDir::new().expect("tempdir");
        let prior = temp.path().join("prior");
        let admitted = temp.path().join("admitted");
        fs::create_dir_all(&prior).expect("prior root");
        fs::create_dir_all(&admitted).expect("admitted root");
        let prior = fs::canonicalize(prior).expect("canonical prior");
        let admitted = fs::canonicalize(admitted).expect("canonical admitted");
        let expected = SourceRoot::capture(admitted.clone()).expect("capture admitted identity");
        let workspace = WorkspaceState::granted(prior.clone());
        let before = workspace.current().expect("prior epoch");

        fs::rename(&admitted, temp.path().join("moved-admitted"))
            .expect("move admitted filesystem object");
        fs::create_dir(&admitted).expect("create replacement at exact path");

        let error = grant_workspace_path_exact(&expected, &workspace)
            .expect_err("degraded grant must refuse replacement identity");
        assert_eq!(error, "identity-changed");
        let after = workspace.current().expect("prior epoch remains");
        assert!(Arc::ptr_eq(&before, &after));
        assert_eq!(after.root, prior);
        assert_eq!(after.generation, before.generation);
        assert_eq!(after.root_identity, before.root_identity);
    }

    #[test]
    fn exact_library_grant_publishes_the_backend_owned_reader_source_identity() {
        let temp = TempDir::new().expect("tempdir");
        let admitted = temp.path().join("admitted");
        fs::create_dir_all(&admitted).expect("admitted root");
        let admitted = fs::canonicalize(admitted).expect("canonical admitted root");
        let expected = SourceRoot::capture(admitted).expect("capture admitted identity");
        let workspace = WorkspaceState::default();
        let source_id = uuid::Uuid::new_v4();

        grant_workspace_path_exact_with_source(&expected, source_id, &workspace)
            .expect("grant exact library source");

        assert_eq!(
            workspace.current().expect("published epoch").source_id(),
            Some(source_id)
        );
    }

    #[cfg(unix)]
    #[test]
    fn exact_library_watch_refuses_a_post_validation_symlink_swap_without_replacing_the_watcher() {
        use std::os::unix::fs::symlink;
        use std::sync::Arc;

        let temp = TempDir::new().expect("tempdir");
        let prior = temp.path().join("prior");
        let admitted = temp.path().join("admitted");
        let replacement = temp.path().join("replacement");
        fs::create_dir_all(&prior).expect("prior root");
        fs::create_dir_all(&admitted).expect("admitted root");
        fs::create_dir_all(&replacement).expect("replacement root");
        let prior = fs::canonicalize(prior).expect("canonical prior");
        let admitted = fs::canonicalize(admitted).expect("canonical admitted");
        let replacement = fs::canonicalize(replacement).expect("canonical replacement");
        let admitted_root =
            SourceRoot::capture(admitted.clone()).expect("capture admitted identity");
        let workspace = WorkspaceState::granted(prior.clone());
        let watcher = WatcherState(Mutex::new(None));
        let armed_gen = AtomicU64::new(0);
        watch_and_commit(
            prior.to_str().expect("utf8 test path"),
            &workspace,
            &watcher,
            &armed_gen,
            |_result: DebounceEventResult| {},
        )
        .expect("arm prior watcher");
        let before = workspace.current().expect("prior epoch");
        let before_armed = armed_gen.load(Ordering::Acquire);
        assert!(watcher.0.lock().expect("watcher lock").is_some());

        fs::remove_dir(&admitted).expect("remove admitted root");
        symlink(&replacement, &admitted).expect("substitute symlink");

        let error = watch_and_commit_path_exact(
            &admitted_root,
            &workspace,
            &watcher,
            &armed_gen,
            |_result: DebounceEventResult| {},
        )
        .expect_err("changed canonical identity must not be watched");
        assert!(error.contains("identity changed"));
        let after = workspace.current().expect("prior epoch remains");
        assert!(Arc::ptr_eq(&before, &after));
        assert_eq!(after.root, prior);
        assert_eq!(after.generation, before.generation);
        assert_eq!(armed_gen.load(Ordering::Acquire), before_armed);
        assert!(watcher.0.lock().expect("watcher lock").is_some());
        assert!(
            !crate::engine::db_path_under(
                &temp.path().join("app-data"),
                &SourceRoot::capture(replacement.clone()).expect("capture replacement identity")
            )
            .exists(),
            "a refused target never reaches index creation"
        );
    }

    #[cfg(unix)]
    #[test]
    fn exact_watch_revalidates_after_validation_before_arming() {
        use std::os::unix::fs::symlink;
        use std::sync::Arc;

        let temp = TempDir::new().expect("tempdir");
        let prior = temp.path().join("prior");
        let admitted = temp.path().join("admitted");
        let replacement = temp.path().join("replacement");
        fs::create_dir_all(&prior).expect("prior root");
        fs::create_dir_all(&admitted).expect("admitted root");
        fs::create_dir_all(&replacement).expect("replacement root");
        let prior = fs::canonicalize(prior).expect("canonical prior");
        let admitted = fs::canonicalize(admitted).expect("canonical admitted");
        let replacement = fs::canonicalize(replacement).expect("canonical replacement");
        let expected = SourceRoot::capture(admitted.clone()).expect("capture admitted identity");
        let moved = temp.path().join("moved-admitted");
        let workspace = WorkspaceState::granted(prior.clone());
        let watcher = WatcherState::default();
        let armed_gen = AtomicU64::new(0);
        watch_and_commit(
            prior.to_str().expect("utf8 prior"),
            &workspace,
            &watcher,
            &armed_gen,
            |_result: DebounceEventResult| {},
        )
        .expect("arm prior watcher");
        let before = workspace.current().expect("prior epoch");
        let before_generation = armed_gen.load(Ordering::Acquire);

        let error = watch_and_commit_path_exact_with_hooks(
            &expected,
            &workspace,
            &watcher,
            &armed_gen,
            || {
                fs::rename(&admitted, &moved).expect("move admitted object");
                symlink(&replacement, &admitted).expect("install replacement symlink");
            },
            || {},
            || {},
            |_result: DebounceEventResult| {},
        )
        .expect_err("post-validation substitution must be refused");

        assert_eq!(error, "symlink-or-reparse");
        let after = workspace.current().expect("prior epoch remains");
        assert!(Arc::ptr_eq(&before, &after));
        assert_eq!(after.root, prior);
        assert_eq!(armed_gen.load(Ordering::Acquire), before_generation);
        let watcher_slot = watcher.0.lock().expect("watcher state");
        assert_eq!(
            watcher_slot
                .as_ref()
                .expect("prior watcher remains")
                ._root
                .identity,
            before.root_identity
        );
    }

    #[cfg(unix)]
    #[test]
    fn exact_watch_revalidates_after_watcher_creation() {
        use std::os::unix::fs::symlink;
        use std::sync::Arc;

        let temp = TempDir::new().expect("tempdir");
        let prior = temp.path().join("prior");
        let admitted = temp.path().join("admitted");
        let replacement = temp.path().join("replacement");
        fs::create_dir_all(&prior).expect("prior root");
        fs::create_dir_all(&admitted).expect("admitted root");
        fs::create_dir_all(&replacement).expect("replacement root");
        let prior = fs::canonicalize(prior).expect("canonical prior");
        let admitted = fs::canonicalize(admitted).expect("canonical admitted");
        let replacement = fs::canonicalize(replacement).expect("canonical replacement");
        let expected = SourceRoot::capture(admitted.clone()).expect("capture admitted identity");
        let moved = temp.path().join("moved-admitted");
        let workspace = WorkspaceState::granted(prior.clone());
        let watcher = WatcherState::default();
        let armed_gen = AtomicU64::new(0);
        watch_and_commit(
            prior.to_str().expect("utf8 prior"),
            &workspace,
            &watcher,
            &armed_gen,
            |_result: DebounceEventResult| {},
        )
        .expect("arm prior watcher");
        let before = workspace.current().expect("prior epoch");
        let before_generation = armed_gen.load(Ordering::Acquire);

        let error = watch_and_commit_path_exact_with_hooks(
            &expected,
            &workspace,
            &watcher,
            &armed_gen,
            || {},
            || {
                fs::rename(&admitted, &moved).expect("move watched object");
                symlink(&replacement, &admitted).expect("install replacement symlink");
            },
            || {
                fs::remove_file(&admitted).expect("remove temporary symlink");
                fs::rename(&moved, &admitted).expect("restore admitted object");
            },
            |_result: DebounceEventResult| {},
        )
        .expect_err("post-watcher substitution must be refused");

        assert_eq!(error, "symlink-or-reparse");
        let after = workspace.current().expect("prior epoch remains");
        assert!(Arc::ptr_eq(&before, &after));
        assert_eq!(after.root, prior);
        assert_eq!(armed_gen.load(Ordering::Acquire), before_generation);
        let watcher_slot = watcher.0.lock().expect("watcher state");
        assert_eq!(
            watcher_slot
                .as_ref()
                .expect("prior watcher remains")
                ._root
                .identity,
            before.root_identity
        );
    }

    #[cfg(unix)]
    #[test]
    fn exact_watch_revalidates_immediately_before_epoch_publication() {
        use std::os::unix::fs::symlink;
        use std::sync::Arc;

        let temp = TempDir::new().expect("tempdir");
        let prior = temp.path().join("prior");
        let admitted = temp.path().join("admitted");
        let replacement = temp.path().join("replacement");
        fs::create_dir_all(&prior).expect("prior root");
        fs::create_dir_all(&admitted).expect("admitted root");
        fs::create_dir_all(&replacement).expect("replacement root");
        let prior = fs::canonicalize(prior).expect("canonical prior");
        let admitted = fs::canonicalize(admitted).expect("canonical admitted");
        let replacement = fs::canonicalize(replacement).expect("canonical replacement");
        let expected = SourceRoot::capture(admitted.clone()).expect("capture admitted identity");
        let moved = temp.path().join("moved-admitted");
        let workspace = WorkspaceState::granted(prior.clone());
        let watcher = WatcherState::default();
        let armed_gen = AtomicU64::new(0);
        watch_and_commit(
            prior.to_str().expect("utf8 prior"),
            &workspace,
            &watcher,
            &armed_gen,
            |_result: DebounceEventResult| {},
        )
        .expect("arm prior watcher");
        let before = workspace.current().expect("prior epoch");
        let before_generation = armed_gen.load(Ordering::Acquire);

        let error = watch_and_commit_path_exact_with_hooks(
            &expected,
            &workspace,
            &watcher,
            &armed_gen,
            || {},
            || {},
            || {
                fs::rename(&admitted, &moved).expect("move watched object");
                symlink(&replacement, &admitted).expect("install replacement symlink");
            },
            |_result: DebounceEventResult| {},
        )
        .expect_err("pre-publication substitution must be refused");

        assert_eq!(error, "symlink-or-reparse");
        let after = workspace.current().expect("prior epoch remains");
        assert!(Arc::ptr_eq(&before, &after));
        assert_eq!(after.root, prior);
        assert_eq!(armed_gen.load(Ordering::Acquire), before_generation);
        let watcher_slot = watcher.0.lock().expect("watcher state");
        assert_eq!(
            watcher_slot
                .as_ref()
                .expect("prior watcher remains")
                ._root
                .identity,
            before.root_identity
        );
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
        assert_eq!(ok.origin_receipt.path, "note.md");
        assert_eq!(ok.origin_receipt.hash, ok.hash);
        assert_eq!(
            ok.origin_receipt.epoch,
            ws.current().unwrap().read_receipt.to_string()
        );

        // A switch INSIDE the window (injected between the read and the re-check)
        // supersedes the captured epoch → the old-root bytes are discarded, never
        // surfaced under the new workspace.
        let stale = read_doc_guarded(&ws, note.to_str().unwrap(), || {
            let _ = ws.switch(root_b.clone());
        });
        assert!(stale.is_err(), "stale read discarded");
        assert_eq!(ws.root().unwrap(), root_b);
    }

    #[cfg(unix)]
    #[test]
    fn read_uses_retained_authority_when_the_root_name_is_replaced_after_resolution() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path().join("notes");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("note.md"), "# admitted").expect("write admitted document");
        let root = fs::canonicalize(root).expect("canonical root");
        let workspace = WorkspaceState::granted(root.clone());
        let moved = temp.path().join("moved-admitted");

        let before_root = root.clone();
        let before_moved = moved.clone();
        let after_root = root.clone();
        let after_moved = moved.clone();
        let payload = read_doc_guarded_with_hooks(
            &workspace,
            root.join("note.md").to_str().expect("utf8 test path"),
            move || {
                fs::rename(&before_root, &before_moved).expect("move admitted root");
                fs::create_dir(&before_root).expect("create replacement root");
                fs::write(before_root.join("note.md"), "# replacement")
                    .expect("write replacement document");
            },
            move || {
                fs::remove_file(after_root.join("note.md")).expect("remove replacement document");
                fs::remove_dir(&after_root).expect("remove replacement root");
                fs::rename(&after_moved, &after_root).expect("restore admitted root name");
            },
        )
        .expect("the admitted object is read and its name is restored before completion");

        assert_eq!(payload.content, "# admitted");
        assert_eq!(
            fs::read_to_string(root.join("note.md")).expect("admitted document remains"),
            "# admitted"
        );
    }

    fn replaced_docs_workspace() -> (TempDir, WorkspaceState, PathBuf, PathBuf) {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path().join("notes");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("note.md"), "# original").expect("write original");
        let root = fs::canonicalize(root).expect("canonical root");
        let ws = WorkspaceState::granted(root.clone());

        let moved = temp.path().join("moved-original");
        fs::rename(&root, &moved).expect("move admitted root");
        fs::create_dir(&root).expect("create replacement root");
        let replacement = root.join("note.md");
        fs::write(&replacement, "# replacement").expect("write replacement");
        (temp, ws, moved, replacement)
    }

    #[test]
    fn read_refuses_a_replacement_at_the_active_root_path() {
        let (_temp, ws, moved, replacement) = replaced_docs_workspace();
        let read_error = read_doc_guarded(
            &ws,
            replacement.to_str().expect("utf8 replacement path"),
            || {},
        )
        .expect_err("read must refuse replacement root");
        assert!(!read_error.is_empty() && read_error.contains("identity"));
        assert_eq!(
            fs::read_to_string(&replacement).expect("replacement untouched"),
            "# replacement"
        );
        assert_eq!(
            fs::read_to_string(moved.join("note.md")).expect("original untouched"),
            "# original"
        );
    }

    #[test]
    fn save_refuses_a_replacement_at_the_active_root_path() {
        let (_temp, ws, moved, replacement) = replaced_docs_workspace();
        let save_error = save_doc_guarded(
            &ws,
            replacement.to_str().expect("utf8 replacement path"),
            "# must not land",
            &hash_of(b"# replacement"),
            false,
            commit_rename,
            || {},
            || {},
        )
        .expect_err("save must refuse replacement root");
        assert!(!save_error.is_empty() && save_error.contains("identity"));
        assert_eq!(
            fs::read_to_string(&replacement).expect("replacement untouched"),
            "# replacement"
        );
        assert_eq!(
            fs::read_to_string(moved.join("note.md")).expect("original untouched"),
            "# original"
        );
    }

    #[cfg(unix)]
    #[test]
    fn save_publishes_through_the_retained_parent_after_final_root_verification() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path().join("notes");
        fs::create_dir(&root).expect("create root");
        fs::write(root.join("note.md"), "# admitted").expect("write admitted document");
        let root = fs::canonicalize(root).expect("canonical root");
        let workspace = WorkspaceState::granted(root.clone());
        let moved = temp.path().join("moved-admitted");

        let hook_root = root.clone();
        let hook_moved = moved.clone();
        let finish_root = root.clone();
        let finish_moved = moved.clone();
        let outcome = save_doc_guarded_with_hooks(
            &workspace,
            root.join("note.md").to_str().expect("utf8 test path"),
            "# saved",
            &hash_of(b"# admitted"),
            false,
            commit_rename,
            || {},
            move || {
                fs::rename(&hook_root, &hook_moved).expect("move admitted root");
                fs::create_dir(&hook_root).expect("create replacement root");
                fs::write(hook_root.join("note.md"), "# replacement")
                    .expect("write replacement document");
            },
            move || {
                assert_eq!(
                    fs::read_to_string(finish_root.join("note.md"))
                        .expect("replacement remains readable"),
                    "# replacement",
                    "descriptor-relative publish never touched the replacement"
                );
                fs::remove_file(finish_root.join("note.md")).expect("remove replacement document");
                fs::remove_dir(&finish_root).expect("remove replacement root");
                fs::rename(&finish_moved, &finish_root).expect("restore admitted root name");
            },
        )
        .expect("save remains bound to the retained source object");

        assert_eq!(outcome.status, "saved");
        assert_eq!(
            fs::read_to_string(root.join("note.md")).expect("read restored admitted document"),
            "# saved"
        );
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
            commit_rename,
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
            commit_rename,
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
            commit_rename,
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
            commit_rename,
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
            commit_rename,
            || {},
            || {},
        )
        .is_err());
    }

    #[test]
    fn the_publisher_is_atomic_a_crash_at_the_swap_never_leaves_a_partial_file() {
        // Drive the REAL save path but INJECT the commit (publisher) step, and crash
        // right at it. With the production publisher — `commit_rename`, an atomic
        // temp→rename — the swap is indivisible: crashing immediately after it
        // leaves the WHOLE new file (and crashing before it would leave the whole
        // old file). Either way the published file is never partial.
        //
        // This test exercises the publisher as a swappable seam, so it has teeth:
        // substituting a NON-atomic (truncating in-place) publisher that dies
        // mid-copy leaves a truncated file and turns the "never partial" assertion
        // RED — the revert-verification for the atomicity contract.
        let (_dir, root) = workspace();
        let note = root.join("note.md");
        fs::write(&note, "# Original\n").unwrap();
        let ws = WorkspaceState::granted(root.clone());
        let expected = hash_of(b"# Original\n");
        let note_path = note.to_str().unwrap().to_string();
        let new = b"# The freshly-published version\n".to_vec();
        let new_str = String::from_utf8(new.clone()).unwrap();

        // Injected publisher: perform the ATOMIC swap, then crash. A reader after the
        // crash must see the whole new file — never a half-written one.
        let commit_then_crash =
            |root: &SourceRoot, tmp: &Path, dest: &Path| -> Result<(), String> {
                commit_rename(root, tmp, dest)?;
                panic!("crash the instant the atomic swap completed");
            };

        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = save_doc_guarded(
                &ws,
                &note_path,
                &new_str,
                &expected,
                false,
                commit_then_crash,
                || {},
                || {},
            );
        }));
        std::panic::set_hook(prev);
        assert!(
            result.is_err(),
            "the injected crash unwound out of the save"
        );

        // The published file is one of the two WHOLE versions — never partial. The
        // atomic rename means it is the whole NEW file here. A truncating in-place
        // publisher dying mid-copy would leave a partial file and fail this.
        let published = fs::read(&note).unwrap();
        assert!(
            published == b"# Original\n".to_vec() || published == new,
            "published file must be fully-old or fully-new, never partial: {published:?}"
        );
        assert_eq!(
            published, new,
            "the atomic swap published the whole new file"
        );
    }

    #[test]
    fn two_saves_to_different_files_in_the_same_directory_serialize() {
        use std::sync::mpsc;
        use std::sync::Arc;
        use std::thread;
        use std::time::Duration;

        // Proves the lock is per-DIRECTORY, not per-file: two saves to DIFFERENT
        // files in the same directory must serialize. Thread 1 saves a.md and PARKS
        // holding the lock (in `before_finish`, after its rename); a save of b.md
        // (a different file, same directory) must block on the shared lock.
        //
        // Revert-genuine: a per-FILE or per-inode lock gives a.md and b.md different
        // locks, so the second save runs concurrently and completes → RED. (The
        // old same-file test could not distinguish this — a per-doc lock also
        // serialized same-file saves.)
        let (_dir, root) = workspace();
        let a = root.join("a.md");
        let b = root.join("b.md");
        fs::write(&a, "A").unwrap();
        fs::write(&b, "B").unwrap();
        let ws = Arc::new(WorkspaceState::granted(root));

        let (held_tx, held_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let ws1 = Arc::clone(&ws);
        let (pa, ha) = (a.to_str().unwrap().to_string(), hash_of(b"A"));
        let t1 = thread::spawn(move || {
            save_doc_guarded(
                &ws1,
                &pa,
                "A2",
                &ha,
                false,
                commit_rename,
                || {},
                move || {
                    held_tx.send(()).unwrap(); // renamed a.md; lock still held
                    release_rx.recv().unwrap(); // park while holding the directory lock
                },
            )
        });

        held_rx.recv().unwrap(); // thread 1 holds the directory lock

        // Thread 2 saves a DIFFERENT file in the SAME directory.
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let ws2 = Arc::clone(&ws);
        let (pb, hb) = (b.to_str().unwrap().to_string(), hash_of(b"B"));
        let t2 = thread::spawn(move || {
            let out = save_doc_guarded(&ws2, &pb, "B2", &hb, false, commit_rename, || {}, || {});
            done_tx.send(()).unwrap();
            out
        });

        // With a per-directory lock the second save stays BLOCKED; a per-file/inode
        // lock (different lock per file) would let it complete concurrently.
        let still_blocked = done_rx.recv_timeout(Duration::from_millis(500)).is_err();
        assert!(
            still_blocked,
            "saves to different files in the same directory must serialize on the shared lock"
        );

        // Release thread 1; both saves succeed (different files, no conflict).
        release_tx.send(()).unwrap();
        let out1 = t1.join().unwrap().expect("thread 1 save");
        let out2 = t2.join().unwrap().expect("thread 2 save");
        assert_eq!(out1.status, "saved");
        assert_eq!(out2.status, "saved");
        assert_eq!(fs::read(&a).unwrap(), b"A2");
        assert_eq!(fs::read(&b).unwrap(), b"B2");
    }

    #[test]
    fn saving_never_leaves_a_per_document_lock_artifact() {
        // The save lock is per-DIRECTORY, so saving many docs never accretes one
        // hidden lock file per doc. Reverting to a per-document `.<name>.savelock`
        // sidecar would leave one artifact per saved doc → this goes RED.
        let (_dir, root) = workspace();
        let a = root.join("a.md");
        let b = root.join("b.md");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();
        let ws = WorkspaceState::granted(root.clone());

        save_doc_guarded(
            &ws,
            a.to_str().unwrap(),
            "a2",
            &hash_of(b"a"),
            false,
            commit_rename,
            || {},
            || {},
        )
        .unwrap();
        save_doc_guarded(
            &ws,
            b.to_str().unwrap(),
            "b2",
            &hash_of(b"b"),
            false,
            commit_rename,
            || {},
            || {},
        )
        .unwrap();

        let locks = fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".savelock"))
            .count();
        assert!(
            locks <= 1,
            "saves must not leave a per-document lock artifact; found {locks} .savelock files"
        );
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
            commit_rename,
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
            commit_rename,
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
            commit_rename,
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
            "pick_workspace_folder", // only returns a picked path
        ];
        // Built at runtime so this literal never appears in the scanned source
        // (which would self-match); the test module is excluded regardless. The
        // closing bracket is deliberately NOT part of the marker: commands carry
        // the off-UI-thread `(async)` dispatch flag (see
        // `no_command_runs_on_the_ui_thread`), and a marker that demanded `]`
        // would silently stop scanning every one of them.
        let marker = format!("#[tauri::{}", "command");
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
                // Engine commands are intentionally thin after the SDK cutover.
                // Check their exact delegation, then the actual guarded module.
                let delegated = match name {
                    "start_import" | "import_status" => {
                        Some(("index", include_str!("engine/index.rs")))
                    }
                    "list_workspace_files" | "resolve_wikilink" => {
                        Some(("navigation", include_str!("engine/navigation.rs")))
                    }
                    _ => None,
                };
                let guarded = if let Some((module, implementation)) = delegated {
                    assert!(
                        block.contains(&format!("{module}::{name}(")),
                        "thin command must call its guarded implementation"
                    );
                    implementation.contains(".finish(")
                } else {
                    block.contains(".finish(")
                };
                assert!(
                    guarded,
                    "command `{name}` returns workspace data but never reaches the \
                     Epoch::finish choke-point — route its result through it (// EPOCH-GUARDED)"
                );
            }
        }
    }

    /// Tighter than the choke-point scan above (which is satisfied merely by the
    /// guarded core sitting in the same block): the PUBLIC `save_doc` command body
    /// must be EXACTLY ONE expression statement whose expression is a BARE CALL to
    /// `save_doc_guarded`. Parsed with `syn` into a real AST, so a method-chain
    /// bypass like `save_doc_guarded(..).inspect(|_| unguarded())` — which a
    /// text/brace scan would pass — is rejected (it is an `Expr::MethodCall`, not a
    /// bare `Expr::Call`). Closes the "add a bypass path to the thin wrapper" hole.
    #[test]
    fn save_doc_command_body_is_a_single_bare_call_to_the_guarded_core() {
        let file: syn::File =
            syn::parse_str(include_str!("commands.rs")).expect("commands.rs parses");

        let func = file
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Fn(func) if func.sig.ident == "save_doc" => Some(func),
                _ => None,
            })
            .expect("the save_doc command exists");

        // Exactly one statement.
        assert_eq!(
            func.block.stmts.len(),
            1,
            "save_doc's body must be exactly one statement (the delegation)"
        );

        // …and that statement must be a bare CALL expression (the fn's tail value),
        // whose callee is the path `save_doc_guarded` — no method chain, no wrapper.
        let expr = match &func.block.stmts[0] {
            syn::Stmt::Expr(expr, None) => expr,
            _ => panic!("save_doc's single statement must be the tail delegation expression"),
        };
        let call = match expr {
            syn::Expr::Call(call) => call,
            _ => panic!(
                "save_doc's body must be a BARE call to save_doc_guarded — no method chain \
                 (e.g. `.inspect(..)`) or other wrapper"
            ),
        };
        let callee = match &*call.func {
            syn::Expr::Path(path) => path.path.segments.last().map(|seg| seg.ident.to_string()),
            _ => None,
        };
        assert_eq!(
            callee.as_deref(),
            Some("save_doc_guarded"),
            "save_doc must delegate directly to save_doc_guarded"
        );

        // Inspecting the callee alone is not enough: a truncating or side-effecting
        // publisher passed AS THE ARGUMENT would still pass. Assert the argument
        // wiring — all 8 args, the publisher (index 5) being the real atomic
        // `commit_rename` path (not a substitute closure), and the two test hooks
        // (6, 7) being EMPTY closures (no injected side-effects) in production.
        let args: Vec<&syn::Expr> = call.args.iter().collect();
        assert_eq!(
            args.len(),
            8,
            "save_doc_guarded must be called with its full 8-arg wiring"
        );

        let publisher = match args[5] {
            syn::Expr::Path(path) => path.path.segments.last().map(|seg| seg.ident.to_string()),
            _ => None,
        };
        assert_eq!(
            publisher.as_deref(),
            Some("commit_rename"),
            "the publisher argument must be the atomic commit_rename — not a substitute closure"
        );

        let is_empty_closure = |expr: &syn::Expr| match expr {
            syn::Expr::Closure(closure) => {
                matches!(&*closure.body, syn::Expr::Block(block) if block.block.stmts.is_empty())
            }
            _ => false,
        };
        assert!(
            is_empty_closure(args[6]),
            "the before_rename hook must be an empty closure in production (no injected work)"
        );
        assert!(
            is_empty_closure(args[7]),
            "the before_finish hook must be an empty closure in production (no injected work)"
        );
    }
}
