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
    pub path: String,
    pub name: String,
}
#[derive(Serialize, Type)]
pub struct SourceChoice {
    pub id: String,
    pub name: String,
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
}
async fn inspect(
    app: &tauri::AppHandle,
    input: &SetupSelection,
    workspace: State<'_, WorkspaceState>,
    library: State<'_, LibraryState>,
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
    let projects: Vec<_> = snapshot
        .sources
        .into_iter()
        .map(|s| ProjectChoice {
            path: s.path,
            name: s.name,
        })
        .collect();
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
async fn state(
    app: &tauri::AppHandle,
    input: SetupSelection,
    workspace: State<'_, WorkspaceState>,
    library: State<'_, LibraryState>,
    reader: &ReaderBackend,
) -> Result<SetupState, String> {
    let (selection, projects, agent) = inspect(app, &input, workspace, library).await?;
    let version = tauri::async_runtime::spawn_blocking(move || crate::agent_cli::probe(agent))
        .await
        .map_err(|_| "Agent inspection unavailable")?;
    let mut result = SetupState {
        supported: true,
        installed: false,
        enabled: false,
        command: None,
        client_version: version.clone(),
        diagnostic: None,
        source_ids: vec![],
        sources: vec![],
        projects,
        connection_status: "not_verified".into(),
    };
    if let Some(selection) = selection {
        let status: mdreader_service::agents::State = reader
            .invoke(Request::AgentSetup {
                action: Action::State { selection },
            })
            .await?;
        result.installed = status.installed;
        result.enabled = status.enabled;
        result.source_ids = status.source_ids;
        result.sources = status
            .sources
            .into_iter()
            .map(|s| SourceChoice {
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
        let (selected, _, agent) =
            inspect(&app, &selection, workspace.clone(), library.clone()).await?;
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
    let selected = inspect(&app, &selection, workspace.clone(), library.clone())
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
pub async fn agent_setup_uninstall(
    app: tauri::AppHandle,
    selection: SetupSelection,
    workspace: State<'_, WorkspaceState>,
    library: State<'_, LibraryState>,
    reader: State<'_, ReaderBackend>,
) -> Result<SetupState, String> {
    let selected = inspect(&app, &selection, workspace.clone(), library.clone())
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
