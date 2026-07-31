//! The git source: commit timestamp and author for a path.
//!
//! # Its honest declaration
//!
//! | attribute | type | pushdown | note |
//! |---|---|---|---|
//! | `path` | `Text` | `Eq`, `Prefix` | `git ls-files` takes a pathspec |
//! | `modifiedAt` | `Timestamp` | *none* | the last commit touching a path is per-path work; there is nothing to narrow with |
//! | `author` | `Text` | *none* | likewise |
//!
//! It does not observe `size`, `title` or `body`.
//!
//! The glob is deliberately **not** pushed as a git pathspec even though git
//! accepts one: git's wildmatch and [`catdb_query::Glob`] disagree about
//! whether `*` crosses `/`, and a pushdown whose semantics differ from the
//! residual it replaces is worse than no pushdown. Only the literal prefix
//! goes down.
//!
//! # Not a repository is a declaration
//!
//! Pointed at a directory that is not inside a work tree, the source constructs
//! without spawning a process, then probes lazily under the caller's deadline
//! and cancellation context and refuses the read with
//! [`SourceError::Unavailable`]. It never panics, and it never stands in an
//! empty answer for "there is no repository here".
//!
//! # Read-only
//!
//! Three commands, all read-only: `rev-parse`, `ls-files`, `log`. Nothing here
//! writes to the object store, the index, or the work tree.

use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use catdb_core::Schema;
use catdb_query::EntityQuery;

use crate::{
    Availability, DataSource, ReadContext, SourceAttr, SourceCapabilities, SourceCursor,
    SourceError, SourceFreshness, SourceId, SourceModel, SourcePage, SourcePlan,
};

const MAX_GIT_PATH_BYTES: usize = 4 * 1024;
const MAX_GIT_INDEX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_GIT_STDOUT_BYTES: usize = 1024 * 1024;
const MAX_GIT_STDERR_BYTES: usize = 64 * 1024;

#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

#[derive(Debug)]
struct GitBudget {
    remaining_work: usize,
    remaining_bytes: usize,
}

impl GitBudget {
    fn new(context: &ReadContext) -> Self {
        Self {
            remaining_work: context.remaining_rows(),
            remaining_bytes: context.remaining_bytes(),
        }
    }

    fn take_work(&mut self) -> Result<(), SourceError> {
        self.remaining_work = self
            .remaining_work
            .checked_sub(1)
            .ok_or(SourceError::RowBudgetExceeded)?;
        Ok(())
    }

    fn take_bytes(&mut self, bytes: usize) -> Result<(), SourceError> {
        self.remaining_bytes = self
            .remaining_bytes
            .checked_sub(bytes)
            .ok_or(SourceError::ByteBudgetExceeded)?;
        Ok(())
    }
}

#[derive(Debug)]
struct GitListing {
    listed: Vec<String>,
    scanned: usize,
    last_examined: Option<String>,
    has_more: bool,
}

/// Aggregate payload accounting for one stdout/stderr reader pair.
///
/// One token per reader is removed from the local quantum up front and held as
/// that reader's EOF proof. A real byte is read under the proof, then must
/// transfer to the shared payload pool before it is accepted. Therefore an
/// aggregate raw payload of `N` requires exactly `N + 2` local bytes regardless
/// of which pipe reaches EOF first. A failed transfer has read at most the
/// proof-covered byte, so the two physical readers together never read beyond
/// the original local quantum.
#[derive(Debug)]
struct AggregatePipeAllowance {
    remaining_payload: Arc<AtomicUsize>,
    initial_payload: usize,
}

impl AggregatePipeAllowance {
    fn pair(bytes: usize) -> Option<(Self, PipeAllowance, PipeAllowance)> {
        let initial_payload = bytes.checked_sub(2)?;
        let remaining_payload = Arc::new(AtomicUsize::new(initial_payload));
        Some((
            Self {
                remaining_payload: remaining_payload.clone(),
                initial_payload,
            },
            PipeAllowance(remaining_payload.clone()),
            PipeAllowance(remaining_payload),
        ))
    }

    fn used(&self) -> usize {
        self.initial_payload
            .saturating_sub(self.remaining_payload.load(AtomicOrdering::SeqCst))
    }
}

/// One reader's exclusively reserved EOF proof plus the shared payload pool.
#[derive(Debug)]
struct PipeAllowance(Arc<AtomicUsize>);

impl PipeAllowance {
    fn take_payload(&self) -> bool {
        self.0
            .fetch_update(
                AtomicOrdering::SeqCst,
                AtomicOrdering::SeqCst,
                |remaining| remaining.checked_sub(1),
            )
            .is_ok()
    }
}

/// Reads commit metadata for the tracked files under a directory.
#[derive(Debug, Clone)]
pub struct GitSource {
    model: SourceModel,
    root: PathBuf,
    contained_root: Option<crate::walk::ContainedRoot>,
    git_binary: PathBuf,
    cursor_key: [u8; 32],
}

impl GitSource {
    #[cfg(unix)]
    const TRUSTED_GIT_COMMAND: &'static str = "/usr/bin/git";
    #[cfg(windows)]
    const TRUSTED_GIT_COMMAND: &'static str = r"C:\Program Files\Git\cmd\git.exe";
    #[cfg(not(any(unix, windows)))]
    const TRUSTED_GIT_COMMAND: &'static str = "git";

    fn command(&self, args: &[&str]) -> Result<std::process::Command, SourceError> {
        let mut components = self.git_binary.components();
        let is_bare_name = matches!(
            (components.next(), components.next()),
            (Some(std::path::Component::Normal(_)), None)
        );
        let executable_is_allowed = if cfg!(any(unix, windows)) {
            self.git_binary.is_absolute()
        } else {
            self.git_binary.is_absolute() || is_bare_name
        };
        if !executable_is_allowed {
            return Err(SourceError::Containment {
                source_id: self.model.id().clone(),
                path: self.git_binary.display().to_string(),
                detail: "runtime Git executable must be absolute on Unix and Windows; unsupported platforms may use only an absolute path or a bare name".to_string(),
            });
        }
        let mut command = std::process::Command::new(&self.git_binary);
        configure_hermetic_git_environment(&mut command).map_err(|error| SourceError::Read {
            source_id: self.model.id().clone(),
            path: self.root.display().to_string(),
            detail: format!("could not construct the fixed Git process environment: {error}"),
        })?;
        command.arg("-C").arg(".").args(args);
        self.root_authority()?
            .configure_command(&mut command)
            .map_err(|error| crate::walk::map_error(error, self.model.id(), &self.root))?;
        configure_process_group(&mut command);
        Ok(command)
    }

    /// The source's name, as it appears on every row it produces.
    pub const ID: &'static str = "git";

    fn attributes() -> Vec<SourceAttr> {
        vec![
            SourceAttr::text("path").pushes([
                catdb_storage::ObjectFilterOp::Eq,
                catdb_storage::ObjectFilterOp::Prefix,
            ]),
            SourceAttr::parsed("modifiedAt", catdb_core::BaseType::Timestamp),
            SourceAttr::text("author"),
        ]
    }

    /// Open a potential Git work tree. Repository probing is deferred until a
    /// bounded read context is available.
    pub fn open(root: impl AsRef<Path>) -> Self {
        Self::open_with_git_binary(root, Self::TRUSTED_GIT_COMMAND)
    }

    /// Preserve the connector's declared schema while explicitly refusing
    /// access to a registered physical source that is not currently usable.
    pub fn unavailable(root: impl AsRef<Path>, reason: impl Into<String>) -> Self {
        let root = root.as_ref().to_path_buf();
        Self {
            model: SourceModel::unavailable_with_attrs(
                SourceId::new(Self::ID),
                "doc",
                "Document",
                Self::attributes(),
                reason,
            ),
            root,
            contained_root: None,
            git_binary: PathBuf::from(Self::TRUSTED_GIT_COMMAND),
            cursor_key: crate::walk::new_cursor_key(),
        }
    }

    /// Open a potential Git work tree through an already-admitted directory
    /// authority without reopening its pathname.
    pub fn open_directory_authority(authority: &crate::DirectoryAuthority) -> Self {
        Self::open_directory_authority_with_git_binary(authority, Self::TRUSTED_GIT_COMMAND)
    }

    /// Open a potential repository with an explicit Git executable.
    ///
    /// This supports deployments with an explicitly configured absolute Git
    /// executable and makes command-failure behavior testable without
    /// mutating process-global `PATH`. Unix and Windows require an absolute
    /// executable because Windows process resolution may fall back to the
    /// parent's `PATH`; unsupported platforms may use a bare name. The
    /// executable is deliberately not part of
    /// [`DataSource::source_spec`], so catalog metadata can never choose a
    /// program.
    pub fn open_with_git_binary(root: impl AsRef<Path>, git_binary: impl AsRef<Path>) -> Self {
        let requested_root = root.as_ref().to_path_buf();
        let contained_root = crate::walk::ContainedRoot::open(&requested_root);
        Self::from_open_attempt(
            requested_root,
            contained_root,
            git_binary.as_ref().to_path_buf(),
        )
    }

    /// Open through an already-admitted directory authority with an explicit
    /// Git executable.
    pub fn open_directory_authority_with_git_binary(
        authority: &crate::DirectoryAuthority,
        git_binary: impl AsRef<Path>,
    ) -> Self {
        Self::from_open_attempt(
            authority.display_root().to_path_buf(),
            Ok(authority.contained_root().clone()),
            git_binary.as_ref().to_path_buf(),
        )
    }

    fn from_open_attempt(
        requested_root: PathBuf,
        contained_root: Result<crate::walk::ContainedRoot, crate::walk::WalkError>,
        git_binary: PathBuf,
    ) -> Self {
        let root = contained_root
            .as_ref()
            .map(|root| root.canonical().to_path_buf())
            .unwrap_or(requested_root);
        let id = SourceId::new(Self::ID);
        let model = match &contained_root {
            Ok(_) => SourceModel::new(id, "doc", "Document", Self::attributes()),
            Err(error) => SourceModel::unavailable(
                id,
                "doc",
                "Document",
                format!(
                    "`{}` is not a directory or has no secure contained-open root: {error}",
                    root.display()
                ),
            ),
        };
        Self {
            model,
            root,
            contained_root: contained_root.ok(),
            git_binary,
            cursor_key: crate::walk::new_cursor_key(),
        }
    }

