//! Session/file metadata hooks are separate from response capture.
use crate::{
    InstallRequest, IntegrationError as Error, IntegrationKind, Result,
    config::{Owned, path_string, quote_command},
};
use serde_json::json;

pub(crate) fn spec(request: &InstallRequest) -> Result<Vec<Owned>> {
    let grant = request.mcp_grant_id.as_deref().ok_or(Error::Invalid)?;
    if grant.len() != 36 || !grant.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-') {
        return Err(Error::Invalid);
    }
    let agent = if request.kind == IntegrationKind::CodexSessionHooks {
        "codex"
    } else {
        "claude"
    };
    let args = [
        path_string(&request.executable)?,
        "--mdreader-agent",
        "session-hook",
        "--agent",
        agent,
        "--grant",
        grant,
        "--data-dir",
        path_string(&request.data_dir)?,
    ];
    let command = args
        .into_iter()
        .map(quote_command)
        .collect::<Result<Vec<_>>>()?
        .join(" ");
    ["SessionStart", "SessionEnd", "PostToolUse"].into_iter().map(|event| {
        let group = if event == "PostToolUse" {
            // Only recognized file tools are attributed; watching indexes other changes.
            json!({"matcher": if agent == "claude" { "Write|Edit|MultiEdit" } else { "apply_patch|write_file|edit_file" }, "hooks":[{"type":"command","command":command,"timeout":3}]})
        } else { json!({"hooks":[{"type":"command","command":command,"timeout":3}]}) };
        Ok(Owned::Json { path: vec!["hooks".into(), event.into()], array:true, raw:serde_json::to_string(&group).map_err(|_|Error::Invalid)? })
    }).collect()
}
