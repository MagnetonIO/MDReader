//! Native projections for project/session browsing; no filesystem discovery.
use crate::reader::ReaderBackend;
use mdreader_service::{agents::projects::ProjectRequest, Request};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

#[derive(Deserialize, Serialize, Type)]
pub struct AgentProject {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub monitoring: bool,
}
#[derive(Deserialize, Serialize, Type)]
pub struct AgentSession {
    pub id: String,
    pub project_id: String,
    pub agent: String,
    pub title: String,
    pub started_at_ms: f64,
    pub last_seen_ms: f64,
    pub ended_at_ms: Option<f64>,
}
#[derive(Deserialize, Serialize, Type)]
pub struct AgentSessionDocument {
    pub source_id: String,
    pub path: String,
    pub title: String,
    pub revision: String,
    pub session_ids: Vec<String>,
}
#[tauri::command]
#[specta::specta]
pub async fn agent_projects_list(
    reader: State<'_, ReaderBackend>,
) -> Result<Vec<AgentProject>, String> {
    reader
        .invoke(Request::AgentProjects {
            request: ProjectRequest::ListProjects,
        })
        .await
}
#[tauri::command]
#[specta::specta]
pub async fn agent_project_sessions(
    project_id: String,
    reader: State<'_, ReaderBackend>,
) -> Result<Vec<AgentSession>, String> {
    reader
        .invoke(Request::AgentProjects {
            request: ProjectRequest::ListSessions { project_id },
        })
        .await
}
#[tauri::command]
#[specta::specta]
pub async fn agent_project_documents(
    project_id: String,
    session_id: Option<String>,
    reader: State<'_, ReaderBackend>,
) -> Result<Vec<AgentSessionDocument>, String> {
    reader
        .invoke(Request::AgentProjects {
            request: ProjectRequest::ListDocuments {
                project_id,
                session_id,
            },
        })
        .await
}