    /// Reopen a source from its exact V1 catalog body.
    ///
    /// The catalog pins only the physical repository root. It never contains
    /// an executable path: catalog-reopened sources use the runtime's trusted
    /// `git` command. The root must exist while its catalog version is
    /// authored because an unavailable instance has no physical identity to
    /// pin.
    pub fn from_source_spec(
        spec: &serde_json::Value,
    ) -> Result<Self, crate::CatalogSourceSpecError> {
        let object = crate::source_spec_object(spec, "git", &["format", "kind", "root"])?;
        let contained = crate::contained_root_from_spec(
            object.get("root").ok_or(crate::CatalogSourceSpecError)?,
        )?;
        Ok(Self::from_open_attempt(
            contained.canonical().to_path_buf(),
            Ok(contained),
            PathBuf::from(Self::TRUSTED_GIT_COMMAND),
        ))
    }

    /// The root this source reads.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[async_trait]
impl DataSource for GitSource {
    fn id(&self) -> &SourceId {
        self.model.id()
    }

    fn schema(&self) -> &Schema {
        self.model.schema()
    }

    fn capabilities(&self) -> &SourceCapabilities {
        self.model.capabilities()
    }

    fn source_spec(&self) -> serde_json::Value {
        serde_json::json!({
            "format": crate::LOCAL_SOURCE_FORMAT,
            "kind": "git",
            "root": crate::root_authority_spec(
                &self.root,
                self.contained_root.as_ref().map(crate::walk::ContainedRoot::identity),
            ),
        })
    }

    async fn freshness(&self, context: &ReadContext) -> Result<SourceFreshness, SourceError> {
        context.check()?;
        let source = self.clone();
        let owned_context = context.clone();
        tokio::task::spawn_blocking(move || source.measure_freshness(&owned_context))
            .await
            .map_err(|_| SourceError::WorkerFailed)?
    }

    fn compile(
        &self,
        query: &EntityQuery,
        authorized_fields: &[String],
    ) -> Result<SourcePlan, SourceError> {
        self.model.compile(query, authorized_fields)
    }

    async fn execute_page(
        &self,
        plan: &SourcePlan,
        cursor: Option<&SourceCursor>,
        context: &ReadContext,
    ) -> Result<SourcePage, SourceError> {
        context.check()?;
        let source = self.clone();
        let owned_plan = plan.clone();
        let owned_cursor = cursor.cloned();
        let owned_context = context.clone();
        tokio::task::spawn_blocking(move || {
            source.execute_bounded(&owned_plan, owned_cursor.as_ref(), &owned_context)
        })
        .await
        .map_err(|_| SourceError::WorkerFailed)?
    }
}

impl GitSource {
    fn measure_freshness(&self, context: &ReadContext) -> Result<SourceFreshness, SourceError> {
        let started_at_ms = crate::observation_now_ms();
        if matches!(
            self.model.capabilities().availability(),
            Availability::Available
        ) {
            self.verify_root_authority()?;
            let mut budget = GitBudget::new(context);
            self.ensure_repository(context, &mut budget)?;
            self.read_head(context, &mut budget)?;
        }
        context.check()?;
        Ok(crate::freshness_since(started_at_ms))
    }

    fn execute_bounded(
        &self,
        plan: &SourcePlan,
        cursor: Option<&SourceCursor>,
        context: &ReadContext,
    ) -> Result<SourcePage, SourceError> {
        let started_at_ms = crate::observation_now_ms();
        self.model.admit(plan)?;
        let mut budget = GitBudget::new(context);
        self.ensure_repository(context, &mut budget)?;
        let cursor = crate::walk::decode_cursor(cursor, &self.cursor_key)?;
        let head = self.read_head(context, &mut budget)?;
        let snapshot = self.snapshot(head.as_deref(), context, &mut budget)?;
        if cursor.as_ref().is_some_and(|cursor| {
            cursor.snapshot.as_deref() != Some(snapshot.as_str())
                || cursor.walk.is_some()
                || cursor.after.is_none()
        }) {
            return Err(SourceError::InvalidCursor);
        }
        let after = cursor
            .as_ref()
            .and_then(|cursor| cursor.after.as_deref())
            .unwrap_or("");
        let prefix = crate::plan::pushed_prefix(plan, "path");
        let exact_path = plan.pushed.filters.iter().any(|filter| {
            filter.attr == "path"
                && filter.op == catdb_storage::ObjectFilterOp::Eq
                && matches!(
                    &filter.value,
                    catdb_storage::ObjectValue::Text(value) if value == &prefix
                )
        });
        let listing = self.list_page(&prefix, exact_path, after, context, &mut budget)?;
        let mut rows = Vec::new();
        for rel in &listing.listed {
            context.check()?;
            let mut raw = std::collections::BTreeMap::new();
            raw.insert("path".to_string(), rel.clone());
            if let Some(head) = head.as_deref() {
                if let Some((epoch_seconds, author)) =
                    self.last_commit(head, rel, context, &mut budget)?
                {
                    // Git speaks seconds; the declared attribute is epoch
                    // milliseconds. Only the unit is the adapter's business —
                    // the typing is the shared transformation.
                    raw.insert("modifiedAt".to_string(), (epoch_seconds * 1000).to_string());
                    raw.insert("author".to_string(), author);
                }
            }
            let row = self.model.row(rel, raw)?;
            if !plan
                .pushed
                .filters
                .iter()
                .all(|filter| crate::plan::filter_matches(filter, &row.value))
            {
                continue;
            }
            if crate::plan::passes_all(&plan.residual, &row.value) {
                let mut row = row;
                if let Some(fields) = row.value.as_object_mut() {
                    fields.retain(|field, _| plan.projected.contains(field));
                }
                rows.push(row);
                if rows.len() == context.page_size() {
                    break;
                }
            }
        }
        let ending_head = self.read_head(context, &mut budget)?;
        let ending_snapshot = self.snapshot(ending_head.as_deref(), context, &mut budget)?;
        if ending_snapshot != snapshot {
            return Err(SourceError::Read {
                source_id: self.model.id().clone(),
                path: self.root.display().to_string(),
                detail:
                    "repository HEAD or index changed while constructing the page; retry the read"
                        .to_string(),
            });
        }
        let next_cursor = if listing.has_more {
            let after = listing.last_examined.ok_or(SourceError::InvalidCursor)?;
            Some(crate::walk::encode_cursor(
                &crate::walk::LocalCursor {
                    after: Some(after),
                    walk: None,
                    snapshot: Some(snapshot),
                },
                &self.cursor_key,
            )?)
        } else {
            None
        };
        context.check()?;
        let freshness = crate::freshness_since(started_at_ms);
        Ok(SourcePage {
            source: self.model.id().clone(),
            scanned: listing.scanned,
            rows,
            next_cursor,
            observed_at_ms: freshness.observed_at_ms,
            freshness_ms: freshness.freshness_ms,
        })
    }
}

impl GitSource {
    fn ensure_repository(
        &self,
        context: &ReadContext,
        budget: &mut GitBudget,
    ) -> Result<(), SourceError> {
        let args = ["rev-parse", "--is-inside-work-tree"];
        let output = self.run_captured(&args, context, budget)?;
        if output.status.success()
            && std::str::from_utf8(&output.stdout).is_ok_and(|stdout| stdout.trim() == "true")
        {
            return Ok(());
        }
        let has_repository_marker = self
            .root_authority()?
            .has_entry(Path::new(".git"))
            .map_err(|error| crate::walk::map_error(error, self.model.id(), &self.root))?;
        if !output.status.success() && has_repository_marker {
            return Err(self.command_failure(&args, &output.stderr));
        }
        Err(SourceError::Unavailable {
            source_id: self.model.id().clone(),
            reason: format!("`{}` is not inside a git work tree", self.root.display()),
        })
    }

