//! Owner-routed session reporting, isolated from optional document access.
use super::{grants, invalid, permissions, Agent};
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
    /// A file outside the session's project, such as a Claude Code plan.
    RegisterSourceDocument {
        session_id: String,
        source_id: String,
        relative_path: String,
    },
    EndSession {
        session_id: String,
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
            serialize(store.register_agent_session(
                &project_id,
                id,
                &client_session_id,
                match agent {
                    Agent::Codex => "codex",
                    Agent::Claude => "claude",
                },
                &title,
            )?)?
        }
        AgentRequest::RegisterDocument {
            session_id,
            relative_path,
        } => {
            let source = store.agent_session_project(id, &session_id)?;
            project(store, id, &source)?;
            store.register_agent_document(id, &session_id, &relative_path)?;
            json!({"registered":true})
        }
        AgentRequest::RegisterSourceDocument {
            session_id,
            source_id,
            relative_path,
        } => {
            let project_id = store.agent_session_project(id, &session_id)?;
            project(store, id, &project_id)?;
            // Linking adds a visible association, so the user must already
            // have chosen this source for the grant to read or report on.
            let readable = grants::grant(store, id)?.1;
            let readable = grants::resolve(store, readable)?;
            if !(readable.contains(&source_id) || permissions.projects.contains(&source_id))
                || !store
                    .list_sources()?
                    .iter()
                    .any(|s| s.enabled && s.id == source_id)
            {
                return Err(ReaderError::Unavailable);
            }
            store.register_agent_source_document(id, &session_id, &source_id, &relative_path)?;
            json!({"registered":true})
        }
        AgentRequest::EndSession { session_id } => {
            let source = store.agent_session_project(id, &session_id)?;
            project(store, id, &source)?;
            store.end_agent_session(id, &session_id)?;
            json!({"ended":true})
        }
        _ => return Err(invalid()),
    };
    if authorized(store, id)?.0 != generation {
        return Err(ReaderError::Cancelled);
    }
    permissions::touch(store, id)?;
    Ok(value)
}
