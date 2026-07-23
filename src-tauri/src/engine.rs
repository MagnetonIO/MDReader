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
//! ## Embedder (P3 choice)
//!
//! P3 uses the deterministic [`HashEmbedder`]: imports stay offline and fast, so
//! progress and wikilink features are exercisable without a model download. The
//! semantic (fastembed) provider arrives with search in P4; the engine records
//! the model on its index version, so the swap is a one-line change here.
//!
//! ## Progress granularity (honest)
//!
//! `catdb-engine`'s `import` is one atomic call — it exposes no per-file
//! callback — so progress is coarse by construction: a pre-scan yields the file
//! *total*, the StatusBar spins through the `importing` phase, and the final
//! report fills in files/chunks on `done` (or the message on `error`). Finer
//! streaming would need engine changes, which are out of this lane's scope.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use catdb_embed::{Embedder, HashEmbedder};
use catdb_engine::{EmbeddedCatDb, EngineConfig, ImportOptions, ImportReport};
use catdb_storage::DocumentRow;
use mdreader_ingest::{registration, scan_root, ScanOptions};

use crate::commands::{canonical_dir, mtime_millis, resolve_dir, FileEntry, WorkspaceState};

/// Default per-file size ceiling the ingest scan honors (mirrors the engine's
/// `ImportOptions` default of 2 MiB); the fs-fallback listing uses the same
/// bound so the two views agree on what counts as a document.
const MAX_FILE_SIZE: u64 = 2 * 1024 * 1024;

// --------------------------------------------------------------------------
// State
// --------------------------------------------------------------------------

/// The engine for the currently granted workspace, plus the last import status.
/// One workspace is active at a time (mirroring [`WorkspaceState`]'s single
/// grant); switching folders replaces the cached engine.
#[derive(Default)]
pub struct EngineState {
    inner: Mutex<EngineInner>,
    status: Mutex<ImportProgress>,
}

#[derive(Default)]
struct EngineInner {
    /// The opened engine, or `None` before the first `start_import`.
    engine: Option<Arc<EmbeddedCatDb>>,
    /// Canonical source root the cached engine imports from.
    root: Option<PathBuf>,
}

// --------------------------------------------------------------------------
// Wire types
// --------------------------------------------------------------------------

/// A snapshot of index-build progress: the payload streamed on the import
/// [`Channel`] *and* the value [`import_status`] returns. Deliberately coarse —
/// see the module docs on progress granularity.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ImportProgress {
    /// `"idle"` (no import yet), `"importing"` (in flight), `"done"`, or
    /// `"error"`.
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

/// Open (creating if needed) the engine at `db_path`, composed with the MDReader
/// domain and the deterministic hash embedder.
fn open_engine(db_path: &Path) -> Result<Arc<EmbeddedCatDb>, String> {
    let embedder = Arc::new(HashEmbedder::new()) as Arc<dyn Embedder>;
    let mut config = EngineConfig::new(db_path, embedder).with_actor("mdreader-app");
    config.domains = vec![registration()];
    EmbeddedCatDb::open(config)
        .map(Arc::new)
        .map_err(|err| format!("open index: {err}"))
}

/// Return the cached engine for `root`, opening and caching it if the grant
/// changed. Yields the engine plus its canonical source root.
fn ensure_engine(
    state: &EngineState,
    app: &AppHandle,
    root: &str,
) -> Result<(Arc<EmbeddedCatDb>, PathBuf), String> {
    let canonical = canonical_dir(root)?;
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "engine state poisoned".to_string())?;
    if let (Some(engine), Some(cached)) = (&inner.engine, &inner.root) {
        if *cached == canonical {
            return Ok((Arc::clone(engine), canonical));
        }
    }
    let base = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("no app data dir: {err}"))?;
    let db_path = db_path_under(&base, &canonical);
    let engine = open_engine(&db_path)?;
    // A workspace switch abandons the previous status too (it belonged to the
    // old root); reset so the StatusBar does not show a stale "done".
    if let Ok(mut status) = state.status.lock() {
        *status = ImportProgress::default();
    }
    inner.engine = Some(Arc::clone(&engine));
    inner.root = Some(canonical.clone());
    Ok((engine, canonical))
}

