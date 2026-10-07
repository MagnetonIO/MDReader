//! Durable visible-output capture, independent of SQLite, models, transcripts and agent control.
//!
//! Hooks enqueue only. The one reader service projects Markdown, commits its durable
//! capture/job state, then acknowledges. A projected body remains authoritative after ack.
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const RETRIEVAL_MARKER: &str = "<!-- mdreader-retrieval -->";
pub const CAPTURE_MARKER: &str = "<!-- mdreader-capture:v1 -->";
const HEADER: &str = "<!-- mdreader-capture:v1 -->\n---\nmdreader_capture: ";
const POLICY_BYTES: usize = 64 * 1024;
/// Must not exceed the shared reader core document admission cap, including frontmatter.
pub const MAX_MARKDOWN_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("capture persistence failed")]
    Io(#[from] std::io::Error),
    #[error("invalid capture data or policy")]
    Invalid,
    #[error("capture input exceeds configured bound")]
    TooLarge,
    #[error("capture queue is full")]
    QueueFull,
    #[error("capture spool is busy; retry later")]
    Busy,
    #[error("capture path authority changed or is unsafe")]
    UnsafePath,
    #[error("capture is excluded: {0:?}")]
    Excluded(SkipReason),
    #[error("capture projection provenance bound reached")]
    ProvenanceFull,
    #[error("capture projection must be durable before acknowledgment")]
    NotProjected,
}
pub type Result<T> = std::result::Result<T, CaptureError>;

