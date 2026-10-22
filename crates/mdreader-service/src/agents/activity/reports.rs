//! Grant-authorized reports from hooks and MCP tools. They write the same
//! activity tables as log ingestion; the old project-scoped tables are no
//! longer written. The caller has already authorized the connected grant;
//! no project grant is involved.
use mdreader_core::agent_activity::{
    Agent, FileObservation, FileOp, SessionObservation, TitleSource,
};
use mdreader_core::{ReaderError, ReaderStore, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Files one hook event may report.
const MAX_REPORTED_FILES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportedFile {
    pub path: PathBuf,
    pub op: FileOp,
}

fn invalid() -> ReaderError {
    ReaderError::Invalid("invalid agent activity report".into())
}

fn now() -> Result<i64> {
    mdreader_core::agent_sessions::now_ms()
}

fn session_id(store: &mut ReaderStore, agent: Agent, client: &str) -> Result<String> {
    store
        .agent_session_id(agent, client)?
        .ok_or(ReaderError::Unavailable)
}

/// A hook event: session id, working directory and any Markdown files
/// written. No project needs to contain `cwd`. Recorded only while the
/// agent's "Track sessions" is on.
pub(crate) fn record(
    store: &mut ReaderStore,
    agent: Agent,
    client_session_id: &str,
    cwd: &Path,
    files: Vec<ReportedFile>,
    ended: bool,
    title: Option<&str>,
) -> Result<Value> {
    let cwd = cwd
        .to_str()
        .filter(|c| c.len() <= 4096 && Path::new(c).is_absolute());
    if cwd.is_none() || files.len() > MAX_REPORTED_FILES {
        return Err(invalid());
    }
    let at = now()?;
    let mut o = SessionObservation::new(agent, client_session_id, at);
    o.cwd = cwd.map(str::to_owned);
    o.ended = ended;
    if let Some(title) = title {
        o.offer_title(title, TitleSource::Custom);
    }
    o.files.extend(
        files
            .into_iter()
            .filter(|f| f.path.is_absolute() && mdreader_core::agent_activity::is_markdown(&f.path))
            .map(|f| FileObservation {
                abs_path: f.path,
                op: f.op,
                at_ms: at,
            }),
    );
    // Hooks are automatic capture: the tracking switch is re-checked in the
    // write transaction itself.
    let changed = store.record_tracked_activity(&[o], None)?;
    Ok(match store.agent_session_id(agent, client_session_id)? {
        Some(id) => json!({"session_id": id, "changed": !changed.is_empty()}),
        None => json!({"recorded": false}),
    })
}

/// MCP `register_session`. No project grant is needed; `cwd`, when known,
/// lets later documents be named relative to it.
pub(crate) fn register(
    store: &mut ReaderStore,
    agent: Agent,
    project: Option<&str>,
    cwd: Option<&Path>,
    client_session_id: &str,
    title: &str,
) -> Result<Value> {
    let cwd = match cwd {
        Some(cwd) => Some(
            cwd.to_str()
                .filter(|c| c.len() <= 4096 && cwd.is_absolute())
                .ok_or_else(invalid)?
                .to_owned(),
        ),
        None => None,
    };
    let mut o = SessionObservation::new(agent, client_session_id, now()?);
    o.cwd = cwd;
    o.offer_title(title, TitleSource::Custom);
    store.record_agent_activity(&[o], None)?;
    Ok(json!({
        "id": session_id(store, agent, client_session_id)?,
        "project_id": project,
        "agent": agent.as_str(),
        "title": title,
    }))
}

/// MCP `register_document`: an existing Markdown file, absolute or relative
/// to the session's working directory.
pub(crate) fn document(
    store: &mut ReaderStore,
    agent: Agent,
    session: &str,
    relative: Option<&str>,
    absolute: Option<&Path>,
) -> Result<Value> {
    let (client, cwd) = store.agent_session_ref(agent, session)?;
    let path: PathBuf = match (absolute, relative) {
        (Some(path), None) if path.is_absolute() => path.to_owned(),
        (None, Some(relative)) if mdreader_core::agent_sessions::markdown_path(relative) => {
            Path::new(&cwd.ok_or(ReaderError::Unavailable)?).join(relative)
        }
        _ => return Err(invalid()),
    };
    if path.to_str().is_none_or(|p| p.len() > 4096)
        || !mdreader_core::agent_activity::is_markdown(&path)
        || !std::fs::metadata(&path).is_ok_and(|m| m.is_file())
    {
        return Err(ReaderError::Unavailable);
    }
    let mut o = SessionObservation::new(agent, client, now()?);
    o.files.push(FileObservation {
        abs_path: path,
        op: FileOp::Edit,
        at_ms: o.last_at_ms,
    });
    store.record_agent_activity(&[o], None)?;
    Ok(json!({"registered":true}))
}

/// MCP `end_session`.
pub(crate) fn end(store: &mut ReaderStore, agent: Agent, session: &str) -> Result<Value> {
    let (client, _) = store.agent_session_ref(agent, session)?;
    let mut o = SessionObservation::new(agent, client, now()?);
    o.ended = true;
    store.record_agent_activity(&[o], None)?;
    Ok(json!({"ended":true}))
}
