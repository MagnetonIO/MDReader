//! Current-worktree capture controls. Agent histories are never discovered here.
use crate::commands::WorkspaceState;
use crate::reader::ReaderBackend;
use mdreader_capture::ProjectPolicy;
use mdreader_service::Request;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::Path;
use tauri::State;

#[derive(Debug, Serialize, Type)]
pub struct AgentCaptureState {
    pub root: String,
    pub enabled: bool,
    pub capture_subagents: bool,
    pub excluded_paths: Vec<String>,
    pub excluded_sessions: Vec<String>,
    pub pending_events: u32,
    pub admitted_occurrences: u32,
    pub diagnostic: Option<String>,
    pub supported: bool,
}
#[derive(Debug, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct AgentCaptureUpdate {
    pub enabled: bool,
    pub capture_subagents: bool,
    pub excluded_paths: Vec<String>,
    pub excluded_sessions: Vec<String>,
}
pub(crate) fn project_id(root: &Path) -> String {
    crate::reader::worktree_project_id(root)
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AgentIntegrationKind {
    ClaudeHooks,
    ClaudeMcp,
    CodexHooks,
    CodexMcp,
}
#[derive(Debug, Deserialize, Type)]
#[serde(deny_unknown_fields)]
pub struct AgentIntegrationSelection {
    pub kind: AgentIntegrationKind,
    pub client_version: String,
    pub compatibility_and_trust_confirmed: bool,
    pub source_ids: Vec<String>,
}
#[derive(Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentIntegrationSummary {
    pub preparation_id: String,
    pub kind: String,
    pub config_path: String,
    pub state: String,
    pub capabilities_verified: bool,
}
#[derive(Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentIntegrationReview {
    pub preparation_id: String,
    pub config_path: String,
    pub policy_path: String,
    pub before_text: Option<String>,
    pub after_text: String,
    pub capabilities_verified: bool,
}
#[derive(Debug, Serialize, Type)]
pub struct AgentSourceChoice {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Serialize, Type)]
pub struct AgentIntegrationsState {
    pub installations: Vec<AgentIntegrationSummary>,
    pub sources: Vec<AgentSourceChoice>,
}