    /// The tracked paths under this root, narrowed by the pushed prefix.
    ///
    /// The prefix reaches git as a pathspec only where the two agree: a git
    /// pathspec matches at path-component boundaries, so `doc` does **not**
    /// select `docs/a.md` the way a string prefix does. Where they disagree the
    /// adapter applies the prefix itself — the source still executes the filter
    /// it declared, it just cannot hand that work to git.
    fn list_page(
        &self,
        prefix: &str,
        exact_path: bool,
        after: &str,
        context: &ReadContext,
        budget: &mut GitBudget,
    ) -> Result<GitListing, SourceError> {
        self.verify_root_authority()?;
        let can_push_pathspec = !prefix.is_empty() && (prefix.ends_with('/') || exact_path);
        let pathspec = can_push_pathspec.then(|| literal_pathspec(prefix));
        let mut args = vec!["ls-files", "-z"];
        if let Some(pathspec) = pathspec.as_deref() {
            args.push("--");
            args.push(pathspec);
        }
        let fail = |detail: String| SourceError::Read {
            source_id: self.model.id().clone(),
            path: self.root.display().to_string(),
            detail,
        };
        let mut command = self.command(&args)?;
        let child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| fail(format!("`git {}` could not run: {error}", args.join(" "))))?;
        let mut child = ProcessTreeGuard::new(child)
            .map_err(|error| fail(format!("git process-tree ownership failed: {error}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| fail("git stdout unavailable".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| fail("git stderr unavailable".to_string()))?;
        let initial_listing_bytes = budget.remaining_bytes;
        let Some((allowance, record_allowance, stderr_allowance)) =
            AggregatePipeAllowance::pair(initial_listing_bytes)
        else {
            return Err(SourceError::ByteBudgetExceeded);
        };
        let (requests_tx, requests_rx) = sync_channel::<usize>(0);
        let (records_tx, records_rx) = sync_channel(0);
        std::thread::spawn(move || {
            let mut reader = BufReader::with_capacity(1, stdout);
            while let Ok(maximum) = requests_rx.recv() {
                match read_nul_record_bounded(&mut reader, maximum, &record_allowance) {
                    Ok(None) => {
                        let _ = records_tx.send(Ok(None));
                        return;
                    }
                    Ok(Some(bytes)) => {
                        if records_tx.send(Ok(Some(bytes))).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = records_tx.send(Err(error));
                        return;
                    }
                }
            }
        });
        let stderr = spawn_pipe_reader(stderr, MAX_GIT_STDERR_BYTES, stderr_allowance);
        let mut listed = Vec::with_capacity(context.page_size());
        let mut scanned = 0usize;
        let mut last_examined = None;
        loop {
            if budget.remaining_work == 0 || listed.len() == context.page_size() {
                child.terminate().map_err(|error| fail(error.to_string()))?;
                let stderr = receive_with_context(&stderr, &mut child, context)
                    .map_err(|error| error.map_disconnected(&fail, "git stderr reader failed"))?
                    .map_err(|error| fail(error.to_string()))?;
                if stderr.exceeded {
                    return Err(if stderr.budget_exhausted {
                        SourceError::ByteBudgetExceeded
                    } else {
                        SourceError::ResourceLimit {
                            source_id: self.model.id().clone(),
                            resource: "git stderr bytes",
                        }
                    });
                }
                budget.take_bytes(allowance.used())?;
                self.verify_root_authority()?;
                return Ok(GitListing {
                    listed,
                    scanned,
                    last_examined,
                    has_more: true,
                });
            }
            let record_limit = MAX_GIT_PATH_BYTES.saturating_add(1);
            if record_limit == 0 {
                return Err(SourceError::ByteBudgetExceeded);
            }
            requests_tx
                .send(record_limit)
                .map_err(|_| fail("git stdout reader stopped unexpectedly".to_string()))?;
            let Some(mut bytes) = (match receive_with_context(&records_rx, &mut child, context) {
                Ok(Ok(record)) => record,
                Ok(Err(GitRecordError::Read(error))) => {
                    return Err(fail(error));
                }
                Ok(Err(GitRecordError::Limit)) => {
                    return Err(SourceError::ResourceLimit {
                        source_id: self.model.id().clone(),
                        resource: "git path record bytes",
                    });
                }
                Ok(Err(GitRecordError::Budget)) => {
                    return Err(SourceError::ByteBudgetExceeded);
                }
                Err(BackgroundWaitError::Context(error)) => return Err(error),
                Err(BackgroundWaitError::Io(error)) => return Err(fail(error.to_string())),
                Err(BackgroundWaitError::Disconnected) => {
                    return Err(fail("git stdout reader stopped unexpectedly".to_string()));
                }
            }) else {
                break;
            };
            budget.take_work()?;
            scanned = scanned.saturating_add(1);
            if bytes.last() == Some(&0) {
                bytes.pop();
            }
            let rel = match String::from_utf8(bytes) {
                Ok(rel) => rel,
                Err(error) => {
                    let error = SourceError::UnrepresentablePath {
                        source_id: self.model.id().clone(),
                        path: String::from_utf8_lossy(error.as_bytes()).into_owned(),
                    };
                    return Err(error);
                }
            };
            last_examined = Some(rel.clone());
            if rel.as_str() <= after
                || !rel.starts_with(prefix)
                || rel.split('/').any(|part| part.starts_with('.'))
            {
                continue;
            }
            listed.push(rel);
        }
        let status =
            wait_child_with_context(&mut child, context).map_err(|error| error.map_io(&fail))?;
        let stderr = receive_with_context(&stderr, &mut child, context)
            .map_err(|error| error.map_disconnected(&fail, "git stderr reader failed"))?
            .map_err(|error| fail(error.to_string()))?;
        if stderr.exceeded {
            return Err(if stderr.budget_exhausted {
                SourceError::ByteBudgetExceeded
            } else {
                SourceError::ResourceLimit {
                    source_id: self.model.id().clone(),
                    resource: "git stderr bytes",
                }
            });
        }
        budget.take_bytes(allowance.used())?;
        if !status.success() {
            return Err(fail(format!(
                "`git ls-files` failed: {}",
                String::from_utf8_lossy(&stderr.bytes).trim()
            )));
        }
        self.verify_root_authority()?;
        Ok(GitListing {
            listed,
            scanned,
            last_examined,
            has_more: false,
        })
    }

    /// The committer timestamp (epoch seconds) and author of the last commit
    /// touching `rel`, or `None` when no commit does.
    fn last_commit(
        &self,
        head: &str,
        rel: &str,
        context: &ReadContext,
        budget: &mut GitBudget,
    ) -> Result<Option<(i64, String)>, SourceError> {
        let literal = literal_pathspec(rel);
        let output = self.run_checked(
            &[
                "log",
                "--no-ext-diff",
                "--no-textconv",
                "--no-show-signature",
                "-1",
                "--format=%ct%x09%an",
                head,
                "--",
                &literal,
            ],
            context,
            budget,
        )?;
        let line = output.trim_end_matches('\n');
        if line.is_empty() {
            return Ok(None);
        }
        let (seconds, author) = line.split_once('\t').ok_or_else(|| SourceError::Read {
            source_id: self.model.id().clone(),
            path: rel.to_string(),
            detail: format!("unreadable `git log` output: {line:?}"),
        })?;
        let seconds = seconds.parse::<i64>().map_err(|error| SourceError::Read {
            source_id: self.model.id().clone(),
            path: rel.to_string(),
            detail: format!("unreadable commit timestamp {seconds:?}: {error}"),
        })?;
        Ok(Some((seconds, author.to_string())))
    }

    fn run_checked(
        &self,
        args: &[&str],
        context: &ReadContext,
        budget: &mut GitBudget,
    ) -> Result<String, SourceError> {
        let output = self.run_checked_bytes(args, context, budget)?;
        String::from_utf8(output).map_err(|error| SourceError::Read {
            source_id: self.model.id().clone(),
            path: self.root.display().to_string(),
            detail: error.to_string(),
        })
    }

    fn run_checked_bytes(
        &self,
        args: &[&str],
        context: &ReadContext,
        budget: &mut GitBudget,
    ) -> Result<Vec<u8>, SourceError> {
        let output = self.run_captured(args, context, budget)?;
        if !output.status.success() {
            return Err(self.command_failure(args, &output.stderr));
        }
        Ok(output.stdout)
    }

    fn run_captured(
        &self,
        args: &[&str],
        context: &ReadContext,
        budget: &mut GitBudget,
    ) -> Result<GitCommandOutput, SourceError> {
        self.run_captured_with_limit(args, context, budget, MAX_GIT_STDOUT_BYTES)
    }

    fn run_captured_with_limit(
        &self,
        args: &[&str],
        context: &ReadContext,
        budget: &mut GitBudget,
        stdout_limit: usize,
    ) -> Result<GitCommandOutput, SourceError> {
        self.verify_root_authority()?;
        let fail = |detail: String| SourceError::Read {
            source_id: self.model.id().clone(),
            path: self.root.display().to_string(),
            detail,
        };
        let mut command = self.command(args)?;
        let child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| fail(format!("`git {}` could not run: {error}", args.join(" "))))?;
        let mut child = ProcessTreeGuard::new(child)
            .map_err(|error| fail(format!("git process-tree ownership failed: {error}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| fail("git stdout unavailable".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| fail("git stderr unavailable".to_string()))?;
        if budget.remaining_bytes < 2 {
            return Err(SourceError::ByteBudgetExceeded);
        }
        let (allowance, stdout_allowance, stderr_allowance) =
            AggregatePipeAllowance::pair(budget.remaining_bytes)
                .ok_or(SourceError::ByteBudgetExceeded)?;
        let stdout = spawn_pipe_reader(stdout, stdout_limit, stdout_allowance);
        let stderr = spawn_pipe_reader(stderr, MAX_GIT_STDERR_BYTES, stderr_allowance);
        let mut stdout_result = None;
        let mut stderr_result = None;
        let mut status = None;
        while status.is_none() || stdout_result.is_none() || stderr_result.is_none() {
            context.check()?;
            if stdout_result.is_none() {
                match stdout.try_recv() {
                    Ok(result) => {
                        stdout_result = Some(result.map_err(|error| fail(error.to_string()))?)
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        return Err(fail("git stdout reader failed".to_string()));
                    }
                }
            }
            if stderr_result.is_none() {
                match stderr.try_recv() {
                    Ok(result) => {
                        stderr_result = Some(result.map_err(|error| fail(error.to_string()))?)
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        return Err(fail("git stderr reader failed".to_string()));
                    }
                }
            }
            if stdout_result.as_ref().is_some_and(|pipe| pipe.exceeded)
                || stderr_result.as_ref().is_some_and(|pipe| pipe.exceeded)
            {
                let budget_exhausted = stdout_result
                    .as_ref()
                    .is_some_and(|pipe| pipe.budget_exhausted)
                    || stderr_result
                        .as_ref()
                        .is_some_and(|pipe| pipe.budget_exhausted);
                return Err(if budget_exhausted {
                    SourceError::ByteBudgetExceeded
                } else {
                    SourceError::ResourceLimit {
                        source_id: self.model.id().clone(),
                        resource: "git command output bytes",
                    }
                });
            }
            if status.is_none()
                && child
                    .poll_parent_exit()
                    .map_err(|error| fail(error.to_string()))?
            {
                status = Some(child.finish().map_err(|error| fail(error.to_string()))?);
            }
            if status.is_none() || stdout_result.is_none() || stderr_result.is_none() {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        let stdout =
            stdout_result.ok_or_else(|| fail("git stdout reader did not finish".to_string()))?;
        let stderr =
            stderr_result.ok_or_else(|| fail("git stderr reader did not finish".to_string()))?;
        budget.take_bytes(allowance.used())?;
        self.verify_root_authority()?;
        Ok(GitCommandOutput {
            status: status.ok_or_else(|| fail("git process did not exit".to_string()))?,
            stdout: stdout.bytes,
            stderr: stderr.bytes,
        })
    }

    fn run_hashed_stdout(
        &self,
        args: &[&str],
        context: &ReadContext,
        budget: &mut GitBudget,
        maximum: usize,
    ) -> Result<blake3::Hash, SourceError> {
        self.verify_root_authority()?;
        let fail = |detail: String| SourceError::Read {
            source_id: self.model.id().clone(),
            path: self.root.display().to_string(),
            detail,
        };
        let mut command = self.command(args)?;
        let child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| fail(format!("`git {}` could not run: {error}", args.join(" "))))?;
        let mut child = ProcessTreeGuard::new(child)
            .map_err(|error| fail(format!("git process-tree ownership failed: {error}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| fail("git stdout unavailable".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| fail("git stderr unavailable".to_string()))?;
        let Some((allowance, stdout_allowance, stderr_allowance)) =
            AggregatePipeAllowance::pair(budget.remaining_bytes)
        else {
            return Err(SourceError::ByteBudgetExceeded);
        };
        let stdout = spawn_hashed_pipe_reader(stdout, maximum, stdout_allowance);
        let stderr = spawn_pipe_reader(stderr, MAX_GIT_STDERR_BYTES, stderr_allowance);
        let mut stdout_result = None;
        let mut stderr_result = None;
        let mut status = None;
        while status.is_none() || stdout_result.is_none() || stderr_result.is_none() {
            context.check()?;
            if stdout_result.is_none() {
                match stdout.try_recv() {
                    Ok(result) => {
                        stdout_result = Some(result.map_err(|error| fail(error.to_string()))?)
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        return Err(fail("git stdout reader failed".to_string()));
                    }
                }
            }
            if stderr_result.is_none() {
                match stderr.try_recv() {
                    Ok(result) => {
                        stderr_result = Some(result.map_err(|error| fail(error.to_string()))?)
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        return Err(fail("git stderr reader failed".to_string()));
                    }
                }
            }
            if stdout_result.as_ref().is_some_and(|pipe| pipe.exceeded)
                || stderr_result.as_ref().is_some_and(|pipe| pipe.exceeded)
            {
                let budget_exhausted = stdout_result
                    .as_ref()
                    .is_some_and(|pipe| pipe.budget_exhausted)
                    || stderr_result
                        .as_ref()
                        .is_some_and(|pipe| pipe.budget_exhausted);
                return Err(if budget_exhausted {
                    SourceError::ByteBudgetExceeded
                } else {
                    SourceError::ResourceLimit {
                        source_id: self.model.id().clone(),
                        resource: "git index bytes",
                    }
                });
            }
            if status.is_none()
                && child
                    .poll_parent_exit()
                    .map_err(|error| fail(error.to_string()))?
            {
                status = Some(child.finish().map_err(|error| fail(error.to_string()))?);
            }
            if status.is_none() || stdout_result.is_none() || stderr_result.is_none() {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        let stdout =
            stdout_result.ok_or_else(|| fail("git stdout reader did not finish".to_string()))?;
        let stderr =
            stderr_result.ok_or_else(|| fail("git stderr reader did not finish".to_string()))?;
        budget.take_bytes(allowance.used())?;
        let status = status.ok_or_else(|| fail("git process did not exit".to_string()))?;
        if !status.success() {
            return Err(self.command_failure(args, &stderr.bytes));
        }
        self.verify_root_authority()?;
        Ok(stdout.digest)
    }

    fn command_failure(&self, args: &[&str], stderr: &[u8]) -> SourceError {
        SourceError::Read {
            source_id: self.model.id().clone(),
            path: self.root.display().to_string(),
            detail: format!(
                "`git {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(stderr).trim()
            ),
        }
    }

    fn read_head(
        &self,
        context: &ReadContext,
        budget: &mut GitBudget,
    ) -> Result<Option<String>, SourceError> {
        let args = ["rev-parse", "--verify", "--quiet", "HEAD"];
        let output = self.run_captured(&args, context, budget)?;
        if output.status.success() {
            let text = std::str::from_utf8(&output.stdout).map_err(|error| SourceError::Read {
                source_id: self.model.id().clone(),
                path: self.root.display().to_string(),
                detail: error.to_string(),
            })?;
            let head = text.trim();
            if head.is_empty() {
                return Err(
                    self.command_failure(&args, b"successful command returned no object id")
                );
            }
            return Ok(Some(head.to_string()));
        }
        if output.status.code() == Some(1)
            && output.stdout.iter().all(u8::is_ascii_whitespace)
            && output.stderr.iter().all(u8::is_ascii_whitespace)
        {
            return Ok(None);
        }
        Err(self.command_failure(&args, &output.stderr))
    }

    fn snapshot(
        &self,
        head: Option<&str>,
        context: &ReadContext,
        budget: &mut GitBudget,
    ) -> Result<String, SourceError> {
        // Git worktrees may keep their index outside the worktree root. Do
        // not follow the path returned by `--git-path`: that creates a second,
        // unversioned pathname authority and an ABA window before open.
        // Instead Git reads its own index while its cwd is descriptor-bound,
        // and we hash the complete semantic stage rows it returns.
        let args = ["ls-files", "--stage", "-z"];
        let index_digest = self.run_hashed_stdout(
            &args,
            context,
            budget,
            usize::try_from(MAX_GIT_INDEX_BYTES).unwrap_or(usize::MAX),
        )?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"catdb-git-snapshot-v1\0");
        match head {
            Some(head) => {
                hasher.update(b"head\0");
                hasher.update(head.as_bytes());
            }
            None => {
                hasher.update(b"unborn-head\0");
            }
        }
        hasher.update(b"index-stage-rows\0");
        hasher.update(index_digest.as_bytes());
        Ok(hasher.finalize().to_hex().to_string())
    }

    fn root_authority(&self) -> Result<&crate::walk::ContainedRoot, SourceError> {
        self.contained_root
            .as_ref()
            .ok_or_else(|| SourceError::Containment {
                source_id: self.model.id().clone(),
                path: self.root.display().to_string(),
                detail: "configured root has no physical identity".to_string(),
            })
    }

    fn verify_root_authority(&self) -> Result<(), SourceError> {
        self.root_authority()?
            .verify_current()
            .map_err(|error| crate::walk::map_error(error, self.model.id(), &self.root))
    }
}

/// Build a process environment that contains no ambient repository authority
/// and no application secrets.
///
/// The default executable is an absolute platform path and the child `PATH`
/// is a fixed OS path, never an inherited application value. On Windows,
/// `SystemRoot` and the fixed system path are obtained from the kernel API
/// rather than process environment. All other variables are fixed literals
/// required to keep these local plumbing reads non-interactive,
/// transport-free, and unable to execute repository hooks.
fn configure_hermetic_git_environment(
    command: &mut std::process::Command,
) -> Result<(), std::io::Error> {
    command.env_clear();
    configure_fixed_platform_environment(command)?;

    command
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", NULL_DEVICE)
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_PAGER", "")
        .env("GIT_EDITOR", "")
        .env("GIT_SEQUENCE_EDITOR", "")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_PROTOCOL_FROM_USER", "0");

    // Command-scope configuration outranks repository/worktree configuration.
    // The source still reads repository structure, but repo-controlled hooks,
    // credential helpers, editors, pagers, and signature programs are not
    // executable surfaces of a semantic read.
    let config = [
        ("core.fsmonitor", "false"),
        ("core.hooksPath", NULL_DEVICE),
        ("core.pager", "cat"),
        ("core.editor", "false"),
        ("sequence.editor", "false"),
        ("core.askPass", ""),
        ("credential.helper", ""),
        ("credential.interactive", "never"),
        ("log.showSignature", "false"),
    ];
    command.env("GIT_CONFIG_COUNT", config.len().to_string());
    for (index, (key, value)) in config.into_iter().enumerate() {
        command
            .env(format!("GIT_CONFIG_KEY_{index}"), key)
            .env(format!("GIT_CONFIG_VALUE_{index}"), value);
    }
    Ok(())
}

#[cfg(unix)]
fn configure_fixed_platform_environment(
    command: &mut std::process::Command,
) -> Result<(), std::io::Error> {
    command.env("PATH", "/usr/bin:/bin");
    Ok(())
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn configure_fixed_platform_environment(
    command: &mut std::process::Command,
) -> Result<(), std::io::Error> {
    let (windows_directory, system_directory) = windows_directories_from_kernel()?;
    configure_fixed_windows_command_environment(command, &windows_directory, &system_directory)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_directories_from_kernel() -> Result<(PathBuf, PathBuf), std::io::Error> {
    use std::os::windows::ffi::OsStringExt as _;
    use windows_sys::Win32::System::SystemInformation::{
        GetSystemDirectoryW, GetWindowsDirectoryW,
    };

    let read_directory = |name: &str,
                          get_directory: unsafe extern "system" fn(*mut u16, u32) -> u32|
     -> Result<PathBuf, std::io::Error> {
        let mut buffer = vec![0u16; 32_768];
        // SAFETY: `buffer` is live, writable, and its element count is supplied
        // exactly to the selected kernel directory function.
        let length = unsafe { get_directory(buffer.as_mut_ptr(), buffer.len() as u32) };
        let length = usize::try_from(length)
            .map_err(|_| std::io::Error::other(format!("{name} length is not representable")))?;
        if length == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if length >= buffer.len() {
            return Err(std::io::Error::other(format!(
                "{name} exceeded the fixed platform buffer"
            )));
        }
        let directory = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length]));
        if !directory.is_absolute() {
            return Err(std::io::Error::other(format!(
                "{name} returned a non-absolute path"
            )));
        }
        Ok(directory)
    };

    Ok((
        read_directory("GetWindowsDirectoryW", GetWindowsDirectoryW)?,
        read_directory("GetSystemDirectoryW", GetSystemDirectoryW)?,
    ))
}

#[cfg(not(any(unix, windows)))]
fn configure_fixed_platform_environment(
    _command: &mut std::process::Command,
) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(any(windows, test))]
fn configure_fixed_windows_command_environment(
    command: &mut std::process::Command,
    windows_directory: &Path,
    system_directory: &Path,
) -> Result<(), std::io::Error> {
    if !windows_directory.is_absolute() || !system_directory.is_absolute() {
        return Err(std::io::Error::other(
            "kernel-provided Windows directories must be absolute",
        ));
    }
    let fixed_path = std::env::join_paths([system_directory, windows_directory])
        .map_err(std::io::Error::other)?;
    command
        .env_clear()
        .env("SystemRoot", windows_directory)
        .env("PATH", fixed_path);
    Ok(())
}

#[cfg(any(windows, test))]
fn windows_cleanup_command_for_directories(
    windows_directory: &Path,
    system_directory: &Path,
) -> Result<std::process::Command, std::io::Error> {
    let mut command = std::process::Command::new(system_directory.join("taskkill.exe"));
    configure_fixed_windows_command_environment(&mut command, windows_directory, system_directory)?;
    Ok(command)
}

#[cfg(windows)]
fn windows_cleanup_command() -> Result<std::process::Command, std::io::Error> {
    let (windows_directory, system_directory) = windows_directories_from_kernel()?;
    windows_cleanup_command_for_directories(&windows_directory, &system_directory)
}

/// Turn one semantic path value into a Git pathspec that cannot interpret any
/// byte of the caller's filename as magic. `--` only ends option parsing; it
/// does not disable pathspec syntax.
fn literal_pathspec(path: &str) -> String {
    format!(":(literal){path}")
}

enum ChildWaitError {
    Context(SourceError),
    Io(std::io::Error),
}

impl ChildWaitError {
    fn map_io(self, fail: &impl Fn(String) -> SourceError) -> SourceError {
        match self {
            Self::Context(error) => error,
            Self::Io(error) => fail(error.to_string()),
        }
    }
}

enum BackgroundWaitError {
    Context(SourceError),
    Io(std::io::Error),
    Disconnected,
}

impl BackgroundWaitError {
    fn map_disconnected(self, fail: &impl Fn(String) -> SourceError, detail: &str) -> SourceError {
        match self {
            Self::Context(error) => error,
            Self::Io(error) => fail(error.to_string()),
            Self::Disconnected => fail(detail.to_string()),
        }
    }
}

fn wait_child_with_context(
    child: &mut ProcessTreeGuard,
    context: &ReadContext,
) -> Result<ExitStatus, ChildWaitError> {
    loop {
        if let Err(error) = context.check() {
            return Err(ChildWaitError::Context(error));
        }
        match child.poll_parent_exit() {
            Ok(true) => return child.finish().map_err(ChildWaitError::Io),
            Ok(false) => std::thread::sleep(Duration::from_millis(2)),
            Err(error) => return Err(ChildWaitError::Io(error)),
        }
    }
}

fn receive_with_context<T>(
    receiver: &Receiver<T>,
    child: &mut ProcessTreeGuard,
    context: &ReadContext,
) -> Result<T, BackgroundWaitError> {
    loop {
        if let Err(error) = context.check() {
            let _ = child.terminate();
            return Err(BackgroundWaitError::Context(error));
        }
        match child.poll_parent_exit() {
            Ok(true) => child.terminate().map_err(BackgroundWaitError::Io)?,
            Ok(false) => {}
            Err(error) => return Err(BackgroundWaitError::Io(error)),
        }
        match receiver.recv_timeout(Duration::from_millis(2)) {
            Ok(value) => return Ok(value),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                let _ = child.terminate();
                return Err(BackgroundWaitError::Disconnected);
            }
        }
    }
}

fn spawn_pipe_reader(
    pipe: impl Read + Send + 'static,
    maximum: usize,
    allowance: PipeAllowance,
) -> Receiver<Result<BoundedPipe, std::io::Error>> {
    let (sender, receiver) = sync_channel(1);
    std::thread::spawn(move || {
        let _ = sender.send(read_pipe_bounded(pipe, maximum, &allowance));
    });
    receiver
}

fn spawn_hashed_pipe_reader(
    pipe: impl Read + Send + 'static,
    maximum: usize,
    allowance: PipeAllowance,
) -> Receiver<Result<HashedPipe, std::io::Error>> {
    let (sender, receiver) = sync_channel(1);
    std::thread::spawn(move || {
        let _ = sender.send(read_pipe_hashed(pipe, maximum, &allowance));
    });
    receiver
}

fn read_pipe_hashed(
    mut pipe: impl Read,
    maximum: usize,
    allowance: &PipeAllowance,
) -> Result<HashedPipe, std::io::Error> {
    let mut hasher = blake3::Hasher::new();
    let mut used = 0usize;
    let mut byte = [0_u8; 1];
    loop {
        if used == maximum {
            return Ok(HashedPipe {
                digest: hasher.finalize(),
                exceeded: true,
                budget_exhausted: false,
            });
        }
        let read = pipe.read(&mut byte)?;
        if read == 0 {
            return Ok(HashedPipe {
                digest: hasher.finalize(),
                exceeded: false,
                budget_exhausted: false,
            });
        }
        if !allowance.take_payload() {
            return Ok(HashedPipe {
                digest: hasher.finalize(),
                exceeded: true,
                budget_exhausted: true,
            });
        }
        hasher.update(&byte);
        used = used.saturating_add(1);
    }
}

fn read_pipe_bounded(
    mut pipe: impl Read,
    maximum: usize,
    allowance: &PipeAllowance,
) -> Result<BoundedPipe, std::io::Error> {
    let mut kept = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        if kept.len() == maximum {
            return Ok(BoundedPipe {
                bytes: kept,
                exceeded: true,
                budget_exhausted: false,
            });
        }
        let read = pipe.read(&mut byte)?;
        if read == 0 {
            return Ok(BoundedPipe {
                bytes: kept,
                exceeded: false,
                budget_exhausted: false,
            });
        }
        if !allowance.take_payload() {
            return Ok(BoundedPipe {
                bytes: kept,
                exceeded: true,
                budget_exhausted: true,
            });
        }
        kept.push(byte[0]);
    }
}

struct BoundedPipe {
    bytes: Vec<u8>,
    exceeded: bool,
    budget_exhausted: bool,
}

struct HashedPipe {
    digest: blake3::Hash,
    exceeded: bool,
    budget_exhausted: bool,
}

enum GitRecordError {
    Read(String),
    Limit,
    Budget,
}

fn read_nul_record_bounded(
    reader: &mut impl Read,
    maximum: usize,
    allowance: &PipeAllowance,
) -> Result<Option<Vec<u8>>, GitRecordError> {
    let mut record = Vec::new();
    loop {
        if record.len() == maximum {
            return Err(GitRecordError::Limit);
        }
        let mut byte = [0_u8; 1];
        let read = match reader.read(&mut byte) {
            Ok(read) => read,
            Err(error) => return Err(GitRecordError::Read(error.to_string())),
        };
        if read == 0 {
            return if record.is_empty() {
                Ok(None)
            } else {
                Err(GitRecordError::Read(
                    "git path listing ended without a NUL delimiter".to_string(),
                ))
            };
        };
        if !allowance.take_payload() {
            return Err(GitRecordError::Budget);
        }
        record.push(byte[0]);
        if byte[0] == 0 {
            return Ok(Some(record));
        }
    }
}

struct GitCommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[cfg(unix)]
fn configure_process_group(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
}

#[cfg(windows)]
fn configure_process_group(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP);
}

#[cfg(not(any(unix, windows)))]
fn configure_process_group(_command: &mut std::process::Command) {}

#[cfg(any(windows, test))]
trait CleanupProcess {
    fn try_wait(&mut self) -> Result<bool, std::io::Error>;
    fn kill(&mut self) -> Result<(), std::io::Error>;
}

#[cfg(any(windows, test))]
impl CleanupProcess for std::process::Child {
    fn try_wait(&mut self) -> Result<bool, std::io::Error> {
        std::process::Child::try_wait(self).map(|status| status.is_some())
    }

    fn kill(&mut self) -> Result<(), std::io::Error> {
        std::process::Child::kill(self)
    }
}

#[cfg(any(windows, test))]
trait CleanupCommand {
    type Process: CleanupProcess;

    fn spawn_cleanup(&mut self) -> Result<Self::Process, std::io::Error>;
}

#[cfg(any(windows, test))]
impl CleanupCommand for std::process::Command {
    type Process = std::process::Child;

    fn spawn_cleanup(&mut self) -> Result<Self::Process, std::io::Error> {
        self.spawn()
    }
}

trait CleanupClock {
    fn now(&mut self) -> Instant;
    fn sleep(&mut self, duration: Duration);
}

struct SystemCleanupClock;

impl CleanupClock for SystemCleanupClock {
    fn now(&mut self) -> Instant {
        Instant::now()
    }

    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

#[cfg(any(windows, test))]
fn poll_cleanup_process_bounded(
    process: &mut impl CleanupProcess,
    deadline: Instant,
    clock: &mut impl CleanupClock,
) -> bool {
    while clock.now() < deadline {
        match process.try_wait() {
            Ok(true) => return true,
            Ok(false) => clock.sleep(Duration::from_millis(2)),
            Err(_) => return false,
        }
    }
    false
}

#[cfg(any(windows, test))]
fn kill_and_reap_bounded(
    process: &mut impl CleanupProcess,
    deadline: Instant,
    clock: &mut impl CleanupClock,
) {
    let _ = process.kill();
    let _ = poll_cleanup_process_bounded(process, deadline, clock);
}

#[cfg(any(windows, test))]
fn run_cleanup_helper_bounded(
    command: &mut impl CleanupCommand,
    deadline: Instant,
    clock: &mut impl CleanupClock,
) {
    let Ok(mut helper) = command.spawn_cleanup() else {
        return;
    };
    if poll_cleanup_process_bounded(&mut helper, deadline, clock) {
        return;
    }
    let _ = helper.kill();
    let _ = helper.try_wait();
}

#[cfg(windows)]
struct WindowsJob {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl WindowsJob {
    #[allow(unsafe_code)]
    fn assign(child: &std::process::Child) -> std::io::Result<Self> {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        // SAFETY: null security/name pointers request an unnamed job with
        // default security. The returned owned handle is closed by Drop.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let job = Self { handle };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let limits_size = u32::try_from(std::mem::size_of_val(&limits))
            .map_err(|_| std::io::Error::other("job limit structure size exceeds u32"))?;
        // SAFETY: `limits` has the exact information-class layout and remains
        // alive for the duration of the call.
        let configured = unsafe {
            SetInformationJobObject(
                job.handle,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                limits_size,
            )
        };
        if configured == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: std::process::Child owns a live process handle; assignment
        // does not transfer ownership of either handle.
        let assigned =
            unsafe { AssignProcessToJobObject(job.handle, child.as_raw_handle().cast()) };
        if assigned == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(job)
    }

    #[allow(unsafe_code)]
    fn terminate(&self) -> std::io::Result<()> {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        // SAFETY: `handle` remains owned and valid until this object is dropped.
        if unsafe { TerminateJobObject(self.handle, 1) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: the handle was returned by CreateJobObjectW and is closed
        // exactly once here. KILL_ON_JOB_CLOSE is the final fail-closed fence.
        let _ = unsafe { windows_sys::Win32::Foundation::CloseHandle(self.handle) };
    }
}

struct ProcessTreeGuard {
    child: std::process::Child,
    parent_exited: bool,
    cached_status: Option<ExitStatus>,
    armed: bool,
    #[cfg(windows)]
    job: WindowsJob,
}

impl ProcessTreeGuard {
    fn new(child: std::process::Child) -> std::io::Result<Self> {
        #[cfg(windows)]
        let mut child = child;
        #[cfg(windows)]
        let job = match WindowsJob::assign(&child) {
            Ok(job) => job,
            Err(error) => {
                // Reject a command whose Job ownership was not established;
                // taskkill is a best-effort cleanup while the direct parent is
                // still available. `Command::spawn` cannot make assignment
                // atomic, so Windows runtime evidence must cover that narrow
                // pre-assignment race.
                let _ = terminate_process_tree(&mut child);
                return Err(error);
            }
        };
        Ok(Self {
            child,
            parent_exited: false,
            cached_status: None,
            armed: true,
            #[cfg(windows)]
            job,
        })
    }

    /// Observe the direct parent without releasing its PID on Unix. Keeping the
    /// exited parent waitable prevents its process-group ID from being reused
    /// before `finish` has killed any descendants that outlived it.
    fn poll_parent_exit(&mut self) -> std::io::Result<bool> {
        if self.parent_exited {
            return Ok(true);
        }

        #[cfg(all(
            unix,
            not(any(
                target_os = "cygwin",
                target_os = "horizon",
                target_os = "openbsd",
                target_os = "redox",
                target_os = "wasi"
            ))
        ))]
        {
            let raw_pid = i32::try_from(self.child.id())
                .map_err(|_| std::io::Error::other("git child PID exceeds i32"))?;
            let pid = rustix::process::Pid::from_raw(raw_pid)
                .ok_or_else(|| std::io::Error::other("git child has PID zero"))?;
            self.parent_exited = rustix::process::waitid(
                rustix::process::WaitId::Pid(pid),
                rustix::process::WaitIdOptions::EXITED
                    | rustix::process::WaitIdOptions::NOHANG
                    | rustix::process::WaitIdOptions::NOWAIT,
            )?
            .is_some();
        }

        #[cfg(any(
            not(unix),
            target_os = "cygwin",
            target_os = "horizon",
            target_os = "openbsd",
            target_os = "redox",
            target_os = "wasi"
        ))]
        {
            if let Some(status) = self.child.try_wait()? {
                self.cached_status = Some(status);
                self.parent_exited = true;
            }
        }

        Ok(self.parent_exited)
    }

    /// Terminate the still-owned process tree and disarm only after the direct
    /// child has been reaped. A cleanup timeout keeps the guard armed so Drop
    /// makes one final bounded attempt.
    fn terminate(&mut self) -> std::io::Result<()> {
        if !self.armed {
            return Ok(());
        }
        #[cfg(windows)]
        self.job.terminate()?;
        if let Some(status) = terminate_process_tree(&mut self.child)? {
            self.cached_status.get_or_insert(status);
            self.parent_exited = true;
            self.disarm();
            return Ok(());
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "git process-tree cleanup exceeded its deadline",
        ))
    }