/// Record `progress` as the latest status and stream it on the import channel
/// (a closed channel is ignored — the frontend also gets the final value from
/// `start_import`'s return).
fn publish(state: &EngineState, channel: &Channel<ImportProgress>, progress: ImportProgress) {
    if let Ok(mut status) = state.status.lock() {
        *status = progress.clone();
    }
    let _ = channel.send(progress);
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

/// Open (or reuse) the workspace index, register `root` as its source, and run
/// an incremental import — streaming coarse progress on `on_event` and returning
/// the final status. The frontend fires this without awaiting (progress flows on
/// the channel) but the resolved value is authoritative, so a mocked IPC (which
/// need not drive the channel) still lands the StatusBar on `done`.
#[tauri::command]
#[specta::specta]
pub async fn start_import(
    app: AppHandle,
    root: String,
    on_event: Channel<ImportProgress>,
    engine: State<'_, EngineState>,
) -> Result<ImportProgress, String> {
    let (db, canonical) = ensure_engine(&engine, &app, &root)?;
    let total = scan_count(&canonical);
    publish(&engine, &on_event, ImportProgress::importing(total));

    // The engine serializes imports internally (single-writer); a concurrent
    // call simply queues, so re-firing on a rapid re-grant is safe.
    let outcome = db.import(&canonical, ImportOptions::default()).await;
    let final_status = match outcome {
        Ok(report) => ImportProgress::done(&report),
        Err(err) => ImportProgress::failed(total, err.to_string()),
    };
    publish(&engine, &on_event, final_status.clone());
    Ok(final_status)
}

/// The latest import status — for the StatusBar's initial render and any poll.
#[tauri::command]
#[specta::specta]
pub fn import_status(engine: State<'_, EngineState>) -> Result<ImportProgress, String> {
    engine
        .status
        .lock()
        .map(|status| status.clone())
        .map_err(|_| "engine state poisoned".to_string())
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

    let index_done = engine
        .status
        .lock()
        .map(|status| status.is_done())
        .unwrap_or(false);
    if index_done {
        let cached = {
            let inner = engine
                .inner
                .lock()
                .map_err(|_| "engine state poisoned".to_string())?;
            match (&inner.engine, &inner.root) {
                (Some(engine), Some(src_root)) => Some((Arc::clone(engine), src_root.clone())),
                _ => None,
            }
        };
        if let Some((db, src_root)) = cached {
            let entries = index_listing(&db, &src_root, &root_path)?;
            if !entries.is_empty() {
                return Ok(WorkspaceListing {
                    entries,
                    from_index: true,
                });
            }
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

/// Resolve a `[[wikilink]]` against the index. `from_doc` is the absolute path of
/// the document the link sits in (reserved for future same-directory
/// disambiguation); `target` is the raw link text; `heading` (optional) is the
/// `#heading` suffix, resolved to a source line in the target document. Matching
/// is filename-first (stem, case-insensitive), then document title; several
/// matches yield `ambiguous` with the candidate set.
#[tauri::command]
#[specta::specta]
pub fn resolve_wikilink(
    from_doc: String,
    target: String,
    heading: Option<String>,
    engine: State<'_, EngineState>,
) -> Result<WikiResolution, String> {
    let _ = from_doc;
    let cached = {
        let inner = engine
            .inner
            .lock()
            .map_err(|_| "engine state poisoned".to_string())?;
        match (&inner.engine, &inner.root) {
            (Some(engine), Some(src_root)) => Some((Arc::clone(engine), src_root.clone())),
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

/// A document's title: the outermost heading of its first section, if any.
fn title_of(db: &EmbeddedCatDb, document_id: uuid::Uuid) -> Result<Option<String>, String> {
    let sections = db
        .store()
        .sections_for_document(document_id)
        .map_err(|err| format!("sections: {err}"))?;
    Ok(sections
        .first()
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
}
