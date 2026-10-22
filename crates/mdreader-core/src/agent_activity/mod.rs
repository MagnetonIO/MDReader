//! Agent session activity: which Markdown files each Claude Code or Codex
//! session created or edited. Only titles, working directories, branches,
//! timestamps and absolute file paths are stored, never message text. The
//! original files stay authoritative; rows can be forgotten and re-ingested.
pub mod clock;
mod exact;
mod legacy;
mod read;
pub(crate) mod roots;
mod schema;
mod state;
mod write;
pub use exact::{file_reference, read_file, ExactReference, Unavailable as FileUnavailable};
pub use read::{FileState, Page, Totals};
use serde::{Deserialize, Serialize};
pub use state::{tail_fingerprint, Cursor, Tracking, TAIL_BYTES};

/// Sessions per page are bounded so a listing stays one small response.
pub const MAX_PAGE: u32 = 100;
/// Bounds on durable growth; observations beyond them are ignored, not fatal.
const MAX_SESSIONS: i64 = 100_000;
const MAX_FILES_PER_SESSION: i64 = 2_000;
const MAX_TITLE: usize = 256;
const MAX_PATH: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    Claude,
    Codex,
}
impl Agent {
    pub const ALL: [Agent; 2] = [Agent::Claude, Agent::Codex];
    pub fn as_str(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }
    fn parse(value: &str) -> Option<Self> {
        match value {
            "claude" => Some(Agent::Claude),
            "codex" => Some(Agent::Codex),
            _ => None,
        }
    }
}

/// Where a session title came from. A higher rank is never replaced by a lower.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleSource {
    /// No title recorded; listings show `<cwd basename> · <date>`.
    Fallback,
    /// The first real user prompt, shortened.
    Prompt,
    /// The client's generated title (Claude `aiTitle`).
    Ai,
    /// A name the user gave in the client (`/rename`, Codex thread name, hooks).
    Custom,
    /// Renamed in MDReader.
    User,
}
impl TitleSource {
    fn as_str(self) -> &'static str {
        match self {
            TitleSource::Fallback => "fallback",
            TitleSource::Prompt => "prompt",
            TitleSource::Ai => "ai",
            TitleSource::Custom => "custom",
            TitleSource::User => "user",
        }
    }
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "fallback" => TitleSource::Fallback,
            "prompt" => TitleSource::Prompt,
            "ai" => TitleSource::Ai,
            "custom" => TitleSource::Custom,
            "user" => TitleSource::User,
            _ => return None,
        })
    }
    /// The SQL rank expression for a stored `title_source` column.
    const RANK_SQL: &'static str = "CASE title_source WHEN 'prompt' THEN 1 WHEN 'ai' THEN 2 WHEN 'custom' THEN 3 WHEN 'user' THEN 4 ELSE 0 END";
    fn rank(self) -> i64 {
        self as i64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOp {
    Create,
    Edit,
}
impl FileOp {
    fn as_str(self) -> &'static str {
        match self {
            FileOp::Create => "create",
            FileOp::Edit => "edit",
        }
    }
    fn parse(value: &str) -> Option<Self> {
        match value {
            "create" => Some(FileOp::Create),
            "edit" => Some(FileOp::Edit),
            _ => None,
        }
    }
}

/// One Markdown file a session created or edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileObservation {
    pub abs_path: std::path::PathBuf,
    pub op: FileOp,
    pub at_ms: i64,
}

/// Everything learned about one session from a log chunk, hook or MCP call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionObservation {
    pub agent: Agent,
    pub client_session_id: String,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    /// Earliest and latest activity in this observation.
    pub first_at_ms: i64,
    pub last_at_ms: i64,
    pub title: Option<(String, TitleSource)>,
    pub ended: bool,
    pub files: Vec<FileObservation>,
}
impl SessionObservation {
    pub fn new(agent: Agent, client_session_id: impl Into<String>, at_ms: i64) -> Self {
        Self {
            agent,
            client_session_id: client_session_id.into(),
            cwd: None,
            git_branch: None,
            first_at_ms: at_ms,
            last_at_ms: at_ms,
            title: None,
            ended: false,
            files: Vec::new(),
        }
    }
    /// Titles or metadata without any activity: refines an existing session
    /// but never creates one or moves its activity times.
    pub fn metadata(agent: Agent, client_session_id: impl Into<String>) -> Self {
        Self {
            first_at_ms: i64::MAX,
            last_at_ms: i64::MIN,
            ..Self::new(agent, client_session_id, 0)
        }
    }
    /// Whether this observation saw any timed activity.
    pub fn has_activity(&self) -> bool {
        self.first_at_ms <= self.last_at_ms
    }
    /// Widen the activity window to include `at_ms`.
    pub fn saw(&mut self, at_ms: i64) {
        self.first_at_ms = self.first_at_ms.min(at_ms);
        self.last_at_ms = self.last_at_ms.max(at_ms);
    }
    /// Offer a title; keep it only if it outranks (or, for client-maintained
    /// names, refreshes) the one already in this observation.
    pub fn offer_title(&mut self, title: &str, source: TitleSource) {
        let Some(title) = clean_title(title) else {
            return;
        };
        let replace = match &self.title {
            None => true,
            Some((_, current)) => {
                source > *current
                    || (source == *current
                        && matches!(source, TitleSource::Ai | TitleSource::Custom))
            }
        };
        if replace {
            self.title = Some((title, source));
        }
    }
}

/// A listed session; `title` already has the fallback applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub agent: Agent,
    pub title: String,
    pub title_source: TitleSource,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub started_at_ms: i64,
    pub last_activity_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub file_count: u32,
}

/// Trimmed single-line title within budget, or `None` when nothing remains.
pub fn clean_title(value: &str) -> Option<String> {
    let line: String = value
        .split(['\n', '\r'])
        .find(|line| !line.trim().is_empty())?
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let mut end = line.len().min(MAX_TITLE);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    Some(line[..end].trim_end().to_owned())
}

/// Markdown by extension, matching the reader's scanner.
pub fn is_markdown(path: &std::path::Path) -> bool {
    path.extension().and_then(|v| v.to_str()).is_some_and(|v| {
        matches!(
            v.to_ascii_lowercase().as_str(),
            "md" | "markdown" | "mdown" | "mkd" | "mkdn"
        )
    })
}

#[cfg(test)]
mod tests;