/// Limits reject whole inputs; they never truncate authoritative bodies.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub max_event_bytes: usize,
    pub max_pending_events: usize,
    pub max_pending_bytes: u64,
    pub max_projection_bytes: usize,
    pub max_occurrences_per_body: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_event_bytes: 256 * 1024,
            max_pending_events: 128,
            max_pending_bytes: 8 * 1024 * 1024,
            max_projection_bytes: MAX_MARKDOWN_BYTES,
            max_occurrences_per_body: 1024,
        }
    }
}
impl Limits {
    fn validate(&self) -> Result<()> {
        if self.max_event_bytes == 0
            || self.max_event_bytes > 1024 * 1024
            || self.max_pending_events == 0
            || self.max_pending_events > 4096
            || self.max_pending_bytes == 0
            || self.max_pending_bytes > 1024 * 1024 * 1024
            || self.max_projection_bytes < self.max_event_bytes
            || self.max_projection_bytes > MAX_MARKDOWN_BYTES
            || self.max_occurrences_per_body == 0
            || self.max_occurrences_per_body > 16384
        {
            return Err(CaptureError::Invalid);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Adapter {
    ClaudeCode,
    CodexHook,
    CodexNotify,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureOrigin {
    Live,
    SelectedBackfill,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    ProjectDisabled,
    OutsideProject,
    PathExcluded,
    SessionExcluded,
    BackfillNotSelected,
    SubagentExcluded,
    UnsupportedEvent,
    MissingVisibleOutput,
    RetrievalEcho,
    ReasoningContent,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EnqueueOutcome {
    Queued { id: String },
    Duplicate { id: String },
    Skipped { reason: SkipReason },
}

/// Explicit policy supplied by the owner of this library, never discovered in histories.
/// Roots are exact admitted worktrees. Exclusions are relative directory prefixes in
/// every worktree; there is no implicit admission of sibling or new worktrees.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectPolicy {
    #[serde(default)]
    pub enabled: bool,
    pub project_id: String,
    pub project_root: PathBuf,
    pub repository_root: PathBuf,
    pub worktree_roots: Vec<PathBuf>,
    #[serde(default)]
    pub capture_subagents: bool,
    #[serde(default)]
    pub excluded_paths: Vec<PathBuf>,
    #[serde(default)]
    pub excluded_sessions: Vec<String>,
    #[serde(default)]
    pub selected_backfill_sessions: Vec<String>,
}
impl ProjectPolicy {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = read_bounded(path, POLICY_BYTES)?;
        let policy: Self = serde_json::from_slice(&bytes).map_err(|_| CaptureError::Invalid)?;
        policy.validate()?;
        Ok(policy)
    }
    pub fn validate(&self) -> Result<()> {
        if self.project_id.trim().is_empty()
            || self.project_id.len() > 256
            || !self.project_root.is_absolute()
            || !self.repository_root.is_absolute()
            || self.worktree_roots.is_empty()
            || self.worktree_roots.len() > 128
            || self.excluded_paths.len() > 1024
            || self.excluded_sessions.len() > 4096
            || self.selected_backfill_sessions.len() > 4096
            || self.worktree_roots.iter().any(|p| !p.is_absolute())
            || self.excluded_paths.iter().any(|p| {
                p.as_os_str().is_empty()
                    || p.components().any(|c| !matches!(c, Component::Normal(_)))
            })
        {
            return Err(CaptureError::Invalid);
        }
        Ok(())
    }
    fn authorize(
        &self,
        cwd: &Path,
        session: &str,
        subagent: Option<&str>,
        origin: CaptureOrigin,
    ) -> Result<AuthorizedRoots> {
        self.validate()?;
        if !self.enabled {
            return Err(CaptureError::Excluded(SkipReason::ProjectDisabled));
        }
        if self.excluded_sessions.iter().any(|s| s == session) {
            return Err(CaptureError::Excluded(SkipReason::SessionExcluded));
        }
        if origin == CaptureOrigin::SelectedBackfill
            && !self.selected_backfill_sessions.iter().any(|s| s == session)
        {
            return Err(CaptureError::Excluded(SkipReason::BackfillNotSelected));
        }
        if subagent.is_some() && !self.capture_subagents {
            return Err(CaptureError::Excluded(SkipReason::SubagentExcluded));
        }
        if !cwd.is_absolute() {
            return Err(CaptureError::Invalid);
        }
        let cwd = fs::canonicalize(cwd)?;
        let mut admitted = Vec::new();
        for configured in &self.worktree_roots {
            let root = fs::canonicalize(configured)?;
            // A root itself cannot be a symlink. System ancestor aliases (such as
            // macOS /var) are normalized into the captured authority.
            if fs::symlink_metadata(configured)?.file_type().is_symlink() {
                return Err(CaptureError::UnsafePath);
            }
            if cwd.starts_with(&root) {
                admitted.push(root);
            }
        }
        let worktree = admitted
            .into_iter()
            .max_by_key(|p| p.components().count())
            .ok_or(CaptureError::Excluded(SkipReason::OutsideProject))?;
        if self
            .excluded_paths
            .iter()
            .any(|p| cwd.starts_with(worktree.join(p)))
        {
            return Err(CaptureError::Excluded(SkipReason::PathExcluded));
        }
        let project = fs::canonicalize(&self.project_root)?;
        let repository = fs::canonicalize(&self.repository_root)?;
        if fs::symlink_metadata(&self.project_root)?
            .file_type()
            .is_symlink()
            || fs::symlink_metadata(&self.repository_root)?
                .file_type()
                .is_symlink()
        {
            return Err(CaptureError::UnsafePath);
        }
        Ok(AuthorizedRoots {
            cwd,
            project,
            repository,
            worktree,
        })
    }
}
struct AuthorizedRoots {
    cwd: PathBuf,
    project: PathBuf,
    repository: PathBuf,
    worktree: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub occurrence_id: String,
    pub project_id: String,
    pub project_root: PathBuf,
    pub repository_root: PathBuf,
    pub worktree_root: PathBuf,
    pub cwd: PathBuf,
    pub adapter: Adapter,
    pub lifecycle_event: String,
    pub session_id: String,
    /// Claude prompt_id when available; Codex turn_id. Missing IDs stay null.
    pub turn_id: Option<String>,
    pub turn_id_kind: Option<String>,
    pub subagent_id: Option<String>,
    pub subagent_type: Option<String>,
    /// Hooks do not provide a trustworthy Git revision. Never infer one from history.
    pub repository_revision: Option<String>,
    pub origin: CaptureOrigin,
    pub captured_unix_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureRecord {
    pub schema_version: u32,
    pub id: String,
    pub content_id: String,
    pub content_revision: String,
    pub provenance: Provenance,
    pub body: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionMetadata {
    pub schema_version: u32,
    pub content_id: String,
    pub content_revision: String,
    pub capture_is_not_approval: bool,
    pub occurrences: Vec<Provenance>,
}
#[derive(Debug)]
pub struct ProjectedCapture {
    pub id: String,
    pub path: PathBuf,
    pub record: CaptureRecord,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueStatus {
    pub pending_events: usize,
    pub pending_bytes: u64,
    pub orphan_temporary_files: usize,
}

// Unknown fields are intentionally discarded. In particular, transcript paths,
// reasoning, prompts, tool inputs/outputs and provider credentials never enter the spool.
#[derive(Deserialize)]
struct HookInput {
    hook_event_name: String,
    session_id: String,
    cwd: PathBuf,
    #[serde(default)]
    last_assistant_message: Option<String>,
    #[serde(default)]
    prompt_id: Option<String>,
    #[serde(default)]
    turn_id: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    agent_type: Option<String>,
}
#[derive(Deserialize)]
struct NotifyInput {
    #[serde(rename = "type")]
    event: String,
    #[serde(rename = "thread-id")]
    session: String,
    #[serde(rename = "turn-id", default)]
    turn: Option<String>,
    cwd: PathBuf,
    #[serde(rename = "last-assistant-message", default)]
    body: Option<String>,
}
struct VisibleOutput {
    event: String,
    session: String,
    turn: Option<String>,
    turn_kind: Option<String>,
    cwd: PathBuf,
    body: String,
    subagent: Option<String>,
    agent_type: Option<String>,
}
fn adapt(adapter: Adapter, bytes: &[u8]) -> Result<VisibleOutput> {
    let value = if adapter == Adapter::CodexNotify {
        let input: NotifyInput =
            serde_json::from_slice(bytes).map_err(|_| CaptureError::Invalid)?;
        if input.event != "agent-turn-complete" {
            return Err(CaptureError::Excluded(SkipReason::UnsupportedEvent));
        }
        VisibleOutput {
            event: input.event,
            session: input.session,
            turn_kind: input.turn.as_ref().map(|_| "turn_id".into()),
            turn: input.turn,
            cwd: input.cwd,
            body: input.body.unwrap_or_default(),
            subagent: None,
            agent_type: None,
        }
    } else {
        let input: HookInput = serde_json::from_slice(bytes).map_err(|_| CaptureError::Invalid)?;
        if !matches!(input.hook_event_name.as_str(), "Stop" | "SubagentStop") {
            return Err(CaptureError::Excluded(SkipReason::UnsupportedEvent));
        }
        if input.hook_event_name == "SubagentStop"
            && input.agent_id.as_deref().is_none_or(str::is_empty)
        {
            return Err(CaptureError::Invalid);
        }
        let (turn, kind) = if adapter == Adapter::ClaudeCode {
            (input.prompt_id, "prompt_id")
        } else {
            (input.turn_id, "turn_id")
        };
        VisibleOutput {
            event: input.hook_event_name,
            session: input.session_id,
            turn_kind: turn.as_ref().map(|_| kind.into()),
            turn,
            cwd: input.cwd,
            body: input.last_assistant_message.unwrap_or_default(),
            subagent: input.agent_id,
            agent_type: input.agent_type,
        }
    };
    if value.session.trim().is_empty()
        || value.session.len() > 512
        || value
            .turn
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 512)
        || value
            .subagent
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 512)
        || value.agent_type.as_ref().is_some_and(|s| s.len() > 512)
    {
        return Err(CaptureError::Invalid);
    }
    if value.body.trim().is_empty() {
        return Err(CaptureError::Excluded(SkipReason::MissingVisibleOutput));
    }
    if value.body.contains(RETRIEVAL_MARKER) || value.body.contains(CAPTURE_MARKER) {
        return Err(CaptureError::Excluded(SkipReason::RetrievalEcho));
    }
    if ["<think>", "<analysis>", "<reasoning>"]
        .iter()
        .any(|marker| value.body.contains(marker))
    {
        return Err(CaptureError::Excluded(SkipReason::ReasoningContent));
    }
    Ok(value)
}
/// Prefix MCP retrieval text so a later verbatim assistant echo is excluded.
pub fn mark_retrieval(text: &str) -> String {
    format!("{RETRIEVAL_MARKER}\n{text}")
}
fn hash<T: Serialize>(value: &T) -> Result<String> {
    let bytes = serde_json::to_vec(value).map_err(|_| CaptureError::Invalid)?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}
fn identity(record: &CaptureRecord) -> Result<String> {
    occurrence_identity(&record.content_id, &record.provenance)
}
fn occurrence_identity(content_id: &str, p: &Provenance) -> Result<String> {
    // Hook and notify are two transports for one Codex authorship occurrence.
    let agent = match p.adapter {
        Adapter::ClaudeCode => "claude_code",
        Adapter::CodexHook | Adapter::CodexNotify => "codex",
    };
    hash(&(
        content_id,
        agent,
        &p.session_id,
        &p.turn_id,
        &p.subagent_id,
        &p.worktree_root,
        &p.cwd,
    ))
}
fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn validate_record(record: &CaptureRecord) -> Result<()> {
    if record.schema_version != 1
        || !valid_id(&record.id)
        || !valid_id(&record.content_id)
        || record.provenance.occurrence_id != record.id
        || record.content_revision != blake3::hash(record.body.as_bytes()).to_hex().as_str()
        || record.content_id != hash(&(&record.provenance.project_id, &record.body))?
        || record.id != identity(record)?
    {
        return Err(CaptureError::Invalid);
    }
    Ok(())
}

// flock belongs to the open file description. Explicit unlock on owner drop
// prevents an unrelated fork's briefly inherited descriptor from extending the
// critical section after our operation has already returned.
struct QueueLock(File);
impl std::ops::Deref for QueueLock {
    type Target = File;
    fn deref(&self) -> &File {
        &self.0
    }
}
impl Drop for QueueLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

pub struct Spool {
    root: PathBuf,
    limits: Limits,
    #[cfg(unix)]
    identity: (u64, u64),
}
impl Spool {
    pub fn open(root: impl Into<PathBuf>, limits: Limits) -> Result<Self> {
        limits.validate()?;
        let root = root.into();
        // Unsupported hosts fail before creating a directory or touching content.
        #[cfg(not(unix))]
        sync_directory(&root)?;
        private_directory(&root)?;
        let root = fs::canonicalize(root)?;
        for name in ["pending", "markdown", "receipts", "temporary"] {
            private_directory(&root.join(name))?;
        }
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            let m = fs::metadata(&root)?;
            (m.dev(), m.ino())
        };
        let spool = Self {
            root,
            limits,
            #[cfg(unix)]
            identity,
        };
        let lock = spool.lock()?;
        drop(lock);
        Ok(spool)
    }
    pub fn markdown_root(&self) -> PathBuf {
        self.root.join("markdown")
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn limits(&self) -> &Limits {
        &self.limits
    }
    fn verify(&self) -> Result<()> {
        for path in [
            &self.root,
            &self.root.join("pending"),
            &self.root.join("markdown"),
            &self.root.join("receipts"),
            &self.root.join("temporary"),
        ] {
            let meta = fs::symlink_metadata(path)?;
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err(CaptureError::UnsafePath);
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let m = fs::metadata(&self.root)?;
            if (m.dev(), m.ino()) != self.identity {
                return Err(CaptureError::UnsafePath);
            }
        }
        Ok(())
    }
    fn lock(&self) -> Result<QueueLock> {
        self.verify()?;
        let path = self.root.join("queue.lock");
        ensure_regular_or_missing(&path)?;
        let file = private_open(&path, false)?;
        file.try_lock_exclusive().map_err(|e| {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                CaptureError::Busy
            } else {
                CaptureError::Io(e)
            }
        })?;
        self.verify()?;
        Ok(QueueLock(file))
    }
    fn pending_path(&self, id: &str) -> Result<PathBuf> {
        if !valid_id(id) {
            return Err(CaptureError::Invalid);
        }
        Ok(self.root.join("pending").join(format!("{id}.json")))
    }
    fn receipt_path(&self, id: &str) -> Result<PathBuf> {
        if !valid_id(id) {
            return Err(CaptureError::Invalid);
        }
        Ok(self.root.join("receipts").join(id))
    }
    pub fn enqueue(
        &self,
        adapter: Adapter,
        bytes: &[u8],
        policy: &ProjectPolicy,
        origin: CaptureOrigin,
    ) -> Result<EnqueueOutcome> {
        if bytes.len() > self.limits.max_event_bytes {
            return Err(CaptureError::TooLarge);
        }
        let result = self.prepare(adapter, bytes, policy, origin);
        let record = match result {
            Ok(r) => r,
            Err(CaptureError::Excluded(reason)) => return Ok(EnqueueOutcome::Skipped { reason }),
            Err(e) => return Err(e),
        };
        let bytes = serde_json::to_vec(&record).map_err(|_| CaptureError::Invalid)?;
        if bytes.len() > self.limits.max_event_bytes {
            return Err(CaptureError::TooLarge);
        }
        let _lock = self.lock()?;
        let path = self.pending_path(&record.id)?;
        let receipt = self.receipt_path(&record.id)?;
        if regular_exists(&path)? {
            self.read_pending_locked(&record.id)?;
            self.confirm_durable(&path)?;
            return Ok(EnqueueOutcome::Duplicate { id: record.id });
        }
        if regular_exists(&receipt)? {
            if read_bounded(&receipt, 64)? != record.content_id.as_bytes() {
                return Err(CaptureError::Invalid);
            }
            let projection = self
                .markdown_root()
                .join(format!("{}.md", record.content_id));
            let metadata = read_projection(&projection, &self.limits)
                .map_err(|_| CaptureError::NotProjected)?;
            if metadata.content_id != record.content_id
                || metadata.content_revision != record.content_revision
                || !metadata
                    .occurrences
                    .iter()
                    .any(|p| p.occurrence_id == record.id)
            {
                return Err(CaptureError::NotProjected);
            }
            self.confirm_durable(&projection)?;
            self.confirm_durable(&receipt)?;
            return Ok(EnqueueOutcome::Duplicate { id: record.id });
        }
        let status = self.status_locked()?;
        if status.pending_events >= self.limits.max_pending_events
            || status.pending_bytes.saturating_add(bytes.len() as u64)
                > self.limits.max_pending_bytes
        {
            return Err(CaptureError::QueueFull);
        }
        self.atomic_write(&path, &bytes)?;
        Ok(EnqueueOutcome::Queued { id: record.id })
    }
    fn prepare(
        &self,
        adapter: Adapter,
        bytes: &[u8],
        policy: &ProjectPolicy,
        origin: CaptureOrigin,
    ) -> Result<CaptureRecord> {
        let output = adapt(adapter, bytes)?;
        let roots = policy.authorize(
            &output.cwd,
            &output.session,
            output.subagent.as_deref(),
            origin,
        )?;
        let mut record = CaptureRecord {
            schema_version: 1,
            id: String::new(),
            content_id: hash(&(&policy.project_id, &output.body))?,
            content_revision: blake3::hash(output.body.as_bytes()).to_hex().to_string(),
            body: output.body,
            provenance: Provenance {
                occurrence_id: String::new(),
                project_id: policy.project_id.clone(),
                project_root: roots.project,
                repository_root: roots.repository,
                worktree_root: roots.worktree,
                cwd: roots.cwd,
                adapter,
                lifecycle_event: output.event,
                session_id: output.session,
                turn_id: output.turn,
                turn_id_kind: output.turn_kind,
                subagent_id: output.subagent,
                subagent_type: output.agent_type,
                repository_revision: None,
                origin,
                captured_unix_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| CaptureError::Invalid)?
                    .as_millis()
                    .try_into()
                    .map_err(|_| CaptureError::Invalid)?,
            },
        };
        record.id = identity(&record)?;
        record.provenance.occurrence_id = record.id.clone();
        Ok(record)
    }
    fn status_locked(&self) -> Result<QueueStatus> {
        let mut status = QueueStatus {
            pending_events: 0,
            pending_bytes: 0,
            orphan_temporary_files: 0,
        };
        for entry in
            fs::read_dir(self.root.join("pending"))?.take(self.limits.max_pending_events + 1)
        {
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if !meta.is_file() || meta.file_type().is_symlink() {
                return Err(CaptureError::UnsafePath);
            }
            status.pending_events += 1;
            status.pending_bytes = status.pending_bytes.saturating_add(meta.len());
        }
        status.orphan_temporary_files = fs::read_dir(self.root.join("temporary"))?
            .take(self.limits.max_pending_events + 1)
            .count();
        Ok(status)
    }
    pub fn status(&self) -> Result<QueueStatus> {
        let _lock = self.lock()?;
        self.status_locked()
    }
    /// Deterministic bounded batch. Invalid records remain pending for explicit recovery.
    pub fn pending(&self, limit: usize) -> Result<Vec<String>> {
        let _lock = self.lock()?;
        let mut ids = Vec::new();
        for entry in
            fs::read_dir(self.root.join("pending"))?.take(self.limits.max_pending_events + 1)
        {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_str().ok_or(CaptureError::Invalid)?;
            let id = name
                .strip_suffix(".json")
                .filter(|s| valid_id(s))
                .ok_or(CaptureError::Invalid)?;
            ensure_regular_or_missing(&entry.path())?;
            ids.push(id.into());
        }
        ids.sort();
        ids.truncate(limit.min(self.limits.max_pending_events));
        Ok(ids)
    }
    pub fn read_pending(&self, id: &str) -> Result<CaptureRecord> {
        let _lock = self.lock()?;
        self.read_pending_locked(id)
    }
    fn read_pending_locked(&self, id: &str) -> Result<CaptureRecord> {
        let bytes = read_bounded(&self.pending_path(id)?, self.limits.max_event_bytes)?;
        let record: CaptureRecord =
            serde_json::from_slice(&bytes).map_err(|_| CaptureError::Invalid)?;
        validate_record(&record)?;
        if record.id != id {
            return Err(CaptureError::Invalid);
        }
        Ok(record)
    }
    /// Service-only operation. Revalidates the current grant, then atomically creates
    /// or updates ordinary Markdown. Repeating after a crash preserves one occurrence.
    pub fn project(&self, id: &str, policy: &ProjectPolicy) -> Result<ProjectedCapture> {
        let _lock = self.lock()?;
        let record = self.read_pending_locked(id)?;
        let p = &record.provenance;
        if p.project_id != policy.project_id {
            return Err(CaptureError::UnsafePath);
        }
        let current =
            policy.authorize(&p.cwd, &p.session_id, p.subagent_id.as_deref(), p.origin)?;
        if current.project != p.project_root
            || current.repository != p.repository_root
            || current.worktree != p.worktree_root
        {
            return Err(CaptureError::UnsafePath);
        }
        let path = self
            .markdown_root()
            .join(format!("{}.md", record.content_id));
        let mut metadata = if regular_exists(&path)? {
            let (metadata, body) = parse_projection(&path, &self.limits)?;
            if metadata.content_id != record.content_id
                || metadata.content_revision != record.content_revision
                || body != record.body
            {
                return Err(CaptureError::Invalid);
            }
            metadata
        } else {
            ProjectionMetadata {
                schema_version: 1,
                content_id: record.content_id.clone(),
                content_revision: record.content_revision.clone(),
                capture_is_not_approval: true,
                occurrences: Vec::new(),
            }
        };
        if metadata
            .occurrences
            .iter()
            .any(|existing| existing.occurrence_id == id && existing != &record.provenance)
        {
            return Err(CaptureError::Invalid);
        }
        if !metadata
            .occurrences
            .iter()
            .any(|existing| existing.occurrence_id == id)
        {
            if metadata.occurrences.len() >= self.limits.max_occurrences_per_body {
                return Err(CaptureError::ProvenanceFull);
            }
            metadata.occurrences.push(record.provenance.clone());
            let json = serde_json::to_string(&metadata).map_err(|_| CaptureError::Invalid)?;
            let bytes = format!("{HEADER}{json}\n---\n\n{}", record.body).into_bytes();
            if bytes.len() > self.limits.max_projection_bytes {
                return Err(CaptureError::TooLarge);
            }
            self.atomic_write(&path, &bytes)?;
        } else {
            // A previous rename may have succeeded before directory sync failed.
            self.confirm_durable(&path)?;
        }
        Ok(ProjectedCapture {
            id: id.into(),
            path,
            record,
        })
    }
    /// Call only after the service commits capture admission and its index job in
    /// library.sqlite. No index success or model availability is required for durability.
    pub fn acknowledge(&self, id: &str) -> Result<()> {
        let _lock = self.lock()?;
        let receipt = self.receipt_path(id)?;
        let pending = self.pending_path(id)?;
        if regular_exists(&receipt)? && !regular_exists(&pending)? {
            let bytes = read_bounded(&receipt, 64)?;
            let content_id = std::str::from_utf8(&bytes).map_err(|_| CaptureError::Invalid)?;
            if !valid_id(content_id) {
                return Err(CaptureError::Invalid);
            }
            let metadata = read_projection(
                &self.markdown_root().join(format!("{content_id}.md")),
                &self.limits,
            )?;
            if !metadata.occurrences.iter().any(|p| p.occurrence_id == id) {
                return Err(CaptureError::NotProjected);
            }
            self.confirm_durable(&self.markdown_root().join(format!("{content_id}.md")))?;
            self.confirm_durable(&receipt)?;
            return Ok(());
        }
        let record = self.read_pending_locked(id)?;
        let metadata = read_projection(
            &self
                .markdown_root()
                .join(format!("{}.md", record.content_id)),
            &self.limits,
        )
        .map_err(|_| CaptureError::NotProjected)?;
        if !metadata.occurrences.iter().any(|p| p == &record.provenance) {
            return Err(CaptureError::NotProjected);
        }
        // Receipt is durable before removing the queue event. A crash here is replayable.
        if !regular_exists(&receipt)? {
            self.atomic_write(&receipt, record.content_id.as_bytes())?;
        } else if read_bounded(&receipt, 64)? != record.content_id.as_bytes() {
            return Err(CaptureError::Invalid);
        }
        self.confirm_durable(&receipt)?;
        fs::remove_file(pending)?;
        sync_directory(&self.root.join("pending"))?;
        Ok(())
    }
    /// Only incomplete temporary writes are discarded. Pending events and Markdown
    /// are never removed by recovery. This runs under the same nonblocking lock.
    pub fn recover_temporary(&self) -> Result<usize> {
        let _lock = self.lock()?;
        let mut count = 0;
        for entry in
            fs::read_dir(self.root.join("temporary"))?.take(self.limits.max_pending_events + 1)
        {
            let entry = entry?;
            ensure_regular_or_missing(&entry.path())?;
            fs::remove_file(entry.path())?;
            count += 1;
        }
        sync_directory(&self.root.join("temporary"))?;
        Ok(count)
    }
    fn confirm_durable(&self, path: &Path) -> Result<()> {
        ensure_regular_or_missing(path)?;
        File::open(path)?.sync_all()?;
        sync_directory(path.parent().ok_or(CaptureError::UnsafePath)?)
    }
    fn atomic_write(&self, destination: &Path, bytes: &[u8]) -> Result<()> {
        ensure_regular_or_missing(destination)?;
        let temporary = self.root.join("temporary").join(format!(
            "{}-{}.tmp",
            std::process::id(),
            blake3::hash(bytes).to_hex()
        ));
        let mut file = private_open(&temporary, true)?;
        let result = (|| {
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, destination)?;
            sync_directory(destination.parent().ok_or(CaptureError::UnsafePath)?)?;
            sync_directory(&self.root.join("temporary"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}
/// Metadata and exact visible Markdown are verified before acknowledgment/retrieval.
pub fn read_projection(path: &Path, limits: &Limits) -> Result<ProjectionMetadata> {
    parse_projection(path, limits).map(|(metadata, _)| metadata)
}
fn parse_projection(path: &Path, limits: &Limits) -> Result<(ProjectionMetadata, String)> {
    let bytes = read_bounded(path, limits.max_projection_bytes)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| CaptureError::Invalid)?;
    let rest = text.strip_prefix(HEADER).ok_or(CaptureError::Invalid)?;
    let (json, body) = rest.split_once("\n---\n\n").ok_or(CaptureError::Invalid)?;
    let metadata: ProjectionMetadata =
        serde_json::from_str(json).map_err(|_| CaptureError::Invalid)?;
    if metadata.schema_version != 1
        || !metadata.capture_is_not_approval
        || !valid_id(&metadata.content_id)
        || metadata.occurrences.is_empty()
        || metadata.occurrences.len() > limits.max_occurrences_per_body
        || metadata.content_revision != blake3::hash(body.as_bytes()).to_hex().as_str()
    {
        return Err(CaptureError::Invalid);
    }
    for occurrence in &metadata.occurrences {
        if !valid_id(&occurrence.occurrence_id)
            || metadata.content_id != hash(&(&occurrence.project_id, body))?
            || occurrence.occurrence_id != occurrence_identity(&metadata.content_id, occurrence)?
        {
            return Err(CaptureError::Invalid);
        }
    }
    Ok((metadata, body.into()))
}
fn private_directory(path: &Path) -> Result<()> {
    if path.exists() {
        let meta = fs::symlink_metadata(path)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(CaptureError::UnsafePath);
        }
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)?;
        }
        #[cfg(not(unix))]
        fs::create_dir_all(path)?;
        if let Some(parent) = path.parent() {
            sync_directory(parent)?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
            return Err(CaptureError::UnsafePath);
        }
    }
    Ok(())
}
fn ensure_regular_or_missing(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(CaptureError::UnsafePath),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
fn regular_exists(path: &Path) -> Result<bool> {
    ensure_regular_or_missing(path)?;
    Ok(path.exists())
}
fn private_open(path: &Path, create_new: bool) -> Result<File> {
    ensure_regular_or_missing(path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
fn read_bounded(path: &Path, max: usize) -> Result<Vec<u8>> {
    ensure_regular_or_missing(path)?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max {
        return Err(CaptureError::TooLarge);
    }
    Ok(bytes)
}
#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}
// Never acknowledge durability on an unimplemented host.
#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Err(CaptureError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "durable capture directory sync is currently Unix-only",
    )))
}

pub mod cli;

#[cfg(test)]
mod lock_lifetime_tests {
    use super::*;
    #[test]
    #[cfg(unix)]
    fn owner_drop_releases_lock_even_when_a_fork_inherits_the_description() {
        let temp = tempfile::tempdir().unwrap();
        let spool = Spool::open(temp.path().join("spool"), Limits::default()).unwrap();
        let guard = spool.lock().unwrap();
        // dup models the same open file description inherited between fork and
        // exec by an unrelated concurrently launched process.
        let inherited = guard.try_clone().unwrap();
        drop(guard);
        assert!(
            spool.pending(1).is_ok(),
            "owner drop must unlock independently of inherited handles"
        );
        drop(inherited);
    }
}