    /// Complete a command only after cleaning its process group. This makes
    /// successful direct-parent exit insufficient by itself: inherited
    /// descendants are killed before ownership is explicitly disarmed.
    fn finish(&mut self) -> std::io::Result<ExitStatus> {
        if !self.parent_exited {
            return Err(std::io::Error::other(
                "git process-tree finish requested before parent exit",
            ));
        }
        self.terminate()?;
        self.cached_status
            .take()
            .ok_or_else(|| std::io::Error::other("git child exit status unavailable"))
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl std::ops::Deref for ProcessTreeGuard {
    type Target = std::process::Child;

    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

impl std::ops::DerefMut for ProcessTreeGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.terminate();
        }
    }
}

fn terminate_process_tree(child: &mut std::process::Child) -> std::io::Result<Option<ExitStatus>> {
    const CLEANUP_BUDGET: Duration = Duration::from_millis(100);
    let cleanup_deadline = Instant::now() + CLEANUP_BUDGET;
    let mut clock = SystemCleanupClock;

    #[cfg(unix)]
    {
        if let Ok(raw_pid) = i32::try_from(child.id()) {
            if let Some(process_group) = rustix::process::Pid::from_raw(raw_pid) {
                let _ = rustix::process::kill_process_group(
                    process_group,
                    rustix::process::Signal::KILL,
                );
            }
        }
    }
    #[cfg(windows)]
    {
        if let Ok(mut helper) = windows_cleanup_command() {
            helper
                .args(["/PID", &child.id().to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            run_cleanup_helper_bounded(&mut helper, cleanup_deadline, &mut clock);
        }
    }
    let _ = child.kill();
    while clock.now() < cleanup_deadline {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(Some(status)),
            Ok(None) => clock.sleep(Duration::from_millis(2)),
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(unix, windows))]
    #[test]
    fn supported_platform_rejects_bare_git_executable_before_spawn() {
        let root = tempfile::tempdir().unwrap();
        let source = GitSource::open_with_git_binary(root.path(), "git");

        assert!(matches!(
            source.command(&["--version"]),
            Err(SourceError::Containment { detail, .. })
                if detail.contains("must be absolute")
        ));
    }

    #[test]
    fn windows_cleanup_command_uses_absolute_kernel_system_directory_path() {
        let root = tempfile::tempdir().unwrap();
        let windows_directory = root.path().join("Windows");
        let system_directory = windows_directory.join("System32");
        let command =
            windows_cleanup_command_for_directories(&windows_directory, &system_directory).unwrap();
        let program = Path::new(command.get_program());

        assert!(program.is_absolute());
        assert_eq!(program, system_directory.join("taskkill.exe"));
    }

    #[cfg(windows)]
    #[test]
    fn production_windows_cleanup_command_uses_kernel_program_and_environment() {
        let (windows_directory, system_directory) = windows_directories_from_kernel().unwrap();
        let command = windows_cleanup_command().unwrap();
        let expected_path =
            std::env::join_paths([system_directory.clone(), windows_directory.clone()]).unwrap();

        assert_eq!(
            Path::new(command.get_program()),
            system_directory.join("taskkill.exe")
        );
        assert_eq!(
            command
                .get_envs()
                .find(|(name, _)| *name == std::ffi::OsStr::new("SystemRoot"))
                .and_then(|(_, value)| value),
            Some(windows_directory.as_os_str())
        );
        assert_eq!(
            command
                .get_envs()
                .find(|(name, _)| *name == std::ffi::OsStr::new("PATH"))
                .and_then(|(_, value)| value),
            Some(expected_path.as_os_str())
        );
    }

    #[derive(Default)]
    struct FakeCleanupState {
        polls: usize,
        kills: usize,
    }

    struct FakeCleanupProcess {
        state: Arc<std::sync::Mutex<FakeCleanupState>>,
        exits_after_kill: bool,
    }

    impl CleanupProcess for FakeCleanupProcess {
        fn try_wait(&mut self) -> Result<bool, std::io::Error> {
            let mut state = self.state.lock().unwrap();
            state.polls += 1;
            assert!(state.polls <= 4, "cleanup polling was not bounded");
            Ok(self.exits_after_kill && state.kills > 0)
        }

        fn kill(&mut self) -> Result<(), std::io::Error> {
            self.state.lock().unwrap().kills += 1;
            Ok(())
        }
    }

    struct FakeCleanupCommand {
        state: Arc<std::sync::Mutex<FakeCleanupState>>,
    }

    impl CleanupCommand for FakeCleanupCommand {
        type Process = FakeCleanupProcess;

        fn spawn_cleanup(&mut self) -> Result<Self::Process, std::io::Error> {
            Ok(FakeCleanupProcess {
                state: self.state.clone(),
                exits_after_kill: true,
            })
        }
    }

    struct FakeCleanupClock {
        now: Instant,
        sleeps: usize,
    }

    impl CleanupClock for FakeCleanupClock {
        fn now(&mut self) -> Instant {
            self.now
        }

        fn sleep(&mut self, duration: Duration) {
            self.sleeps += 1;
            self.now += duration;
        }
    }

    #[test]
    fn cleanup_helper_polling_is_bounded_then_kills_and_reaps() {
        let start = Instant::now();
        let state = Arc::new(std::sync::Mutex::new(FakeCleanupState::default()));
        let mut command = FakeCleanupCommand {
            state: state.clone(),
        };
        let mut clock = FakeCleanupClock {
            now: start,
            sleeps: 0,
        };

        run_cleanup_helper_bounded(&mut command, start + Duration::from_millis(5), &mut clock);

        let state = state.lock().unwrap();
        assert_eq!(clock.sleeps, 3);
        assert_eq!(state.kills, 1);
        assert_eq!(state.polls, 4, "the final poll reaps after the kill");
    }

    #[test]
    fn child_fallback_kills_then_reaps_within_the_same_bound() {
        let start = Instant::now();
        let state = Arc::new(std::sync::Mutex::new(FakeCleanupState::default()));
        let mut child = FakeCleanupProcess {
            state: state.clone(),
            exits_after_kill: true,
        };
        let mut clock = FakeCleanupClock {
            now: start,
            sleeps: 0,
        };

        kill_and_reap_bounded(&mut child, start + Duration::from_millis(5), &mut clock);

        let state = state.lock().unwrap();
        assert_eq!(state.kills, 1);
        assert_eq!(state.polls, 1);
        assert_eq!(clock.sleeps, 0);
    }

    #[cfg(unix)]
    #[test]
    fn fixed_windows_command_environment_clears_existing_entries() {
        const CHILD_MARKER: &str = "CATDB_WINDOWS_FIXED_ENV_CHILD";
        const SECRET: &str = "CATDB_WINDOWS_FIXED_ENV_SECRET";
        const EXPECTED_WINDOWS: &str = "CATDB_WINDOWS_FIXED_ENV_ROOT";
        const EXPECTED_SYSTEM: &str = "CATDB_WINDOWS_FIXED_ENV_SYSTEM";

        if std::env::var_os(CHILD_MARKER).is_some() {
            let expected_windows = PathBuf::from(std::env::var_os(EXPECTED_WINDOWS).unwrap());
            let expected_system = PathBuf::from(std::env::var_os(EXPECTED_SYSTEM).unwrap());
            let expected_path =
                std::env::join_paths([expected_system, expected_windows.clone()]).unwrap();

            assert_eq!(
                std::env::var_os("SystemRoot"),
                Some(expected_windows.into())
            );
            assert_eq!(std::env::var_os("PATH"), Some(expected_path));
            assert!(std::env::var_os(SECRET).is_none());
            return;
        }

        let root = tempfile::tempdir().unwrap();
        let windows_directory = root.path().join("Windows");
        let system_directory = windows_directory.join("System32");
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.env(SECRET, "must-not-reach-helper");
        configure_fixed_windows_command_environment(
            &mut command,
            &windows_directory,
            &system_directory,
        )
        .unwrap();
        let status = command
            .args([
                "--exact",
                "git::tests::fixed_windows_command_environment_clears_existing_entries",
                "--nocapture",
            ])
            .env(CHILD_MARKER, "1")
            .env(EXPECTED_WINDOWS, &windows_directory)
            .env(EXPECTED_SYSTEM, &system_directory)
            .status()
            .unwrap();

        assert!(status.success());
    }

    #[cfg(unix)]
    fn process_tree_fixture(
        root: &Path,
        parent_waits: bool,
    ) -> (ProcessTreeGuard, rustix::process::Pid) {
        let pid_file = root.join(if parent_waits {
            "waiting-descendant.pid"
        } else {
            "orphan-descendant.pid"
        });
        let script = if parent_waits {
            r#"sleep 60 & printf '%s' "$!" > "$1"; wait"#
        } else {
            r#"sleep 60 & printf '%s' "$!" > "$1""#
        };
        let mut command = std::process::Command::new("sh");
        command
            .args(["-c", script, "sh"])
            .arg(&pid_file)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_process_group(&mut command);
        let child = ProcessTreeGuard::new(command.spawn().expect("spawn process tree"))
            .expect("own process tree");
        let deadline = Instant::now() + Duration::from_secs(2);
        let descendant = loop {
            if let Ok(raw) = std::fs::read_to_string(&pid_file) {
                if let Ok(raw) = raw.parse::<i32>() {
                    if let Some(pid) = rustix::process::Pid::from_raw(raw) {
                        break pid;
                    }
                }
            }
            assert!(
                Instant::now() < deadline,
                "descendant did not reach the pid-file barrier"
            );
            std::thread::yield_now();
        };
        (child, descendant)
    }

    #[cfg(unix)]
    fn assert_process_terminated(pid: rustix::process::Pid) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while rustix::process::test_kill_process(pid).is_ok() {
            assert!(
                Instant::now() < deadline,
                "descendant {pid:?} survived process-tree cleanup"
            );
            std::thread::yield_now();
        }
    }

    #[cfg(unix)]
    struct TestDescendant(rustix::process::Pid);

    #[cfg(unix)]
    impl Drop for TestDescendant {
        fn drop(&mut self) {
            let _ = rustix::process::kill_process(self.0, rustix::process::Signal::KILL);
        }
    }

    #[cfg(unix)]
    fn write_fake_git(root: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;

        let fake_git = root.join(name);
        std::fs::write(&fake_git, body).expect("write fake git");
        let mut permissions = std::fs::metadata(&fake_git)
            .expect("fake git metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_git, permissions).expect("make fake git executable");
        fake_git
    }

    #[cfg(unix)]
    fn descendant_from_pid_file(path: &Path) -> TestDescendant {
        TestDescendant(
            std::fs::read_to_string(path)
                .expect("parent reached pid-file barrier")
                .parse::<i32>()
                .ok()
                .and_then(rustix::process::Pid::from_raw)
                .expect("valid descendant pid"),
        )
    }

    #[cfg(unix)]
    #[test]
    fn captured_runner_preserves_success_and_output_when_descendant_inherits_either_pipe() {
        let root = tempfile::tempdir().expect("temporary directory");
        let fake_git = write_fake_git(
            root.path(),
            "fake-git-inherited-pipe",
            r#"#!/bin/sh
case "$3" in
  inherited-stdout)
    sleep 60 >&1 2>/dev/null &
    ;;
  inherited-stderr)
    sleep 60 >/dev/null &
    ;;
  *)
    exit 3
    ;;
esac
printf '%s' "$!" > "$4"
printf 'parent-stdout'
printf 'parent-stderr' >&2
exit 0
"#,
        );
        let source = GitSource::open_with_git_binary(root.path(), &fake_git);

