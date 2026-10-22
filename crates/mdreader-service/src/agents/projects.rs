//! Owner-routed session reporting, isolated from optional document access.
use super::{activity::reports, grants, invalid, permissions, Agent};
pub use mdreader_core::agent_sessions::Project;
use mdreader_core::{ReaderError, ReaderStore, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectRequest {
    ListProjects,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentRequest {
    Handshake,
    ListProjects,
    FindProject {
        path: std::path::PathBuf,
    },
    /// Needs no project grant. A granted `project_id` roots the session at
    /// that project; otherwise `cwd` (absolute) does, if given.
    RegisterSession {
        #[serde(default)]
        project_id: Option<String>,
        client_session_id: String,
        title: String,
        #[serde(default)]
        cwd: Option<std::path::PathBuf>,
    },
    /// An existing Markdown file: absolute `path`, or `relative_path` under
    /// the session's working directory.
    RegisterDocument {
        session_id: String,
        #[serde(default)]
        relative_path: Option<String>,
        #[serde(default)]
        path: Option<std::path::PathBuf>,
    },
    EndSession {
        session_id: String,
    },
    /// A hook event. `cwd` need not be inside any project. Recorded only
    /// while "Track sessions" is on for the grant's agent.
    RecordActivity {
        client_session_id: String,
        cwd: std::path::PathBuf,
        files: Vec<super::activity::ReportedFile>,
        ended: bool,
        /// A name the client gave the session (`session_title`), if any.
        #[serde(default)]
        title: Option<String>,
    },
    CreateDocument {
        source_id: String,
        path: String,
        operation_id: String,
        markdown: String,
    },
    UpdateDocument {
        source_id: String,
        path: String,
        operation_id: String,
        expected_revision: String,
        markdown: String,
    },
    InspectOperation {
        operation_id: String,
    },
}
impl std::fmt::Debug for AgentRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AgentRequest(<redacted>)")
    }
}
pub fn browse(store: &mut ReaderStore, request: ProjectRequest) -> Result<Value> {
    match request {
        ProjectRequest::ListProjects => serialize(store.agent_projects()?),
    }
}
fn activity_agent(agent: Agent) -> mdreader_core::agent_activity::Agent {
    match agent {
        Agent::Claude => mdreader_core::agent_activity::Agent::Claude,
        Agent::Codex => mdreader_core::agent_activity::Agent::Codex,
    }
}
pub(super) fn serialize<T: Serialize>(value: T) -> Result<Value> {
    serde_json::to_value(value).map_err(|_| invalid())
}
pub(super) fn authorized(
    store: &mut ReaderStore,
    id: &str,
) -> Result<(u64, Agent, permissions::Permissions)> {
    let (enabled, _, generation) = grants::grant(store, id)?;
    let record = grants::record(store, id)?.ok_or(ReaderError::Unavailable)?;
    if !enabled || record.phase != "installed" {
        return Err(ReaderError::Unavailable);
    }
    record.selection.checked()?;
    Ok((
        generation,
        record.selection.agent,
        permissions::get(store, id)?,
    ))
}
/// A hook event is consented by two separate choices: the agent was
/// connected (an installed grant) and "Track sessions" is on for it. The MCP
/// access switch governs tools the agent calls, not these hook reports.
fn hook(store: &mut ReaderStore, id: &str, request: AgentRequest) -> Result<Value> {
    let AgentRequest::RecordActivity {
        client_session_id,
        cwd,
        files,
        ended,
        title,
    } = request
    else {
        return Err(invalid());
    };
    let record = grants::record(store, id)?.ok_or(ReaderError::Unavailable)?;
    if record.phase != "installed" {
        return Err(ReaderError::Unavailable);
    }
    record.selection.checked()?;
    let agent = activity_agent(record.selection.agent);
    if !store
        .agent_tracking()?
        .iter()
        .any(|t| t.agent == agent && t.enabled)
    {
        return Ok(json!({"recorded":false}));
    }
    let value = reports::record(
        store,
        agent,
        &client_session_id,
        &cwd,
        files,
        ended,
        title.as_deref(),
    )?;
    permissions::touch(store, id)?;
    Ok(value)
}
pub fn report(store: &mut ReaderStore, id: &str, request: AgentRequest) -> Result<Value> {
    if matches!(request, AgentRequest::RecordActivity { .. }) {
        return hook(store, id, request);
    }
    let (generation, agent, permissions) = authorized(store, id)?;
    let value = match request {
        AgentRequest::Handshake => json!({"connected":true}),
        AgentRequest::ListProjects => serialize(
            store
                .agent_projects()?
                .into_iter()
                .filter(|p| p.monitoring && permissions.projects.contains(&p.id))
                .collect::<Vec<_>>(),
        )?,
        AgentRequest::FindProject { path } => {
            if !path.is_absolute() {
                return Err(invalid());
            }
            let mut found = None;
            let mut relative = None;
            let mut depth = 0;
            for id in &permissions.projects {
                let Ok(root) = store.chosen_local_root(id) else {
                    continue;
                };
                let relative_path = if path == root.path {
                    root.verify().map(|_| std::path::PathBuf::new())
                } else {
                    root.relative_path(&path)
                };
                if let Ok(path) = relative_path {
                    if root.path.components().count() > depth
                        && store
                            .folder_index_status(id)
                            .is_ok_and(|s| s.monitor_enabled)
                    {
                        let value = path.to_str().ok_or(ReaderError::Unavailable)?;
                        if value.len() > 4096 {
                            return Err(invalid());
                        }
                        root.verify()?;
                        depth = root.path.components().count();
                        found = Some(id.clone());
                        relative = Some(value.replace('\\', "/"));
                    }
                }
            }
            json!({"project_id":found,"relative_path":relative})
        }
        AgentRequest::RegisterSession {
            project_id,
            client_session_id,
            title,
            cwd,
        } => {
            let root = project_id
                .as_deref()
                .filter(|p| permissions.projects.iter().any(|g| g == p))
                .and_then(|p| store.chosen_local_root(p).ok());
            let cwd = root.as_ref().map(|r| r.path.clone()).or(cwd);
            reports::register(
                store,
                activity_agent(agent),
                project_id.as_deref(),
                cwd.as_deref(),
                &client_session_id,
                &title,
            )?
        }
        AgentRequest::RegisterDocument {
            session_id,
            relative_path,
            path,
        } => reports::document(
            store,
            activity_agent(agent),
            &session_id,
            relative_path.as_deref(),
            path.as_deref(),
        )?,
        AgentRequest::EndSession { session_id } => {
            reports::end(store, activity_agent(agent), &session_id)?
        }
        _ => return Err(invalid()),
    };
    if authorized(store, id)?.0 != generation {
        return Err(ReaderError::Cancelled);
    }
    permissions::touch(store, id)?;
    Ok(value)
}
