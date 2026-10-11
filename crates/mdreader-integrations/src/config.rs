use crate::{
    json, Capabilities, InstallRequest, IntegrationError as Error, IntegrationKind, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;
use toml_edit::{DocumentMut, Item};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) enum Owned {
    Json {
        path: Vec<String>,
        array: bool,
        raw: String,
    },
    Toml {
        path: Vec<String>,
        fragment: String,
    },
}
fn string(path: &Path) -> Result<&str> {
    let value = path.to_str().ok_or(Error::Invalid)?;
    if !path.is_absolute() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(Error::Invalid);
    }
    // Claude substitutes placeholders before exec; no user path may be interpreted as one.
    if value.contains("${") {
        return Err(Error::Unsupported);
    }
    Ok(value)
}
pub(crate) fn path_string(path: &Path) -> Result<&str> {
    string(path)
}
/// POSIX shell quoting, never used on Windows/PowerShell hosts.
#[cfg(not(windows))]
pub fn quote_posix(value: &str) -> Result<String> {
    if value.is_empty() || value.chars().any(char::is_control) || value.contains("${") {
        return Err(Error::Invalid);
    }
    Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
}

#[cfg(not(windows))]
fn quote_command(value: &str) -> Result<String> {
    quote_posix(value)
}
#[cfg(windows)]
fn quote_command(value: &str) -> Result<String> {
    // Codex's Windows command-hook runner uses cmd.exe /C. Quote each argument,
    // refusing expansion and shell metacharacters (also with delayed expansion).
    // Structured argv integrations do not need this restriction.
    if value.is_empty()
        || value.chars().any(char::is_control)
        || value.contains(['"', '%', '!', '^', '&', '|', '<', '>', '(', ')', '$', '`'])
    {
        return Err(Error::Unsupported);
    }
    let trailing = value.len() - value.trim_end_matches('\\').len();
    Ok(format!("\"{}{}\"", value, "\\".repeat(trailing)))
}

