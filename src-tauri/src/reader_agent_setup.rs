//! Settings adapter: explicit scopes, native command quoting, no automatic install.
use crate::{
    commands::WorkspaceState,
    library::{list_library_sources, LibraryState},
    reader::ReaderBackend,
};
use mdreader_service::{
    agents::{Action, Agent, Scope},
    Request,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;
#[derive(Clone, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct SetupSelection {
    pub agent: String,
    pub scope: String,
    pub project_root: Option<String>,
}
#[derive(Serialize, Type)]
pub struct ProjectChoice {
    pub id: Option<String>,
    pub enabled: bool,
    pub monitoring: bool,
    pub path: String,
    pub name: String,
}
#[derive(Serialize, Type)]
pub struct SourceChoice {
    pub id: String,
    pub name: String,
    pub editable: bool,
}
#[derive(Serialize, Type)]
pub struct SetupState {
    pub supported: bool,
    pub installed: bool,
    pub enabled: bool,
    pub command: Option<String>,
    pub client_version: Option<String>,
    pub diagnostic: Option<String>,
    pub source_ids: Vec<String>,
    pub sources: Vec<SourceChoice>,
    pub projects: Vec<ProjectChoice>,
    pub connection_status: String,
    pub reporting_project_ids: Vec<String>,
    pub edit_source_ids: Vec<String>,
    pub connected: bool,
    pub last_seen_ms: Option<f64>,
}
async fn inspect(
    app: &tauri::AppHandle,
    input: &SetupSelection,
    workspace: State<'_, WorkspaceState>,
    library: State<'_, LibraryState>,
    reader: &ReaderBackend,
) -> Result<
    (
        Option<mdreader_service::agents::Selection>,
        Vec<ProjectChoice>,
        Agent,
    ),
    String,
