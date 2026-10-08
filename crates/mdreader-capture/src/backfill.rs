//! Explicit single-event review. Never discovers or reads agent histories.
use crate::{Adapter, CaptureError, CaptureOrigin, Limits, ProjectPolicy, Result, adapt};
use serde::Serialize;
use serde_json::{Map, Value};

/// Only parsed visible fields are returned; unknown input fields never survive
/// review. Missing provenance remains absent rather than being reconstructed.
#[derive(Serialize)]
pub struct VisibleBackfill {
    pub canonical_event: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub turn_id_kind: Option<String>,
    pub subagent_id: Option<String>,
    pub cwd: String,
    pub body: String,
}

pub fn inspect(adapter: Adapter, bytes: &[u8], policy: &ProjectPolicy) -> Result<VisibleBackfill> {
    if bytes.len() > Limits::default().max_event_bytes {
        return Err(CaptureError::TooLarge);
    }
    let output = adapt(adapter, bytes)?;
    // Review checks the current grant/exclusions without authorizing this
    // session for backfill yet. Enqueue separately requires explicit selection.
    let roots = policy.authorize(
        &output.cwd,
        &output.session,
        output.subagent.as_deref(),
        CaptureOrigin::Live,
    )?;
    let cwd = roots.cwd.to_str().ok_or(CaptureError::Invalid)?.to_string();
    let mut fields = Map::new();
    fields.insert("cwd".into(), Value::String(cwd.clone()));
    if adapter == Adapter::CodexNotify {
        fields.insert("type".into(), Value::String(output.event));
        fields.insert("thread-id".into(), Value::String(output.session.clone()));
        fields.insert(
            "last-assistant-message".into(),
            Value::String(output.body.clone()),
        );
        if let Some(turn) = &output.turn {
            fields.insert("turn-id".into(), Value::String(turn.clone()));
        }
    } else {
        fields.insert("hook_event_name".into(), Value::String(output.event));
        fields.insert("session_id".into(), Value::String(output.session.clone()));
        fields.insert(
            "last_assistant_message".into(),
            Value::String(output.body.clone()),
        );
        if let Some(turn) = &output.turn {
            fields.insert(
                if adapter == Adapter::ClaudeCode {
                    "prompt_id"
                } else {
                    "turn_id"
                }
                .into(),
                Value::String(turn.clone()),
            );
        }
        if let Some(agent) = &output.subagent {
            fields.insert("agent_id".into(), Value::String(agent.clone()));
        }
        if let Some(kind) = &output.agent_type {
            fields.insert("agent_type".into(), Value::String(kind.clone()));
        }
    }
    let canonical_event = serde_json::to_string(&fields).map_err(|_| CaptureError::Invalid)?;
    if canonical_event.len() > Limits::default().max_event_bytes {
        return Err(CaptureError::TooLarge);
    }
    Ok(VisibleBackfill {
        canonical_event,
        session_id: output.session,
        turn_id: output.turn,
        turn_id_kind: output.turn_kind,
        subagent_id: output.subagent,
        cwd,
        body: output.body,
    })
}

/// Bind review to the exact visible event, chosen adapter and retained service
/// grant. The caller supplies a serialized grant with only its selected-session
/// list omitted, so an uncertain enqueue can be replayed without a new identity.
pub fn review_digest(adapter: Adapter, event: &str, authority: &[u8]) -> Result<String> {
    if event.len() > Limits::default().max_event_bytes || authority.len() > 256 * 1024 {
        return Err(CaptureError::TooLarge);
    }
    let bytes =
        serde_json::to_vec(&(adapter, event, authority)).map_err(|_| CaptureError::Invalid)?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"mdreader:selected-backfill-review:v1\0");
    hash.update(&bytes);
    Ok(hash.finalize().to_hex().to_string())
}
