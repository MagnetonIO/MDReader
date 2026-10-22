//! Agent session activity: opt-in reading of Claude Code and Codex logs for
//! session titles and the Markdown files each session created or edited.
//! The app's watcher names changed logs; this owner validates, reads and
//! records them. No message text is stored or logged.
mod claude_log;
mod codex_log;
mod discover;
mod ingest;
pub(crate) mod reports;
mod titles;
use mdreader_core::agent_activity::{
    self as core, Agent, ExactReference, FileOp, FileUnavailable, Page, Tracking,
};
use mdreader_core::{ReaderError, ReaderStore, Result};
pub use reports::ReportedFile;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::ops::DerefMut;
use std::path::{Path, PathBuf};

/// Bytes read per `Ingest` call before the rest is handed back as pending, so
/// one request stays well inside the IPC timeout.
const INGEST_BUDGET: u64 = 64 * 1024 * 1024;
const MAX_INGEST_PATHS: usize = 512;

/// Where this machine's agents keep their logs, from the owner's own home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Homes {
    pub claude_projects: PathBuf,
    pub claude_plans: PathBuf,
    pub codex_sessions: PathBuf,
    pub codex_index: PathBuf,
}
impl Homes {
    pub fn under(home: &Path) -> Self {
        Self {
            claude_projects: home.join(".claude/projects"),
            claude_plans: home.join(".claude/plans"),
            codex_sessions: home.join(".codex/sessions"),
            codex_index: home.join(".codex/session_index.jsonl"),
        }
    }
    pub fn from_env() -> Result<Self> {
        #[cfg(windows)]
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
        #[cfg(not(windows))]
        let home = std::env::var_os("HOME");
        let home = PathBuf::from(home.ok_or(ReaderError::Unavailable)?);
        if !home.is_absolute() {
            return Err(ReaderError::Unavailable);
        }
        Ok(Self::under(&home))
    }
    fn available(&self, agent: Agent) -> bool {
        match agent {
            Agent::Claude => self.claude_projects.is_dir(),
            Agent::Codex => self.codex_sessions.is_dir(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActivityRequest {
    List {
        agent: Option<Agent>,
        offset: u32,
        limit: u32,
        filter: Option<String>,
    },
    Files {
        session_id: String,
    },
    Rename {
        session_id: String,
        title: String,
    },
    Hide {
        session_id: String,
    },
    Tracking,
    SetTracking {
        agent: Agent,
        enabled: bool,
    },
    LoadOlder {
        days: u32,
    },
    Forget {
        agent: Agent,
    },
    /// Logs of enabled agents inside their history window that changed since
    /// they were last read, newest first.
    Discover,
    /// Read the named logs (only known log locations of enabled agents).
    Ingest {
        paths: Vec<PathBuf>,
    },
    Revision,
    /// The working directory recorded for a listed session, if any.
    SessionCwd {
        session_id: String,
    },
    /// Read an "Agent files" exact document; `null` when the id is not one.
    ReadExact {
        source_id: String,
        path: String,
        revision: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingState {
    pub agent: Agent,
    pub enabled: bool,
    /// Whether this agent's log folder exists on this machine.
    pub available: bool,
    pub since_ms: Option<i64>,
    pub sessions: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityFile {
    pub abs_path: String,
    pub display_path: String,
    pub op: FileOp,
    pub first_at_ms: i64,
    pub last_at_ms: i64,
    pub exists: bool,
    pub is_plan: bool,
    pub reference: Option<ExactReference>,
    pub unavailable: Option<FileUnavailable>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestReply {
    pub changed: Vec<String>,
    /// Paths not read because this call's byte budget ran out.
    pub pending: Vec<PathBuf>,
    pub revision: i64,
}

fn serialize<T: Serialize>(value: T) -> Result<Value> {
    serde_json::to_value(value).map_err(|_| ReaderError::Unavailable)
}

fn now_ms() -> Result<i64> {
    mdreader_core::agent_sessions::now_ms()
}

fn tracking<G: DerefMut<Target = ReaderStore>>(
    lock: &dyn Fn() -> Result<G>,
    homes: &Homes,
) -> Result<Vec<TrackingState>> {
    let rows: Vec<Tracking> = lock()?.agent_tracking()?;
    Ok(rows
        .into_iter()
        .map(|t| TrackingState {
            available: homes.available(t.agent),
            agent: t.agent,
            enabled: t.enabled,
            since_ms: t.since_ms,
            sessions: t.sessions,
        })
        .collect())
}

fn display_path(abs: &str, cwd: Option<&str>) -> String {
    cwd.and_then(|cwd| Path::new(abs).strip_prefix(cwd).ok())
        .and_then(|p| p.to_str())
        .filter(|p| !p.is_empty())
        .map_or_else(|| abs.to_owned(), str::to_owned)
}

fn files<G: DerefMut<Target = ReaderStore>>(
    lock: &dyn Fn() -> Result<G>,
    homes: &Homes,
    session: &str,
) -> Result<Vec<ActivityFile>> {
    let (cwd, rows, private) = {
        let mut store = lock()?;
        let (cwd, rows) = store.agent_activity_files(session)?;
        (cwd, rows, store.agent_private_paths()?)
    };
    // References read each file, so they are resolved outside the lock.
    Ok(rows
        .into_iter()
        .map(|row| {
            let reference = core::file_reference(&private, &row.abs_path, &row.exact_id);
            let abs = Path::new(&row.abs_path);
            ActivityFile {
                display_path: display_path(&row.abs_path, cwd.as_deref()),
                exists: std::fs::metadata(abs).is_ok_and(|m| m.is_file()),
                is_plan: abs.starts_with(&homes.claude_plans),
                op: row.op,
                first_at_ms: row.first_at_ms,
                last_at_ms: row.last_at_ms,
                unavailable: reference.as_ref().err().copied(),
                reference: reference.ok(),
                abs_path: row.abs_path,
            }
        })
        .collect())
}

fn ingest_paths<G: DerefMut<Target = ReaderStore>>(
    lock: &dyn Fn() -> Result<G>,
    homes: &Homes,
    paths: Vec<PathBuf>,
) -> Result<IngestReply> {
    if paths.len() > MAX_INGEST_PATHS {
        return Err(ReaderError::Invalid("too many logs in one request".into()));
    }
    let enabled: Vec<Agent> = lock()?
        .agent_tracking()?
        .into_iter()
        .filter(|t| t.enabled)
        .map(|t| t.agent)
        .collect();
    let mut names = ingest::ThreadNames::new(homes);
    let mut changed = std::collections::BTreeSet::new();
    let (mut budget, mut pending) = (INGEST_BUDGET, Vec::new());
    for path in paths {
        if budget == 0 {
            pending.push(path);
            continue;
        }
        let Some(log) = ingest::classify(homes, &path) else {
            continue;
        };
        if !enabled.contains(&log.agent()) {
            continue;
        }
        // One unreadable or vanished log never blocks the others.
        if let Ok((ids, read)) = ingest::ingest(lock, &log, &path, &mut names) {
            changed.extend(ids);
            budget = budget.saturating_sub(read);
        }
    }
    Ok(IngestReply {
        changed: changed.into_iter().collect(),
        pending,
        revision: lock()?.agent_activity_revision()?,
    })
}

pub fn execute<G: DerefMut<Target = ReaderStore>>(
    lock: &dyn Fn() -> Result<G>,
    homes: &Homes,
    request: ActivityRequest,
) -> Result<Value> {
    match request {
        ActivityRequest::List {
            agent,
            offset,
            limit,
            filter,
        } => {
            let page: Page =
                lock()?.agent_activity_page(agent, offset, limit, filter.as_deref())?;
            serialize(page)
        }
        ActivityRequest::Files { session_id } => serialize(files(lock, homes, &session_id)?),
        ActivityRequest::Rename { session_id, title } => {
            lock()?.rename_agent_session(&session_id, &title)?;
            Ok(json!({"renamed":true}))
        }
        ActivityRequest::Hide { session_id } => {
            lock()?.hide_agent_session(&session_id)?;
            Ok(json!({"hidden":true}))
        }
        ActivityRequest::Tracking => serialize(tracking(lock, homes)?),
        ActivityRequest::SetTracking { agent, enabled } => {
            lock()?.set_agent_tracking(agent, enabled, now_ms()?)?;
            serialize(tracking(lock, homes)?)
        }
        ActivityRequest::LoadOlder { days } => {
            lock()?.extend_agent_history(i64::from(days), now_ms()?)?;
            serialize(tracking(lock, homes)?)
        }
        ActivityRequest::Forget { agent } => {
            lock()?.forget_agent_activity(agent)?;
            serialize(tracking(lock, homes)?)
        }
        ActivityRequest::Discover => serialize(discover::changed_logs(lock, homes)?),
        ActivityRequest::Ingest { paths } => serialize(ingest_paths(lock, homes, paths)?),
        ActivityRequest::Revision => serialize(lock()?.agent_activity_revision()?),
        ActivityRequest::SessionCwd { session_id } => {
            serialize(lock()?.agent_activity_files(&session_id)?.0)
        }
        ActivityRequest::ReadExact {
            source_id,
            path,
            revision,
        } => {
            let (recorded, private) = {
                let mut store = lock()?;
                // This probe runs before every local catalog read, so an
                // activity namespace fault must close only agent files, never
                // fail ordinary library reads.
                match store.agent_file_path(&source_id) {
                    Ok(Some(recorded)) => (recorded, store.agent_private_paths()?),
                    Ok(None) | Err(_) => return Ok(Value::Null),
                }
            };
            serialize(core::read_file(
                &private,
                &recorded,
                &source_id,
                &path,
                revision.as_deref(),
            )?)
        }
    }
}

#[cfg(test)]
mod scale_probe;
#[cfg(test)]
mod tests;