pub(crate) fn validate_capability(kind: IntegrationKind, cap: &Capabilities) -> Result<()> {
    if !cap.local_commands
        || cap.client_version.is_empty()
        || cap.client_version.len() > 128
        || cap.client_version.chars().any(char::is_control)
    {
        return Err(Error::Unsupported);
    }
    if match kind {
        IntegrationKind::ClaudeHooks => !cap.claude_exec_hooks,
        IntegrationKind::CodexHooks => !cap.codex_lifecycle_hooks,
        IntegrationKind::CodexNotify => !cap.codex_notify,
        _ => false,
    } {
        return Err(Error::Unsupported);
    }
    Ok(())
}
pub(crate) fn spec(
    request: &InstallRequest,
    policy_path: &Path,
    capture_subagents: bool,
) -> Result<Vec<Owned>> {
    validate_capability(request.kind, &request.capabilities)?;
    let executable = string(&request.executable)?;
    let data = string(&request.data_dir)?;
    let policy = string(policy_path)?;
    if matches!(
        request.kind,
        IntegrationKind::ClaudeMcp | IntegrationKind::ClaudeUserMcp | IntegrationKind::CodexMcp
    ) && ((request.source_ids.is_empty() && request.mcp_grant_id.is_none())
        || request.source_ids.len() > 128
        || request.source_ids.iter().any(|id| {
            id.is_empty()
                || id.len() > 256
                || !id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-:.".contains(&c))
        }))
    {
        return Err(Error::Invalid);
    }
    let mut source_ids = request.source_ids.clone();
    source_ids.sort();
    source_ids.dedup();
    if source_ids.len() != request.source_ids.len() {
        return Err(Error::Invalid);
    }
    let spool = string(&request.spool_root)?;
    match request.kind {
        IntegrationKind::ClaudeHooks | IntegrationKind::CodexHooks => {
            let agent = if request.kind == IntegrationKind::ClaudeHooks {
                "claude"
            } else {
                "codex"
            };
            let args = ["--mdreader-capture", "hook", agent, policy, spool];
            let handler = if agent == "claude" {
                json!({"type":"command","command":executable,"args":args,"timeout":3})
            } else {
                let mut argv = vec![quote_command(executable)?];
                for arg in args {
                    argv.push(quote_command(arg)?);
                }
                json!({"type":"command","command":argv.join(" "),"timeout":3})
            };
            let raw =
                serde_json::to_string(&json!({"hooks":[handler]})).map_err(|_| Error::Invalid)?;
            let events = if capture_subagents {
                vec!["Stop", "SubagentStop"]
            } else {
                vec!["Stop"]
            };
            Ok(events
                .into_iter()
                .map(|event| Owned::Json {
                    path: vec!["hooks".into(), event.into()],
                    array: true,
                    raw: raw.clone(),
                })
                .collect())
        }
        IntegrationKind::ClaudeMcp | IntegrationKind::ClaudeUserMcp | IntegrationKind::CodexMcp => {
            let mut args = vec![
                "--mdreader-mcp".to_owned(),
                "--data-dir".into(),
                data.into(),
            ];
            if let Some(grant) = &request.mcp_grant_id {
                if grant.len() != 36 || !grant.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-') {
                    return Err(Error::Invalid);
                }
                args.push("--grant".into());
                args.push(grant.clone());
            }
            for source in source_ids {
                args.push("--source".into());
                args.push(source);
            }
            if matches!(
                request.kind,
                IntegrationKind::ClaudeMcp | IntegrationKind::ClaudeUserMcp
            ) {
                let raw = serde_json::to_string(
                    &json!({"type":"stdio","command":executable,"args":args}),
                )
                .map_err(|_| Error::Invalid)?;
                Ok(vec![Owned::Json {
                    path: vec!["mcpServers".into(), "mdreader".into()],
                    array: false,
                    raw,
                }])
            } else {
                // JSON strings/arrays use the compatible TOML basic-string subset here.
                let command = serde_json::to_string(executable).map_err(|_| Error::Invalid)?;
                let args = serde_json::to_string(&args).map_err(|_| Error::Invalid)?;
                Ok(vec![Owned::Toml {
                    path: vec!["mcp_servers".into(), "mdreader".into()],
                    fragment: format!(
                        "\n[mcp_servers.mdreader]\ncommand = {command}\nargs = {args}\n"
                    ),
                }])
            }
        }
        IntegrationKind::CodexNotify => {
            let args = [
                executable,
                "--mdreader-capture",
                "hook",
                "codex-notify",
                policy,
                spool,
            ];
            Ok(vec![Owned::Toml {
                path: vec!["notify".into()],
                fragment: format!(
                    "notify = {}\n",
                    serde_json::to_string(&args).map_err(|_| Error::Invalid)?
                ),
            }])
        }
    }
}
fn toml(input: &str) -> Result<DocumentMut> {
    input.parse().map_err(|_| Error::Invalid)
}
fn item<'a>(doc: &'a DocumentMut, path: &[String]) -> Result<Option<&'a Item>> {
    let mut table: &dyn toml_edit::TableLike = doc.as_table();
    for (index, key) in path.iter().enumerate() {
        let Some(found) = table.get(key) else {
            return Ok(None);
        };
        if index == path.len() - 1 {
            return Ok(Some(found));
        }
        table = found.as_table_like().ok_or(Error::Invalid)?;
    }
    Err(Error::Invalid)
}
pub(crate) fn validate(input: &str, owned: &[Owned]) -> Result<()> {
    match owned.first().ok_or(Error::Invalid)? {
        Owned::Json { .. } => {
            json::validate(input)?;
            let value: serde_json::Value =
                serde_json::from_str(input).map_err(|_| Error::Invalid)?;
            for entry in owned {
                let Owned::Json {
                    path, array: true, ..
                } = entry
                else {
                    continue;
                };
                let mut found = &value;
                for key in path {
                    let Some(next) = found.get(key) else {
                        found = &serde_json::Value::Null;
                        break;
                    };
                    found = next;
                }
                if found.is_null() {
                    continue;
                }
                for group in found.as_array().ok_or(Error::Invalid)? {
                    let group = group.as_object().ok_or(Error::Invalid)?;
                    if group
                        .keys()
                        .any(|key| !matches!(key.as_str(), "hooks" | "matcher"))
                        || group.get("matcher").is_some_and(|v| !v.is_string())
                    {
                        return Err(Error::Unsupported);
                    }
                    for handler in group
                        .get("hooks")
                        .and_then(serde_json::Value::as_array)
                        .ok_or(Error::Invalid)?
                    {
                        let handler = handler.as_object().ok_or(Error::Invalid)?;
                        let kind = handler
                            .get("type")
                            .and_then(serde_json::Value::as_str)
                            .ok_or(Error::Invalid)?;
                        if !matches!(kind, "command" | "prompt" | "agent" | "http" | "mcp_tool") {
                            return Err(Error::Unsupported);
                        }
                        if kind == "command"
                            && (handler.get("command").is_none_or(|v| !v.is_string())
                                || handler.get("args").is_some_and(|v| {
                                    v.as_array()
                                        .is_none_or(|args| args.iter().any(|arg| !arg.is_string()))
                                }))
                        {
                            return Err(Error::Invalid);
                        }
                    }
                }
            }
            Ok(())
        }
        Owned::Toml { .. } => toml(input).map(|_| ()),
    }
}
pub(crate) fn install(mut input: String, owned: &[Owned]) -> Result<String> {
    validate(&input, owned)?;
    for entry in owned {
        input = match entry {
            Owned::Json { path, array, raw } => json::install(&input, path, *array, raw)?,
            Owned::Toml { path, fragment } => {
                let doc = toml(&input)?;
                if item(&doc, path)?.is_some() {
                    return Err(Error::Conflict);
                }
                let output = if path.len() == 1 {
                    format!("{fragment}{input}")
                } else {
                    format!("{input}{fragment}")
                };
                // Reject sealed inline parents and all unsupported table/schema collisions.
                toml(&output)?;
                output
            }
        };
    }
    Ok(input)
}
pub(crate) fn verify(input: &str, owned: &[Owned]) -> Result<()> {
    validate(input, owned)?;
    for entry in owned {
        match entry {
            Owned::Json { path, array, raw } => {
                if !json::verify(input, path, *array, raw)? {
                    return Err(Error::Conflict);
                }
            }
            Owned::Toml { path, fragment } => {
                let current = toml(input)?;
                let expected = toml(fragment)?;
                let current = item(&current, path)?.ok_or(Error::Conflict)?;
                let expected = item(&expected, path)?.ok_or(Error::Invalid)?;
                if current.to_string() != expected.to_string()
                    || input.match_indices(fragment).count() != 1
                {
                    return Err(Error::Conflict);
                }
            }
        }
    }
    Ok(())
}
pub(crate) fn remove(mut input: String, owned: &[Owned]) -> Result<String> {
    verify(&input, owned)?;
    for entry in owned.iter().rev() {
        input = match entry {
            Owned::Json { path, array, raw } => json::remove(&input, path, *array, raw)?,
            Owned::Toml { fragment, .. } => input.replacen(fragment, "", 1),
        };
    }
    validate(&input, owned)?;
    Ok(input)
}
