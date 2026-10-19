//! Metadata hooks have separate receipts and never enable response capture.
use super::*;
fn supported(record: &Record) -> bool {
    let version = record
        .client_version
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .find(|s| s.contains('.'))
        .unwrap_or("");
    let mut values = version.split('.').filter_map(|v| v.parse::<u64>().ok());
    let (major, minor) = (values.next().unwrap_or(0), values.next().unwrap_or(0));
    match record.selection.agent {
        Agent::Claude => major >= 2,
        Agent::Codex => major >= 1 || (major == 0 && minor >= 162),
    }
}
fn hook_request(directory: &Path, record: &Record) -> Result<Option<InstallRequest>> {
    if !supported(record) {
        return Ok(None);
    }
    let mut request = request(directory, record)?;
    request.kind = match (record.selection.agent, record.selection.scope) {
        (Agent::Claude, Scope::Project) => IntegrationKind::ClaudeSessionHooks,
        (Agent::Claude, Scope::User) => IntegrationKind::ClaudeUserSessionHooks,
        (Agent::Codex, _) => IntegrationKind::CodexSessionHooks,
    };
    request.capabilities.claude_exec_hooks = record.selection.agent == Agent::Claude;
    request.capabilities.codex_lifecycle_hooks = record.selection.agent == Agent::Codex;
    Ok(Some(request))
}
pub(super) fn install(directory: &Path, record: &Record) -> Result<()> {
    let Some(request) = hook_request(directory, record)? else {
        return Ok(());
    };
    install_config(
        &mut installer(directory)?,
        &request,
        &policy(&record.selection)?,
    )
}
pub(super) fn uninstall(directory: &Path, record: &Record) -> Result<()> {
    if let Some(request) = hook_request(directory, record)? {
        installer(directory)?
            .uninstall(&request)
            .map_err(integration_error)?;
    }
    Ok(())
}
