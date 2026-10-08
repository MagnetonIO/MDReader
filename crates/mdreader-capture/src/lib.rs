//! Durable visible-output capture, independent of models, transcripts and agent control.
//!
//! Hooks enqueue only. The one reader service projects Markdown, commits its durable
//! capture/job state, then acknowledges. A projected body remains authoritative after ack.
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub mod backfill;
mod platform;
#[cfg(windows)]
pub use platform::windows::{same_open_file, PrivateDirectory};
mod policy;
pub use policy::ProjectPolicy;
mod projection;
mod spool;
#[cfg(not(windows))]
use platform::sync_directory;
use platform::{
    ensure_regular_or_missing, private_directory, private_open, read_bounded, regular_exists,
};
#[cfg(not(windows))]
use projection::parse_projection;
pub use projection::read_projection;
pub use spool::{FrozenSpool, Spool};

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
    #[cfg(windows)]
    #[error("capture database persistence failed")]
    Database(#[from] rusqlite::Error),
    #[error("invalid capture data or policy")]
    Invalid,
    #[error("capture input exceeds configured bound")]
    TooLarge,
    #[error("capture queue is full")]
    QueueFull,
    #[error("capture spool is busy; retry later")]
    Busy,
    #[error("capture spool namespace durability is unsupported on this platform")]
    UnsupportedPlatform,
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

pub mod cli;