        for mode in ["inherited-stdout", "inherited-stderr"] {
            let pid_file = root.path().join(format!("{mode}.pid"));
            let context = ReadContext::new(
                uuid::Uuid::new_v4(),
                Instant::now() + Duration::from_secs(2),
                1,
                64,
                1,
                crate::CancellationToken::new(),
            )
            .expect("read context");
            let mut budget = GitBudget::new(&context);

            let output = source
                .run_captured(
                    &[mode, pid_file.to_str().expect("UTF-8 temporary path")],
                    &context,
                    &mut budget,
                )
                .unwrap_or_else(|error| panic!("{mode} did not preserve parent success: {error}"));
            let descendant = descendant_from_pid_file(&pid_file);

            assert!(output.status.success(), "{mode}");
            assert_eq!(output.stdout, b"parent-stdout", "{mode}");
            assert_eq!(output.stderr, b"parent-stderr", "{mode}");
            assert_process_terminated(descendant.0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn hashed_runner_preserves_output_when_descendant_inherits_either_pipe() {
        let root = tempfile::tempdir().expect("temporary directory");
        let fake_git = write_fake_git(
            root.path(),
            "fake-git-hashed-inherited-pipe",
            r#"#!/bin/sh
case "$3" in
  inherited-stdout)
    sleep 60 >&1 2>/dev/null &
    ;;
  inherited-stderr)
    sleep 60 >/dev/null &
    ;;
  *)
    exit 3
    ;;
esac
printf '%s' "$!" > "$4"
printf 'hashed-parent'
exit 0
"#,
        );
        let source = GitSource::open_with_git_binary(root.path(), &fake_git);

