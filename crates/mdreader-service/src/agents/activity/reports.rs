//! Grant-authorized reports from hooks and MCP tools. They write the same
//! activity tables as log ingestion; the old project-scoped tables are no
//! longer written. The caller has already authorized the grant and project.
use mdreader_core::agent_activity::{
    Agent, FileObservation, FileOp, SessionObservation, TitleSource,
};
use mdreader_core::{ReaderError, ReaderStore, Result, SourceRoot};
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
/// written. No project needs to contain `cwd`.
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
    let changed = store.record_agent_activity(&[o], None)?;
    Ok(json!({
        "session_id": session_id(store, agent, client_session_id)?,
        "changed": !changed.is_empty(),
    }))
}

/// MCP `register_session`: a session rooted at a granted project.
pub(crate) fn register(
    store: &mut ReaderStore,
    agent: Agent,
    project: &str,
    root: &SourceRoot,
    client_session_id: &str,
    title: &str,
) -> Result<Value> {
    let mut o = SessionObservation::new(agent, client_session_id, now()?);
    o.cwd = root.path.to_str().map(str::to_owned);
    o.offer_title(title, TitleSource::Custom);
    store.record_agent_activity(&[o], None)?;
    root.verify()?;
    Ok(json!({
        "id": session_id(store, agent, client_session_id)?,
        "project_id": project,
        "agent": agent.as_str(),
        "title": title,
    }))
}

/// MCP `register_document`: an existing project-relative Markdown file.
pub(crate) fn document(
    store: &mut ReaderStore,
    agent: Agent,
    session: &str,
    project: &str,
    root: &SourceRoot,
    relative: &str,
) -> Result<Value> {
    if !mdreader_core::agent_sessions::markdown_path(relative) {
        return Err(invalid());
    }
    let file = root
        .open_optional_file_relative(Path::new(relative))?
        .ok_or(ReaderError::Unavailable)?;
    mdreader_core::source_root::file_stamp::single_link_stamp(&file)?;
    let (client, _) = store.agent_session_ref(agent, session)?;
    let mut o = SessionObservation::new(agent, client, now()?);
    o.files.push(FileObservation {
        abs_path: root.path.join(relative),
        op: FileOp::Edit,
        at_ms: o.last_at_ms,
    });
    store.record_agent_activity(&[o], None)?;
    root.verify()?;
    store.queue_folder_changes(project, &[relative.to_owned()])?;
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
