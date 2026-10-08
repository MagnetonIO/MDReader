//! Explicit selected-event backfill over existing retained grants. This module
//! never discovers histories, grants folders, or reconstructs missing authorship.
use super::{Grant, artifact_matches, invalid, policies, write_policy};
use mdreader_capture::{
    Adapter, CaptureError, CaptureOrigin, EnqueueOutcome, Limits, ProjectPolicy, Spool,
};
use mdreader_core::{ReaderError, ReaderStore, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackfillRequest {
    State,
    Review {
        adapter: Adapter,
        event: String,
    },
    Enqueue {
        adapter: Adapter,
        event: String,
        review_digest: String,
        session_id: String,
    },
    Deselect {
        session_id: String,
    },
}
// Request's parent Debug must never expose selected JSON or visible output.
impl std::fmt::Debug for BackfillRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::State => "BackfillState",
            Self::Review { .. } => "BackfillReview(redacted)",
            Self::Enqueue { .. } => "BackfillEnqueue(redacted)",
            Self::Deselect { .. } => "BackfillDeselect(redacted)",
        })
    }
}
impl BackfillRequest {
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Enqueue { .. } | Self::Deselect { .. })
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct BackfillReview {
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
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum BackfillReply {
    State {
        enabled: bool,
        selected_sessions: Vec<String>,
    },
    Reviewed {
        value: BackfillReview,
    },
    Enqueued {
        outcome: EnqueueOutcome,
    },
    Deselected {
        session_id: String,
    },
}
fn capture_error(error: CaptureError) -> ReaderError {
    let message = match error {
        CaptureError::Busy => "Capture queue is busy; explicitly retry this same reviewed output.",
        CaptureError::QueueFull => "Capture queue is full; the output was not confirmed queued.",
        CaptureError::TooLarge => "The selected output exceeds the 256 KiB bound.",
        CaptureError::Excluded(reason) => {
            return ReaderError::Invalid(format!("Selected output is excluded: {reason:?}"));
        }
        _ => "The selected visible-output event or capture queue is unavailable.",
    };
    ReaderError::Invalid(message.into())
}
fn grant(store: &ReaderStore, project_id: &str) -> Result<Option<Grant>> {
    if project_id.is_empty() || project_id.len() > 256 {
        return Err(invalid());
    }
    Ok(policies(store)?
        .into_iter()
        .find(|grant| grant.policy.project_id == project_id))
}
fn verify_authority(store: &ReaderStore, grant: &Grant) -> Result<()> {
    if !grant.policy.enabled {
        return Err(ReaderError::Invalid(
            "Enable capture for this project before selecting past outputs.".into(),
        ));
    }
    if grant.roots.is_empty() {
        return Err(invalid());
    }
    for root in &grant.roots {
        root.restore()?.verify()?;
    }
    grant
        .projection_root
        .as_ref()
        .ok_or_else(invalid)?
        .restore()?
        .verify()?;
    store.index_status(&[grant.source_id.as_ref().ok_or_else(invalid)?.clone()])?;
    Ok(())
}
fn verify(store: &ReaderStore, directory: &Path, grant: &Grant) -> Result<()> {
    verify_review(store, directory, grant, None)
}
fn verify_review(
    store: &ReaderStore,
    directory: &Path,
    grant: &Grant,
    candidate: Option<&ProjectPolicy>,
) -> Result<()> {
    verify_authority(store, grant)?;
    artifact_matches(directory, &grant.policy).or_else(|error| {
        // The only repairable mismatch is this explicitly confirmed session's
        // exact next policy. Unrelated selection/security changes stay closed.
        candidate.map_or(Err(error), |policy| artifact_matches(directory, policy))
    })
}
fn authority(grant: &Grant) -> Result<Vec<u8>> {
    let mut value = serde_json::to_value(grant).map_err(|_| invalid())?;
    value
        .get_mut("policy")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(invalid)?
        .remove("selected_backfill_sessions");
    serde_json::to_vec(&value).map_err(|_| invalid())
}
fn review(
    store: &ReaderStore,
    directory: &Path,
    grant: &Grant,
    adapter: Adapter,
    event: &str,
    candidate: Option<&ProjectPolicy>,
) -> Result<BackfillReview> {
    verify_review(store, directory, grant, candidate).map_err(|_| ReaderError::Unavailable)?;
    let visible = mdreader_capture::backfill::inspect(adapter, event.as_bytes(), &grant.policy)
        .map_err(capture_error)?;
    let digest = mdreader_capture::backfill::review_digest(
        adapter,
        &visible.canonical_event,
        &authority(grant)?,
    )
    .map_err(capture_error)?;
    verify_review(store, directory, grant, candidate).map_err(|_| ReaderError::Unavailable)?;
    Ok(BackfillReview {
        selected: grant
            .policy
            .selected_backfill_sessions
            .contains(&visible.session_id),
        canonical_event: visible.canonical_event,
        review_digest: digest,
        session_id: visible.session_id,
        turn_id: visible.turn_id,
        turn_id_kind: visible.turn_id_kind,
        subagent_id: visible.subagent_id,
        cwd: visible.cwd,
        body: visible.body,
    })
}
/// Change only the selected-session list. Never call configure, which captures
/// new root identities. Publish artifact first so a failed DB save fails closed.
fn persist_selection(store: &ReaderStore, directory: &Path, grant: &Grant) -> Result<()> {
    grant.policy.validate().map_err(capture_error)?;
    if serde_json::to_vec(&grant.policy)
        .map_err(|_| invalid())?
        .len()
        > 64 * 1024
    {
        return Err(invalid());
    }
    let json = serde_json::to_string(grant).map_err(|_| invalid())?;
    write_policy(directory, &grant.policy)?;
    store.save_capture_policy(&grant.policy.project_id, &json)
}
/// Called by the existing desktop service owner, never by a renderer writer.
pub fn execute(
    store: &mut ReaderStore,
    directory: &Path,
    project_id: &str,
    request: BackfillRequest,
) -> Result<BackfillReply> {
    let current = grant(store, project_id)?;
    if matches!(request, BackfillRequest::State) {
        return Ok(BackfillReply::State {
            enabled: current.as_ref().is_some_and(|grant| grant.policy.enabled),
            selected_sessions: current
                .map(|grant| grant.policy.selected_backfill_sessions)
                .unwrap_or_default(),
        });
    }
    let mut current = current.ok_or_else(invalid)?;
    match request {
        BackfillRequest::Review { adapter, event } => Ok(BackfillReply::Reviewed {
            value: review(store, directory, &current, adapter, &event, None)?,
        }),
        BackfillRequest::Enqueue {
            adapter,
            event,
            review_digest,
            session_id,
        } => {
            if session_id.is_empty() || session_id.len() > 512 {
                return Err(invalid());
            }
            let mut candidate = current.policy.clone();
            let selecting = !candidate.selected_backfill_sessions.contains(&session_id);
            if selecting {
                candidate
                    .selected_backfill_sessions
                    .push(session_id.clone());
            }
            candidate.validate().map_err(capture_error)?;
            let inspected = review(
                store,
                directory,
                &current,
                adapter,
                &event,
                Some(&candidate),
            )?;
            if inspected.session_id != session_id || inspected.review_digest != review_digest {
                return Err(ReaderError::Conflict);
            }
            if selecting {
                current.policy = candidate;
                persist_selection(store, directory, &current)?;
            }
            verify(store, directory, &current).map_err(|_| ReaderError::Unavailable)?;
            let spool = Spool::open(directory.join("captures"), Limits::default())
                .map_err(capture_error)?;
            let outcome = spool
                .enqueue(
                    adapter,
                    inspected.canonical_event.as_bytes(),
                    &current.policy,
                    CaptureOrigin::SelectedBackfill,
                )
                .map_err(capture_error)?;
            Ok(BackfillReply::Enqueued { outcome })
        }
        BackfillRequest::Deselect { session_id } => {
            if session_id.is_empty() || session_id.len() > 512 {
                return Err(invalid());
            }
            // Narrowing remains possible for a revoked/replaced/missing project;
            // preserved identities still gate every projection and future read.
            current
                .policy
                .selected_backfill_sessions
                .retain(|session| session != &session_id);
            persist_selection(store, directory, &current)?;
            Ok(BackfillReply::Deselected { session_id })
        }
        BackfillRequest::State => Err(invalid()),
    }
}
