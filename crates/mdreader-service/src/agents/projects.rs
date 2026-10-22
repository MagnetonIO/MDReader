//! Owner-routed session reporting, isolated from optional document access.
use super::{activity::reports, grants, invalid, permissions, Agent};
pub use mdreader_core::agent_sessions::{Project, Session, SessionDocument};
use mdreader_core::{ReaderError, ReaderStore, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectRequest {
    ListProjects,
    ListSessions {
        project_id: String,
    },
    ListDocuments {
        project_id: String,
        session_id: Option<String>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentRequest {
    Handshake,
    ListProjects,
    FindProject {
        path: std::path::PathBuf,
    },
    RegisterSession {
        project_id: String,
        client_session_id: String,
        title: String,
    },
    RegisterDocument {
        session_id: String,
        relative_path: String,
    },
    EndSession {
        session_id: String,
    },
    /// A hook event. `cwd` need not be inside any project.
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
        ProjectRequest::ListSessions { project_id } => {
            serialize(store.agent_sessions(&project_id)?)
        }
        ProjectRequest::ListDocuments {
            project_id,
            session_id,
        } => serialize(store.agent_documents(&project_id, session_id.as_deref())?),
    }
}
fn activity_agent(agent: Agent) -> mdreader_core::agent_activity::Agent {
    match agent {
        Agent::Claude => mdreader_core::agent_activity::Agent::Claude,
        Agent::Codex => mdreader_core::agent_activity::Agent::Codex,
    }
}
/// The granted project an MCP-registered session is rooted at.
fn session_project(
    store: &mut ReaderStore,
    permissions: &permissions::Permissions,
    agent: mdreader_core::agent_activity::Agent,
    session: &str,
) -> Result<(String, mdreader_core::SourceRoot)> {
    let (_, cwd) = store.agent_session_ref(agent, session)?;
    let cwd = cwd.ok_or(ReaderError::Unavailable)?;
    permissions
        .projects
        .iter()
        .find_map(|p| {
            let root = store.chosen_local_root(p).ok()?;
            (root.path.to_str() == Some(cwd.as_str())).then(|| (p.clone(), root))
        })
        .ok_or(ReaderError::Unavailable)
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
pub(super) fn project(store: &mut ReaderStore, id: &str, project: &str) -> Result<u64> {
    let (generation, _, permissions) = authorized(store, id)?;
    if !permissions.projects.iter().any(|p| p == project)
        || !store.folder_index_status(project)?.monitor_enabled
    {
        return Err(ReaderError::Unavailable);
    }
    store.chosen_local_root(project)?.verify()?;
    Ok(generation)
}
pub fn report(store: &mut ReaderStore, id: &str, request: AgentRequest) -> Result<Value> {
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
        } => {
            project(store, id, &project_id)?;
            let root = store.chosen_local_root(&project_id)?;
            reports::register(
                store,
                activity_agent(agent),
                &project_id,
                &root,
                &client_session_id,
                &title,
            )?
        }
        AgentRequest::RegisterDocument {
            session_id,
            relative_path,
        } => {
            let (project_id, root) =
                session_project(store, &permissions, activity_agent(agent), &session_id)?;
            project(store, id, &project_id)?;
            reports::document(
                store,
                activity_agent(agent),
                &session_id,
                &project_id,
                &root,
                &relative_path,
            )?
        }
        AgentRequest::EndSession { session_id } => {
            let (project_id, _) =
                session_project(store, &permissions, activity_agent(agent), &session_id)?;
            project(store, id, &project_id)?;
            reports::end(store, activity_agent(agent), &session_id)?
        }
        AgentRequest::RecordActivity {
            client_session_id,
            cwd,
            files,
            ended,
            title,
        } => reports::record(
            store,
            activity_agent(agent),
            &client_session_id,
            &cwd,
            files,
            ended,
            title.as_deref(),
        )?,
        _ => return Err(invalid()),
    };
    if authorized(store, id)?.0 != generation {
        return Err(ReaderError::Cancelled);
    }
    permissions::touch(store, id)?;
    Ok(value)
}
