//! Bounded hook metadata. Never opens transcripts or retains tool bodies.
use mdreader_service::{Request, agents::projects::AgentRequest};
use serde::Deserialize;
use serde_json::Value;
use std::{
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
struct Event {
    session_id: String,
    cwd: PathBuf,
    hook_event_name: String,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_input: Option<Value>,
}
fn paths(event: &Event) -> Vec<PathBuf> {
    if event.hook_event_name != "PostToolUse" {
        return vec![];
    }
    let input = event.tool_input.as_ref();
    let mut files = vec![];
    match event.tool_name.as_deref() {
        Some("Write" | "Edit" | "MultiEdit" | "write_file" | "edit_file") => {
            if let Some(path) = input
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")))
                .and_then(Value::as_str)
            {
                files.push(PathBuf::from(path));
            }
        }
        Some("apply_patch") => {
            if let Some(patch) = input.and_then(|v| v.get("command")).and_then(Value::as_str) {
                for line in patch.lines() {
                    if let Some(path) = line
                        .strip_prefix("*** Add File: ")
                        .or_else(|| line.strip_prefix("*** Update File: "))
                        .or_else(|| line.strip_prefix("*** Move to: "))
                    {
                        files.push(PathBuf::from(path));
                        if files.len() >= 128 {
                            break;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    files
        .into_iter()
        .filter_map(|path| {
            let extension = path.extension()?.to_str()?;
            if !matches!(extension, "md" | "markdown") || path.to_string_lossy().len() > 4096 {
                return None;
            }
            Some(if path.is_absolute() {
                path
            } else {
                event.cwd.join(path)
            })
        })
        .collect()
}
fn call(directory: &Path, grant: &str, request: AgentRequest) -> Result<Value, ()> {
    mdreader_service::call(
        directory,
        &Request::AgentCall {
            grant_id: grant.into(),
            request,
        },
    )
    .map_err(|_| ())
}
fn report(directory: &Path, grant: &str, agent: &str, event: Event) -> Result<(), ()> {
    if !event.cwd.is_absolute()
        || event.session_id.is_empty()
        || event.session_id.len() > 256
        || !matches!(
            event.hook_event_name.as_str(),
            "SessionStart" | "SessionEnd" | "PostToolUse"
        )
    {
        return Err(());
    }
    let found = call(
        directory,
        grant,
        AgentRequest::FindProject {
            path: event.cwd.clone(),
        },
    )?;
    let project = found
        .get("project_id")
        .and_then(Value::as_str)
        .ok_or(())?
        .to_owned();
    let session = call(
        directory,
        grant,
        AgentRequest::RegisterSession {
            project_id: project.clone(),
            client_session_id: event.session_id.clone(),
            title: format!(
                "{} session",
                if agent == "claude" {
                    "Claude Code"
                } else {
                    "Codex"
                }
            ),
        },
    )?;
    let id = session
        .get("id")
        .and_then(Value::as_str)
        .ok_or(())?
        .to_owned();
    for path in paths(&event) {
        let Ok(found) = call(directory, grant, AgentRequest::FindProject { path }) else {
            continue;
        };
        if found.get("project_id").and_then(Value::as_str) != Some(project.as_str()) {
            continue;
        }
        if let Some(path) = found.get("relative_path").and_then(Value::as_str) {
            let _ = call(
                directory,
                grant,
                AgentRequest::RegisterDocument {
                    session_id: id.clone(),
                    relative_path: path.into(),
                },
            );
        }
    }
    if event.hook_event_name == "SessionEnd" {
        call(
            directory,
            grant,
            AgentRequest::EndSession { session_id: id },
        )?;
    }
    Ok(())
}
pub(crate) fn run(mut args: impl Iterator<Item = OsString>) -> Result<(), String> {
    // Failures never block the agent's work; folder watching remains the fallback.
    let outcome = (|| -> Result<(), ()> {
        let (mut agent, mut grant, mut directory) = (None, None, None);
        while let Some(flag) = args.next() {
            let value = args.next().ok_or(())?;
            match flag.to_str() {
                Some("--agent") if agent.is_none() => agent = value.into_string().ok(),
                Some("--grant") if grant.is_none() => grant = value.into_string().ok(),
                Some("--data-dir") if directory.is_none() => directory = Some(PathBuf::from(value)),
                _ => return Err(()),
            }
        }
        let agent = agent.ok_or(())?;
        if !matches!(agent.as_str(), "claude" | "codex") {
            return Err(());
        }
        let grant = grant.ok_or(())?;
        uuid::Uuid::parse_str(&grant).map_err(|_| ())?;
        let directory = directory.ok_or(())?;
        if !directory.is_absolute() {
            return Err(());
        }
        let mut bytes = vec![];
        std::io::stdin()
            .take(256 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ())?;
        if bytes.len() > 256 * 1024 {
            return Err(());
        }
        let event: Event = serde_json::from_slice(&bytes).map_err(|_| ())?;
        let executable = std::env::current_exe().map_err(|_| ())?;
        mdreader_service::ensure_running(&directory, &executable, &["--mdreader-service".into()])
            .map_err(|_| ())?;
        report(&directory, &grant, &agent, event)
    })();
    let _ = outcome;
    println!("{{}}");
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognized_paths_only_and_transcripts_are_ignored() {
        let event:Event=serde_json::from_value(serde_json::json!({"cwd":"/public/project","session_id":"synthetic","hook_event_name":"PostToolUse","tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Add File: docs/a.md\n+body\n*** Move to: docs/b.md\n*** Add File: src/code.rs\n*** End Patch"},"transcript_path":"/never/read"})).unwrap();
        assert_eq!(
            paths(&event),
            vec![
                PathBuf::from("/public/project/docs/a.md"),
                PathBuf::from("/public/project/docs/b.md")
            ]
        );
        let event = Event {
            tool_name: Some("Bash".into()),
            ..event
        };
        assert!(paths(&event).is_empty());
    }
}