        for mode in ["inherited-stdout", "inherited-stderr"] {
            let pid_file = root.path().join(format!("hashed-{mode}.pid"));
            let context = ReadContext::new(
                uuid::Uuid::new_v4(),
                Instant::now() + Duration::from_secs(2),
                1,
                64,
                1,
                crate::CancellationToken::new(),
            )
            .expect("read context");
            let mut budget = GitBudget::new(&context);

            let digest = source
                .run_hashed_stdout(
                    &[mode, pid_file.to_str().expect("UTF-8 temporary path")],
                    &context,
                    &mut budget,
                    64,
                )
                .unwrap_or_else(|error| panic!("{mode} did not preserve hashed output: {error}"));
            let descendant = descendant_from_pid_file(&pid_file);

            assert_eq!(digest, blake3::hash(b"hashed-parent"), "{mode}");
            assert_process_terminated(descendant.0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn list_page_preserves_success_when_descendant_inherits_stdout() {
        let root = tempfile::tempdir().expect("temporary directory");
        let fake_git = write_fake_git(
            root.path(),
            "fake-git-list-inherited-stdout",
            r#"#!/bin/sh
if [ "$3" = "ls-files" ]; then
  sleep 60 2>/dev/null &
  printf '%s' "$!" > "$0.pid"
  printf 'a\000'
  exit 0
fi
exit 3
"#,
        );
        let pid_file = PathBuf::from(format!("{}.pid", fake_git.display()));
        let source = GitSource::open_with_git_binary(root.path(), &fake_git);
        let context = ReadContext::new(
            uuid::Uuid::new_v4(),
            Instant::now() + Duration::from_secs(2),
            10,
            64,
            10,
            crate::CancellationToken::new(),
        )
        .expect("read context");
        let mut budget = GitBudget::new(&context);

        let listing = source
            .list_page("", false, "", &context, &mut budget)
            .expect("successful ls-files parent must not become a deadline");
        let descendant = descendant_from_pid_file(&pid_file);

        assert_eq!(listing.listed, ["a"]);
        assert_eq!(listing.scanned, 1);
        assert!(!listing.has_more);
        assert_process_terminated(descendant.0);
    }

    #[cfg(unix)]
    #[test]
    fn captured_runner_cleans_up_descendant_after_successful_parent_exit() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().expect("temporary directory");
        let descendant_pid = root.path().join("runner-descendant.pid");
        let fake_git = root.path().join("fake-git-orphan");
        std::fs::write(
            &fake_git,
            r#"#!/bin/sh
if [ "$3" = "spawn-orphan" ]; then
  sleep 60 </dev/null >/dev/null 2>/dev/null &
  printf '%s' "$!" > "$4"
  exit 0
fi
exit 3
"#,
        )
        .expect("write fake git");
        let mut permissions = std::fs::metadata(&fake_git)
            .expect("fake git metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_git, permissions).expect("make fake git executable");
        let source = GitSource::open_with_git_binary(root.path(), &fake_git);
        let context = ReadContext::new(
            uuid::Uuid::new_v4(),
            Instant::now() + Duration::from_secs(2),
            1,
            32,
            1,
            crate::CancellationToken::new(),
        )
        .expect("read context");
        let mut budget = GitBudget::new(&context);

        let output = source
            .run_captured(
                &[
                    "spawn-orphan",
                    descendant_pid.to_str().expect("UTF-8 temporary path"),
                ],
                &context,
                &mut budget,
            )
            .expect("successful fake git command");
        assert!(output.status.success());
        let descendant = TestDescendant(
            std::fs::read_to_string(&descendant_pid)
                .expect("parent reached pid-file barrier")
                .parse::<i32>()
                .ok()
                .and_then(rustix::process::Pid::from_raw)
                .expect("valid descendant pid"),
        );

        assert_process_terminated(descendant.0);
    }

    #[cfg(unix)]
    #[test]
    fn reader_error_drops_the_guard_and_terminates_the_descendant() {
        fn surface_reader_error(child: ProcessTreeGuard) -> std::io::Result<()> {
            let (sender, receiver) = sync_channel::<Result<BoundedPipe, std::io::Error>>(1);
            sender
                .send(Err(std::io::Error::other("injected pipe read failure")))
                .expect("send injected reader failure");
            let _guard = child;
            receiver.recv().expect("reader result")?;
            Ok(())
        }

        let root = tempfile::tempdir().expect("temporary directory");
        let (child, descendant) = process_tree_fixture(root.path(), true);
        let error = surface_reader_error(child).expect_err("reader error reaches caller");

        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        assert_process_terminated(descendant);
    }

    #[cfg(unix)]
    #[test]
    fn try_wait_error_drops_the_guard_and_terminates_the_orphaned_descendant() {
        let root = tempfile::tempdir().expect("temporary directory");
        let (mut child, descendant) = process_tree_fixture(root.path(), false);
        let parent_pid =
            rustix::process::Pid::from_raw(i32::try_from(child.id()).expect("parent pid"))
                .expect("non-zero parent pid");
        rustix::process::waitpid(Some(parent_pid), rustix::process::WaitOptions::empty())
            .expect("externally reap parent")
            .expect("parent exited");

        child
            .try_wait()
            .expect_err("externally reaped child makes try_wait fail");
        drop(child);

        assert_process_terminated(descendant);
    }

    #[cfg(unix)]
    #[test]
    fn aggregate_raw_output_with_only_one_two_pipe_proof_byte_fails_closed() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let fake_git = root.path().join("fake-git-two-pipe-budget");
        std::fs::write(
            &fake_git,
            r#"#!/bin/sh
printf e >&2
exec 2>&-
sleep 0.02
printf o
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_git).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_git, permissions).unwrap();
        let source = GitSource::open_with_git_binary(root.path(), &fake_git);
        let context = ReadContext::new(
            uuid::Uuid::new_v4(),
            Instant::now() + Duration::from_secs(2),
            1,
            3,
            1,
            crate::CancellationToken::new(),
        )
        .unwrap();
        let mut budget = GitBudget::new(&context);

        assert!(matches!(
            source.run_captured(&["two-pipe"], &context, &mut budget),
            Err(SourceError::ByteBudgetExceeded)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn aggregate_payload_plus_two_proofs_is_stable_for_every_pipe_shape_and_schedule() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let fake_git = root.path().join("fake-git-two-pipe-shapes");
        std::fs::write(
            &fake_git,
            r#"#!/bin/sh
case "$3" in
  stdout-only)
    printf o
    ;;
  stderr-only)
    printf e >&2
    ;;
  split-stderr-first)
    printf e >&2
    exec 2>&-
    sleep 0.005
    printf o
    ;;
  split-stdout-first)
    printf o
    exec 1>&-
    sleep 0.005
    printf e >&2
    ;;
  zero)
    ;;
  *)
    exit 3
    ;;
