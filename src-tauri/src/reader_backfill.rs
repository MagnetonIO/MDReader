//! Thin desktop-only selected-output IPC. File selection occurs explicitly in
//! the client; this adapter never reads agent directories or transcript paths.
use crate::{
    commands::WorkspaceState,
    reader::ReaderBackend,
    reader_agents::{checked_epoch, project_id},
};
use mdreader_service::{
    Request,
    capture::backfill::{BackfillReply, BackfillRequest, BackfillReview},
};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

#[derive(Clone, Copy, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AgentBackfillAdapter {
    ClaudeCode,
    CodexHook,
    CodexNotify,
}
impl From<AgentBackfillAdapter> for mdreader_capture::Adapter {
    fn from(value: AgentBackfillAdapter) -> Self {
        match value {
            AgentBackfillAdapter::ClaudeCode => Self::ClaudeCode,
            AgentBackfillAdapter::CodexHook => Self::CodexHook,
            AgentBackfillAdapter::CodexNotify => Self::CodexNotify,
        }
    }
}
#[derive(Deserialize, Type)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentBackfillRequest {
    State,
    Review {
        adapter: AgentBackfillAdapter,
        event: String,
    },
    Enqueue {
        adapter: AgentBackfillAdapter,
        event: String,
        review_digest: String,
        session_id: String,
    },
    Deselect {
        session_id: String,
    },
}
impl From<AgentBackfillRequest> for BackfillRequest {
    fn from(value: AgentBackfillRequest) -> Self {
        match value {
            AgentBackfillRequest::State => Self::State,
            AgentBackfillRequest::Review { adapter, event } => Self::Review {
                adapter: adapter.into(),
                event,
            },
            AgentBackfillRequest::Enqueue {
                adapter,
                event,
                review_digest,
                session_id,
            } => Self::Enqueue {
                adapter: adapter.into(),
                event,
                review_digest,
                session_id,
            },
            AgentBackfillRequest::Deselect { session_id } => Self::Deselect { session_id },
        }
    }
}
#[derive(Serialize, Type)]
pub struct AgentBackfillReview {
    pub canonical_event: String,
    pub review_digest: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub turn_id_kind: Option<String>,
    pub subagent_id: Option<String>,
    pub cwd: String,
    pub body: String,
    pub selected: bool,
}
impl From<BackfillReview> for AgentBackfillReview {
    fn from(value: BackfillReview) -> Self {
        Self {
            canonical_event: value.canonical_event,
            review_digest: value.review_digest,
            session_id: value.session_id,
            turn_id: value.turn_id,
            turn_id_kind: value.turn_id_kind,
            subagent_id: value.subagent_id,
            cwd: value.cwd,
            body: value.body,
            selected: value.selected,
        }
    }
}
#[derive(Serialize, Type)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AgentBackfillOutcome {
    Queued { id: String },
    Duplicate { id: String },
    Skipped { reason: String },
}
#[derive(Serialize, Type)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum AgentBackfillReply {
    State {
        enabled: bool,
        selected_sessions: Vec<String>,
    },
    Reviewed {
        value: AgentBackfillReview,
    },
    Enqueued {
        outcome: AgentBackfillOutcome,
    },
    Deselected {
        session_id: String,
    },
}
impl From<BackfillReply> for AgentBackfillReply {
    fn from(value: BackfillReply) -> Self {
        match value {
            BackfillReply::State {
                enabled,
                selected_sessions,
            } => Self::State {
                enabled,
                selected_sessions,
            },
            BackfillReply::Reviewed { value } => Self::Reviewed {
                value: value.into(),
            },
            BackfillReply::Enqueued { outcome } => Self::Enqueued {
                outcome: match outcome {
                    mdreader_capture::EnqueueOutcome::Queued { id } => {
                        AgentBackfillOutcome::Queued { id }
                    }
                    mdreader_capture::EnqueueOutcome::Duplicate { id } => {
                        AgentBackfillOutcome::Duplicate { id }
                    }
                    mdreader_capture::EnqueueOutcome::Skipped { reason } => {
                        AgentBackfillOutcome::Skipped {
                            reason: format!("{reason:?}"),
                        }
                    }
                },
            },
            BackfillReply::Deselected { session_id } => Self::Deselected { session_id },
        }
    }
}
#[tauri::command]
#[specta::specta]
pub async fn agent_capture_backfill(
    expected_root: String,
    request: AgentBackfillRequest,
    workspace: State<'_, WorkspaceState>,
    reader: State<'_, ReaderBackend>,
) -> Result<AgentBackfillReply, String> {
    if !cfg!(all(unix, not(mobile))) {
        return Err(
            "Selected output backfill requires the supported local desktop service.".into(),
        );
    }
    if matches!(&request, AgentBackfillRequest::Review { event, .. } | AgentBackfillRequest::Enqueue { event, .. } if event.len() > 256 * 1024)
    {
        return Err("Choose one final-output JSON event of at most 256 KiB.".into());
    }
    let metadata_only = matches!(
        request,
        AgentBackfillRequest::State | AgentBackfillRequest::Deselect { .. }
    );
    let epoch = selected_epoch(&workspace, &expected_root, metadata_only)?;
    let reply: BackfillReply = reader.invoke(Request::CaptureBackfill {
        project_id: project_id(&epoch.root), request: request.into(),
    }).await.map_err(|error| {
        // Service errors are fixed domain diagnostics. Never append input JSON,
        // filesystem errors, or private paths from a selected file.
        if error.starts_with("Invalid request:") { error }
        else { "Backfill was not confirmed. The same reviewed event can be retried explicitly; it may already be queued. If capture settings changed, choose and review the event again.".into() }
    })?;
    finish(&workspace, &epoch, metadata_only, reply.into())
}

// Metadata-only selection revocation must not depend on the original directory
// remaining readable. Current Arc + exact expected path fences the UI context;
// these requests cannot read content, enqueue, or grant filesystem authority.
fn selected_epoch(
    workspace: &WorkspaceState,
    expected_root: &str,
    metadata_only: bool,
) -> Result<std::sync::Arc<crate::engine::Epoch>, String> {
    if !metadata_only {
        return checked_epoch(workspace, expected_root);
    }
    let epoch = workspace.current().ok_or("Open a local folder first")?;
    if epoch.root != std::path::Path::new(expected_root) {
        return Err("The selected folder changed; reopen backfill settings.".into());
    }
    Ok(epoch)
}
fn finish<T>(
    workspace: &WorkspaceState,
    epoch: &std::sync::Arc<crate::engine::Epoch>,
    metadata_only: bool,
    value: T,
) -> Result<T, String> {
    if metadata_only && workspace.is_current(epoch) {
        return Ok(value);
    }
    epoch.finish(workspace, value).map_err(|_| "The selected folder changed. Backfill may already be queued; reopen its original folder to reconcile.".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_root_allows_only_metadata_revocation_and_folder_switch_still_fences_reply() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let first = first.canonicalize().unwrap();
        let workspace = WorkspaceState::default();
        workspace.switch(first.clone()).unwrap();
        std::fs::rename(&first, temp.path().join("missing")).unwrap();
        let epoch = selected_epoch(&workspace, first.to_str().unwrap(), true).unwrap();
        assert!(selected_epoch(&workspace, first.to_str().unwrap(), false).is_err());
        assert!(finish(&workspace, &epoch, true, ()).is_ok());
        workspace.switch(second.canonicalize().unwrap()).unwrap();
        assert!(finish(&workspace, &epoch, true, ()).is_err());
        assert!(selected_epoch(&workspace, first.to_str().unwrap(), true).is_err());
    }
}