#[tauri::command]
#[specta::specta]
pub async fn agent_integrations_state(
    expected_root: String,
    workspace: State<'_, WorkspaceState>,
    reader: State<'_, ReaderBackend>,
) -> Result<AgentIntegrationsState, String> {
    let epoch = checked_epoch(&workspace, &expected_root)?;
    let installations = reader
        .invoke(Request::ListIntegrations {
            project_id: project_id(&epoch.root),
        })
        .await?;
    let sources: Vec<mdreader_core::Source> = reader.invoke(Request::ListSources).await?;
    let sources = sources
        .into_iter()
        .filter(|s| s.enabled)
        .map(|s| AgentSourceChoice {
            id: s.id,
            name: s.name,
        })
        .collect();
    epoch
        .finish(
            &workspace,
            AgentIntegrationsState {
                installations,
                sources,
            },
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn prepare_agent_integration(
    expected_root: String,
    selection: AgentIntegrationSelection,
    workspace: State<'_, WorkspaceState>,
    reader: State<'_, ReaderBackend>,
) -> Result<AgentIntegrationReview, String> {
    let epoch = checked_epoch(&workspace, &expected_root)?;
    if !cfg!(all(unix, not(mobile))) || !selection.compatibility_and_trust_confirmed {
        return Err(
            "Confirm client compatibility and project trust before reviewing an integration".into(),
        );
    }
    use mdreader_service::integrations::{ClientCapabilities, Kind, Selection};
    let kind = match selection.kind {
        AgentIntegrationKind::ClaudeHooks => Kind::ClaudeHooks,
        AgentIntegrationKind::ClaudeMcp => Kind::ClaudeMcp,
        AgentIntegrationKind::CodexHooks => Kind::CodexHooks,
        AgentIntegrationKind::CodexMcp => Kind::CodexMcp,
    };
    let id = project_id(&epoch.root);
    let policy: Option<ProjectPolicy> = reader
        .invoke(Request::CapturePolicy {
            project_id: id.clone(),
        })
        .await?;
    epoch.finish(&workspace, ()).map_err(|e| e.to_string())?;
    if policy.is_none() {
        // Reviewing an MCP grant creates a disabled policy, never capture opt-in.
        if matches!(kind, Kind::ClaudeHooks | Kind::CodexHooks) {
            return Err("Enable capture for this folder before installing its hooks".into());
        }
        let _: serde_json::Value = reader
            .invoke(Request::ConfigureCapture {
                policy: ProjectPolicy {
                    enabled: false,
                    project_id: id.clone(),
                    project_root: epoch.root.clone(),
                    repository_root: epoch.root.clone(),
                    worktree_roots: vec![epoch.root.clone()],
                    capture_subagents: false,
                    excluded_paths: vec![],
                    excluded_sessions: vec![],
                    selected_backfill_sessions: vec![],
                },
            })
            .await?;
    }
    epoch.finish(&workspace, ()).map_err(|e| e.to_string())?;
    let capabilities = ClientCapabilities {
        local_commands: true,
        claude_exec_hooks: matches!(kind, Kind::ClaudeHooks),
        codex_lifecycle_hooks: matches!(kind, Kind::CodexHooks),
        codex_notify: false,
        client_version: selection.client_version,
    };
    let review = reader
        .invoke(Request::PrepareIntegration {
            selection: Selection {
                kind,
                project_id: id,
                worktree_root: epoch.root.clone(),
                executable: None,
                source_ids: selection.source_ids,
                capabilities,
            },
        })
        .await?;
    epoch.finish(&workspace, review).map_err(|e| e.to_string())
}
async fn checked_installation(
    reader: &ReaderBackend,
    workspace: &WorkspaceState,
    expected_root: &str,
    id: &str,
) -> Result<(), String> {
    let epoch = checked_epoch(workspace, expected_root)?;
    let entries: Vec<AgentIntegrationSummary> = reader
        .invoke(Request::ListIntegrations {
            project_id: project_id(&epoch.root),
        })
        .await?;
    epoch.finish(workspace, ()).map_err(|e| e.to_string())?;
    if !entries.iter().any(|e| e.preparation_id == id) {
        return Err("This integration does not belong to the selected folder".into());
    }
    Ok(())
}
#[tauri::command]
#[specta::specta]
pub async fn apply_agent_integration(
    expected_root: String,
    preparation_id: String,
    workspace: State<'_, WorkspaceState>,
    reader: State<'_, ReaderBackend>,
) -> Result<(), String> {
    checked_installation(&reader, &workspace, &expected_root, &preparation_id).await?;
    let _: serde_json::Value = reader
        .invoke(Request::ApplyIntegration { preparation_id })
        .await?;
    checked_epoch(&workspace, &expected_root)?
        .finish(&workspace, ())
        .map_err(|e| e.to_string())
}
#[tauri::command]
#[specta::specta]
pub async fn uninstall_agent_integration(
    expected_root: String,
    preparation_id: String,
    workspace: State<'_, WorkspaceState>,
    reader: State<'_, ReaderBackend>,
) -> Result<(), String> {
    checked_installation(&reader, &workspace, &expected_root, &preparation_id).await?;
    let _: serde_json::Value = reader
        .invoke(Request::UninstallIntegration { preparation_id })
        .await?;
    checked_epoch(&workspace, &expected_root)?
        .finish(&workspace, ())
        .map_err(|e| e.to_string())
}
pub(crate) fn checked_epoch(
    workspace: &WorkspaceState,
    expected_root: &str,
) -> Result<std::sync::Arc<crate::engine::Epoch>, String> {
    let epoch = workspace.current().ok_or("Open a local folder first")?;
    epoch.validate_root()?;
    if epoch.root != Path::new(expected_root) {
        return Err("The selected folder changed; reopen Agent capture settings".into());
    }
    Ok(epoch)
}
async fn state(
    reader: &ReaderBackend,
    workspace: &WorkspaceState,
    expected_root: &str,
) -> Result<AgentCaptureState, String> {
    let epoch = checked_epoch(workspace, expected_root)?;
    let policy: Option<ProjectPolicy> = reader
        .invoke(Request::CapturePolicy {
            project_id: project_id(&epoch.root),
        })
        .await?;
    let status: mdreader_service::capture::CaptureStatus =
        reader.invoke(Request::CaptureStatus).await?;
    let result = AgentCaptureState {
        root: epoch.root.to_string_lossy().into_owned(),
        enabled: policy.as_ref().is_some_and(|p| p.enabled),
        capture_subagents: policy.as_ref().is_some_and(|p| p.capture_subagents),
        excluded_paths: policy
            .as_ref()
            .map(|p| {
                p.excluded_paths
                    .iter()
                    .map(|v| v.to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default(),
        excluded_sessions: policy.map(|p| p.excluded_sessions).unwrap_or_default(),
        pending_events: status.pending_events.try_into().unwrap_or(u32::MAX),
        admitted_occurrences: status.admitted_occurrences.try_into().unwrap_or(u32::MAX),
        diagnostic: status.diagnostic,
        supported: cfg!(all(unix, not(mobile))),
    };
    epoch.finish(workspace, result).map_err(|e| e.to_string())
}
#[tauri::command]
#[specta::specta]
pub async fn agent_capture_state(
    expected_root: String,
    workspace: State<'_, WorkspaceState>,
    reader: State<'_, ReaderBackend>,
) -> Result<AgentCaptureState, String> {
    state(&reader, &workspace, &expected_root).await
}
#[tauri::command]
#[specta::specta]
pub async fn set_agent_capture(
    expected_root: String,
    update: AgentCaptureUpdate,
    workspace: State<'_, WorkspaceState>,
    reader: State<'_, ReaderBackend>,
) -> Result<AgentCaptureState, String> {
    if !cfg!(all(unix, not(mobile))) {
        return Err("Local agent capture is not supported on this platform yet".into());
    }
    let epoch = checked_epoch(&workspace, &expected_root)?;
    let id = project_id(&epoch.root);
    let previous: Option<ProjectPolicy> = reader
        .invoke(Request::CapturePolicy {
            project_id: id.clone(),
        })
        .await?;
    epoch.finish(&workspace, ()).map_err(|e| e.to_string())?;
    let policy = ProjectPolicy {
        project_id: id,
        project_root: epoch.root.clone(),
        repository_root: epoch.root.clone(),
        worktree_roots: vec![epoch.root.clone()],
        enabled: update.enabled,
        capture_subagents: update.capture_subagents,
        excluded_paths: update.excluded_paths.into_iter().map(Into::into).collect(),
        excluded_sessions: update.excluded_sessions,
        selected_backfill_sessions: previous
            .map(|p| p.selected_backfill_sessions)
            .unwrap_or_default(),
    };
    policy
        .validate()
        .map_err(|_| "Capture exclusions or project settings are invalid")?;
    let _: serde_json::Value = reader.invoke(Request::ConfigureCapture { policy }).await?;
    epoch.finish(&workspace, ()).map_err(|e| e.to_string())?;
    state(&reader, &workspace, &expected_root).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folder_switch_rejects_stale_capture_controls_and_ids_do_not_admit_siblings() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let workspace = WorkspaceState::default();
        let root = first.path().canonicalize().unwrap();
        workspace.switch(root.clone()).unwrap();
        assert!(checked_epoch(&workspace, root.to_str().unwrap()).is_ok());
        workspace
            .switch(second.path().canonicalize().unwrap())
            .unwrap();
        assert!(checked_epoch(&workspace, root.to_str().unwrap()).is_err());
        assert_ne!(project_id(&root), project_id(second.path()));
    }
}