esac
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_git).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_git, permissions).unwrap();
        let source = GitSource::open_with_git_binary(root.path(), &fake_git);
        let cases: [(&str, &[u8], &[u8], usize); 5] = [
            ("stdout-only", b"o", b"", 1),
            ("stderr-only", b"", b"e", 1),
            ("split-stderr-first", b"o", b"e", 2),
            ("split-stdout-first", b"o", b"e", 2),
            ("zero", b"", b"", 0),
        ];

        for iteration in 0..12 {
            for (mode, expected_stdout, expected_stderr, raw_bytes) in cases {
                let exact = raw_bytes + 2;
                let context = ReadContext::new(
                    uuid::Uuid::new_v4(),
                    Instant::now() + Duration::from_secs(2),
                    1,
                    exact,
                    1,
                    crate::CancellationToken::new(),
                )
                .unwrap();
                let mut budget = GitBudget::new(&context);
                let output = source
                    .run_captured(&[mode], &context, &mut budget)
                    .unwrap_or_else(|error| {
                        panic!("iteration {iteration} mode {mode} exact budget failed: {error}")
                    });
                assert_eq!(output.stdout, expected_stdout);
                assert_eq!(output.stderr, expected_stderr);
                assert_eq!(
                    budget.remaining_bytes, 2,
                    "proof tokens are capacity, not reported payload"
                );

                let one_short_context = ReadContext::new(
                    uuid::Uuid::new_v4(),
                    Instant::now() + Duration::from_secs(2),
                    1,
                    exact - 1,
                    1,
                    crate::CancellationToken::new(),
                )
                .unwrap();
                let mut one_short_budget = GitBudget::new(&one_short_context);
                assert!(
                    matches!(
                        source.run_captured(&[mode], &one_short_context, &mut one_short_budget),
                        Err(SourceError::ByteBudgetExceeded)
                    ),
                    "iteration {iteration} mode {mode} accepted aggregate raw + one proof byte"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn ls_files_stdout_and_stderr_use_the_same_two_proof_aggregate_allowance() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let fake_git = root.path().join("fake-git-listing-two-pipe-budget");
        std::fs::write(
            &fake_git,
            r#"#!/bin/sh
if [ "$3" = "ls-files" ]; then
  printf e >&2
  exec 2>&-
  sleep 0.02
  printf 'a\000'
  exit 0
fi
exit 3
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_git).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_git, permissions).unwrap();
        let source = GitSource::open_with_git_binary(root.path(), &fake_git);
        let exact_context = ReadContext::new(
            uuid::Uuid::new_v4(),
            Instant::now() + Duration::from_secs(2),
            10,
            5,
            10,
            crate::CancellationToken::new(),
        )
        .unwrap();
        let mut exact_budget = GitBudget::new(&exact_context);
        let listing = source
            .list_page("", false, "", &exact_context, &mut exact_budget)
            .expect("three raw bytes plus two pipe proofs fit");
        assert_eq!(listing.listed, ["a"]);
        assert_eq!(listing.scanned, 1);
        assert!(!listing.has_more);
        assert_eq!(exact_budget.remaining_bytes, 2);

        let one_short_context = ReadContext::new(
            uuid::Uuid::new_v4(),
            Instant::now() + Duration::from_secs(2),
            10,
            4,
            10,
            crate::CancellationToken::new(),
        )
        .unwrap();
        let mut one_short_budget = GitBudget::new(&one_short_context);
        assert!(matches!(
            source.list_page("", false, "", &one_short_context, &mut one_short_budget),
            Err(SourceError::ByteBudgetExceeded)
        ));
    }

    #[test]
    fn concurrent_readers_never_physically_read_beyond_their_aggregate_quantum() {
        struct CountingReader {
            inner: std::io::Cursor<&'static [u8]>,
            physical_bytes: Arc<AtomicUsize>,
        }

        impl Read for CountingReader {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let read = self.inner.read(buffer)?;
                self.physical_bytes.fetch_add(read, AtomicOrdering::SeqCst);
                Ok(read)
            }
        }

        for _ in 0..64 {
            let physical_bytes = Arc::new(AtomicUsize::new(0));
            let (aggregate, first, second) =
                AggregatePipeAllowance::pair(4).expect("two proof bytes fit");
            let first_bytes = physical_bytes.clone();
            let second_bytes = physical_bytes.clone();
            let first = std::thread::spawn(move || {
                read_pipe_bounded(
                    CountingReader {
                        inner: std::io::Cursor::new(b"abc"),
                        physical_bytes: first_bytes,
                    },
                    16,
                    &first,
                )
                .unwrap()
            });
            let second = std::thread::spawn(move || {
                read_pipe_bounded(
                    CountingReader {
                        inner: std::io::Cursor::new(b"abc"),
                        physical_bytes: second_bytes,
                    },
                    16,
                    &second,
                )
                .unwrap()
            });
            let first = first.join().unwrap();
            let second = second.join().unwrap();

            assert!(first.budget_exhausted || second.budget_exhausted);
            assert_eq!(aggregate.used(), 2);
            assert!(
                physical_bytes.load(AtomicOrdering::SeqCst) <= 4,
                "two payload tokens plus two proof-covered reads is the physical ceiling"
            );
        }
    }

    #[test]
    fn per_pipe_absolute_cap_stops_before_an_extra_physical_read() {
        let (aggregate, reader, _sibling) =
            AggregatePipeAllowance::pair(100).expect("two proof bytes fit");
        let result = read_pipe_bounded(std::io::Cursor::new(b"abc"), 2, &reader)
            .expect("cursor reads cannot fail");

        assert_eq!(result.bytes, b"ab");
        assert!(result.exceeded);
        assert!(!result.budget_exhausted);
        assert_eq!(aggregate.used(), 2);
    }

    #[test]
    fn pipe_pair_reserves_one_eof_proof_per_reader_before_payload() {
        let (one_short, one_short_reader, _one_short_sibling) =
            AggregatePipeAllowance::pair(4).expect("two proof bytes fit");
        let one_short_result =
            read_pipe_bounded(std::io::Cursor::new(b"abc"), 16, &one_short_reader)
                .expect("cursor reads cannot fail");
        assert_eq!(one_short_result.bytes, b"ab");
        assert!(one_short_result.exceeded);
        assert!(one_short_result.budget_exhausted);
        assert_eq!(one_short.used(), 2);

        let (exact, exact_reader, _exact_sibling) =
            AggregatePipeAllowance::pair(5).expect("payload plus two proof bytes fit");
        let exact_result = read_pipe_bounded(std::io::Cursor::new(b"abc"), 16, &exact_reader)
            .expect("cursor reads cannot fail");
        assert_eq!(exact_result.bytes, b"abc");
        assert!(!exact_result.exceeded);
        assert!(!exact_result.budget_exhausted);
        assert_eq!(exact.used(), 3);
    }
}