> {
    let agent = match input.agent.as_str() {
        "claude" => Agent::Claude,
        "codex" => Agent::Codex,
        _ => return Err("Choose an agent".into()),
    };
    let scope = match input.scope.as_str() {
        "project" => Scope::Project,
        "user" => Scope::User,
        _ => return Err("Choose an installation scope".into()),
    };
    let snapshot = list_library_sources(app.clone(), workspace, library)
        .map_err(|_| "Folder list unavailable")?;
    let mut projects: Vec<_> = snapshot
        .sources
        .into_iter()
        .map(|s| ProjectChoice {
            id: None,
            enabled: false,
            monitoring: false,
            path: s.path,
            name: s.name,
        })
        .collect();
    let indexed: Vec<crate::reader_agent_projects::AgentProject> = reader
        .invoke(Request::AgentProjects {
            request: mdreader_service::agents::projects::ProjectRequest::ListProjects,
        })
        .await?;
    for project in &mut projects {
        if let Ok(root) = mdreader_core::SourceRoot::capture(project.path.clone().into()) {
            let found: Option<mdreader_core::Source> = reader
                .invoke(Request::FindSource {
                    path: root.path,
                    identity: root.identity,
                })
                .await?;
            if let Some(found) = found {
                project.id = Some(found.id.clone());
                project.enabled = found.enabled;
                project.monitoring = indexed.iter().any(|p| p.id == found.id && p.monitoring);
            }
        }
    }
    let selection = if scope == Scope::Project {
        match &input.project_root {
            Some(path) if projects.iter().any(|p| &p.path == path) => Some(
                crate::agent_cli::selection(agent, scope, Some(path.into()))?,
            ),
            Some(_) => return Err("Choose a saved project folder".into()),
            None => None,
        }
    } else {
        Some(crate::agent_cli::selection(agent, scope, None)?)
    };
    Ok((selection, projects, agent))
}
/// Reader source ids of Library members whose contract forbids writes. The
/// service cannot see Library contracts, so edit grants are filtered here.
async fn read_only_source_ids(
    app: &tauri::AppHandle,
    reader: &ReaderBackend,
) -> Result<Vec<String>, String> {
    let (path, _) =
        crate::library::registry_path(app).map_err(|_| "Saved folders are unavailable")?;
    let members = crate::library::LibraryStore::new(path)
        .read_query_snapshot()
        .map_err(|_| "Saved folders are unavailable")?
        .local_sources;
    let mut ids = Vec::new();
    for member in members.into_iter().filter(|member| !member.writable()) {
        let Ok((path, identity)) = member.persisted_binding() else {
            continue;
        };
        let found: Option<mdreader_core::Source> = reader
            .invoke(Request::FindSource { path, identity })
            .await?;
        ids.extend(found.map(|source| source.id));
    }
    Ok(ids)
}
async fn state(
    app: &tauri::AppHandle,
    input: SetupSelection,
    workspace: State<'_, WorkspaceState>,
    library: State<'_, LibraryState>,
    reader: &ReaderBackend,
) -> Result<SetupState, String> {
    let (selection, projects, agent) = inspect(app, &input, workspace, library, reader).await?;
    let version = tauri::async_runtime::spawn_blocking(move || crate::agent_cli::probe(agent))
        .await
        .map_err(|_| "Agent inspection unavailable")?;
    let mut result = SetupState {
        supported: true,
        installed: false,
        enabled: false,
        command: Some(format!(
            "npx skills add MagnetonIO/MDReader --skill mdreader --agent {} --copy{}",
            if agent == Agent::Claude {
                "claude-code"
            } else {
                "codex"
            },
            if input.scope == "user" {
                " --global"
            } else {
                ""
            },
        )),
        client_version: version.clone(),
        diagnostic: None,
        source_ids: vec![],
        sources: vec![],
        projects,
        connection_status: "not_verified".into(),
        reporting_project_ids: vec![],
        edit_source_ids: vec![],
        connected: false,
        last_seen_ms: None,
    };
    if let Some(selection) = selection {
        let status: mdreader_service::agents::State = reader
            .invoke(Request::AgentSetup {
                action: Action::State { selection },
            })
            .await?;
        result.reporting_project_ids = status.reporting_project_ids;
        result.edit_source_ids = status.edit_source_ids;
        result.connected = status.connected;
        result.connection_status = if status.connected {
            "connected"
        } else {
            "not_verified"
        }
        .into();
        result.last_seen_ms = status.last_seen_ms.map(|n| n as f64);
        result.installed = status.installed;
        result.enabled = status.enabled;
        result.source_ids = status.source_ids;
        let read_only = read_only_source_ids(app, reader).await?;
        result.sources = status
            .sources
            .into_iter()
            .map(|s| SourceChoice {
                editable: status.editable_source_ids.contains(&s.id) && !read_only.contains(&s.id),
                id: s.id,
                name: s.name,
            })
            .collect();
        if version.is_none() {
            result.diagnostic = Some("Install Codex or Claude Code, then refresh status.".into());
        }
    } else {
        let sources: Vec<mdreader_core::Source> = reader.invoke(Request::ListSources).await?;
        result.sources = sources
            .into_iter()
            .filter(|s| s.enabled)
            .map(|s| SourceChoice {
                editable: false,
                id: s.id,
                name: s.name,
            })
            .collect();
        result.diagnostic =
            Some("Choose a saved project folder, or install for your user account.".into());
    }
    Ok(result)
}
#[tauri::command]
#[specta::specta]
pub async fn agent_setup_state(
    app: tauri::AppHandle,
    selection: SetupSelection,
    workspace: State<'_, WorkspaceState>,
    library: State<'_, LibraryState>,
    reader: State<'_, ReaderBackend>,
) -> Result<SetupState, String> {
    state(&app, selection, workspace, library, &reader).await
}
/// Install only after an explicit renderer action. Paths and executable are
/// captured by native code; no renderer-provided launcher can be installed.
#[tauri::command]
#[specta::specta]
pub async fn agent_setup_install(
    app: tauri::AppHandle,
    selection: SetupSelection,
    workspace: State<'_, WorkspaceState>,
    library: State<'_, LibraryState>,
    reader: State<'_, ReaderBackend>,
) -> Result<SetupState, String> {
    #[cfg(mobile)]
    {
        let _ = (app, selection, workspace, library, reader);
        Err("Agent installation is available in desktop MDReader".into())
    }
    #[cfg(not(mobile))]
    {
        let (selected, _, agent) = inspect(
            &app,
            &selection,
            workspace.clone(),
            library.clone(),
            &reader,
        )
        .await?;
        let selected = selected.ok_or("Choose a project folder")?;
        let version = tauri::async_runtime::spawn_blocking(move || crate::agent_cli::probe(agent))
            .await
            .map_err(|_| "Agent inspection unavailable")?
            .ok_or("Install the agent CLI, then try again")?;
        let executable = std::env::current_exe().map_err(|_| "Executable unavailable")?;
        let installed: mdreader_service::agents::State = reader
            .invoke(Request::AgentSetup {
                action: Action::Install {
                    selection: selected,
                    executable,
                    client_version: version,
                },
            })
            .await?;
        if !installed.installed {
            return Err("Agent installation could not finish; existing files were retained".into());
        }
        state(&app, selection, workspace, library, &reader).await
    }
}
#[tauri::command]
#[specta::specta]
pub async fn agent_setup_set_mcp(
    app: tauri::AppHandle,
    selection: SetupSelection,
    sources: Vec<String>,
    enabled: bool,
    workspace: State<'_, WorkspaceState>,
    library: State<'_, LibraryState>,
    reader: State<'_, ReaderBackend>,
) -> Result<SetupState, String> {
    let selected = inspect(
        &app,
        &selection,
        workspace.clone(),
        library.clone(),
        &reader,
    )
    .await?
    .0
    .ok_or("Choose a project folder")?;
    let _: mdreader_service::agents::State = reader
        .invoke(Request::AgentSetup {
            action: Action::SetMcp {
                selection: selected,
                sources,
                enabled,
            },
        })
        .await?;
    state(&app, selection, workspace, library, &reader).await
}
#[tauri::command]
#[specta::specta]
// Tauri injects the application and three state handles alongside explicit grants.
#[allow(clippy::too_many_arguments)]
pub async fn agent_setup_set_permissions(
    app: tauri::AppHandle,
    selection: SetupSelection,
    enabled: bool,
    project_ids: Vec<String>,
    read_source_ids: Vec<String>,
    edit_source_ids: Vec<String>,
    workspace: State<'_, WorkspaceState>,
    library: State<'_, LibraryState>,
    reader: State<'_, ReaderBackend>,
) -> Result<SetupState, String> {
    if enabled {
        let read_only = read_only_source_ids(&app, &reader).await?;
        if edit_source_ids.iter().any(|id| read_only.contains(id)) {
            return Err("That source is read-only; agents cannot edit it".into());
        }
    }
    if enabled {
        // An explicit enable/access-save also updates receipt-owned launchers
        // to this application. Inspection never mutates client configuration.
        agent_setup_install(
            app.clone(),
            selection.clone(),
            workspace.clone(),
            library.clone(),
            reader.clone(),
        )
        .await?;
    }
    let selected = inspect(
        &app,
        &selection,
        workspace.clone(),
        library.clone(),
        &reader,
    )
    .await?
    .0
    .ok_or("Choose a project folder")?;
    let _: mdreader_service::agents::State = reader
        .invoke(Request::AgentSetup {
            action: Action::SetPermissions {
                selection: selected,
                enabled,
                project_ids,
                read_source_ids,
                edit_source_ids,
            },
        })
        .await?;
    state(&app, selection, workspace, library, &reader).await
}
#[tauri::command]
#[specta::specta]
pub async fn agent_setup_uninstall(
    app: tauri::AppHandle,
    selection: SetupSelection,
    workspace: State<'_, WorkspaceState>,
    library: State<'_, LibraryState>,
    reader: State<'_, ReaderBackend>,
) -> Result<SetupState, String> {
    let selected = inspect(
        &app,
        &selection,
        workspace.clone(),
        library.clone(),
        &reader,
    )
    .await?
    .0
    .ok_or("Choose a project folder")?;
    let _: mdreader_service::agents::State = reader
        .invoke(Request::AgentSetup {
            action: Action::Uninstall {
                selection: selected,
            },
        })
        .await?;
    state(&app, selection, workspace, library, &reader).await
}
