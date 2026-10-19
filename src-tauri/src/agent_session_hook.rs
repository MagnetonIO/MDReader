//! Bounded hook metadata. Never opens transcripts or retains tool bodies.
use mdreader_core::SourceRoot;
use mdreader_service::{agents::projects::AgentRequest, Request};
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
        Some("ExitPlanMode") => {
            // Claude Code injects the plan file path for hooks. Without it the
            // plan's own Write event already reported the file.
            if let Some(path) = input
                .and_then(|v| v.get("planFilePath"))
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
            let text = path.to_str()?;
            let name = path.file_name()?.to_str()?;
            if !mdreader_core::agent_sessions::markdown_path(name) || text.len() > 4096 {
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
/// The Claude Code plans folder and the reader source indexing it, if the
/// user enabled it. Registration is still authorized by the service.
fn plans_source(directory: &Path) -> Option<(SourceRoot, String)> {
    let dir = crate::library::claude_plans_dir(&crate::agent_cli::home().ok()?)?;
    let root = SourceRoot::capture(dir.canonicalize().ok()?).ok()?;
    let found = mdreader_service::call(
        directory,
        &Request::FindSource {
            path: root.path.clone(),
            identity: root.identity.clone(),
        },
    )
    .ok()?;
    let source: mdreader_core::Source = serde_json::from_value::<Option<_>>(found).ok()??;
    Some((root, source.id))
}
fn plan_relative(root: &SourceRoot, path: &Path) -> Option<String> {
    let relative = root.relative_path(path).ok()?;
    let value = relative.to_str()?;
    (!value.is_empty() && value.len() <= 4096).then(|| value.replace('\\', "/"))
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
    attribute(
        agent,
        event,
        &mut |request| call(directory, grant, request),
        &mut || plans_source(directory),
    )
}
/// Service calls are injected so attribution is testable without a service.
fn attribute(
    agent: &str,
    event: Event,
    call: &mut dyn FnMut(AgentRequest) -> Result<Value, ()>,
    plans_source: &mut dyn FnMut() -> Option<(SourceRoot, String)>,
) -> Result<(), ()> {
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
    let found = call(AgentRequest::FindProject {
        path: event.cwd.clone(),
    })?;
    let project = found
        .get("project_id")
        .and_then(Value::as_str)
        .ok_or(())?
        .to_owned();
    let session = call(AgentRequest::RegisterSession {
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
    })?;
    let id = session
        .get("id")
        .and_then(Value::as_str)
        .ok_or(())?
        .to_owned();
    let mut plans = None;
    for path in paths(&event) {
        let Ok(found) = call(AgentRequest::FindProject { path: path.clone() }) else {
            continue;
        };
        if found.get("project_id").and_then(Value::as_str) == Some(project.as_str()) {
            if let Some(path) = found.get("relative_path").and_then(Value::as_str) {
                let _ = call(AgentRequest::RegisterDocument {
                    session_id: id.clone(),
                    relative_path: path.into(),
                });
            }
            continue;
        }
        let Some((root, source_id)) = plans.get_or_insert_with(&mut *plans_source) else {
            continue;
        };
        if let Some(relative_path) = plan_relative(root, &path) {
            let _ = call(AgentRequest::RegisterSourceDocument {
                session_id: id.clone(),
                source_id: source_id.clone(),
                relative_path,
            });
        }
    }
    if event.hook_event_name == "SessionEnd" {
        call(AgentRequest::EndSession { session_id: id })?;
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
        let uppercase = Event {
            tool_name: Some("Write".into()),
            tool_input: Some(serde_json::json!({"file_path":"/public/project/NOTE.MD"})),
            ..event
        };
        assert_eq!(
            paths(&uppercase),
            vec![PathBuf::from("/public/project/NOTE.MD")]
        );
        let event = Event {
            tool_name: Some("Bash".into()),
            ..uppercase
        };
        assert!(paths(&event).is_empty());
    }
    #[test]
    fn plans_write_registers_against_the_enabled_granted_plans_source() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let plans = home.join("plans");
        std::fs::create_dir(&plans).unwrap();
        let plan = plans.join("public-plan.md");
        std::fs::write(&plan, "# Public plan").unwrap();
        let project = home.join("project");
        let event: Event = serde_json::from_value(serde_json::json!({"cwd":project,"session_id":"synthetic","hook_event_name":"PostToolUse","tool_name":"Write","tool_input":{"file_path":plan}})).unwrap();
        let mut registered = vec![];
        let mut call = |request: AgentRequest| -> Result<Value, ()> {
            Ok(match request {
                AgentRequest::FindProject { path } if path == project => {
                    serde_json::json!({"project_id":"project-source","relative_path":""})
                }
                AgentRequest::FindProject { .. } => {
                    serde_json::json!({"project_id":null,"relative_path":null})
                }
                AgentRequest::RegisterSession { project_id, .. } => {
                    assert_eq!(project_id, "project-source");
                    serde_json::json!({"id":"session"})
                }
                AgentRequest::RegisterSourceDocument {
                    session_id,
                    source_id,
                    relative_path,
                } => {
                    registered.push((session_id, source_id, relative_path));
                    serde_json::json!({"registered":true})
                }
                _ => return Err(()),
            })
        };
        let root = SourceRoot::capture(plans.clone()).unwrap();
        attribute("claude", event, &mut call, &mut || {
            Some((root.clone(), "plans-source".into()))
        })
        .unwrap();
        assert_eq!(
            registered,
            vec![(
                "session".to_string(),
                "plans-source".to_string(),
                "public-plan.md".to_string()
            )]
        );
    }
    #[test]
    fn plan_writes_and_exit_plan_mode_paths_are_recognized() {
        let temp = tempfile::tempdir().unwrap();
        let plans = temp.path().canonicalize().unwrap().join("plans");
        std::fs::create_dir(&plans).unwrap();
        let plan = plans.join("public-plan.md");
        std::fs::write(&plan, "# Public plan").unwrap();
        let write: Event = serde_json::from_value(serde_json::json!({"cwd":"/public/project","session_id":"synthetic","hook_event_name":"PostToolUse","tool_name":"Write","tool_input":{"file_path":plan}})).unwrap();
        assert_eq!(paths(&write), vec![plan.clone()]);
        let exit = Event {
            tool_name: Some("ExitPlanMode".into()),
            tool_input: Some(serde_json::json!({"plan":"# Public plan","planFilePath":plan})),
            ..write
        };
        assert_eq!(paths(&exit), vec![plan.clone()]);
        let bare = Event {
            tool_input: Some(serde_json::json!({"plan":"# Public plan"})),
            ..exit
        };
        assert!(paths(&bare).is_empty());
        let root = SourceRoot::capture(plans.clone()).unwrap();
        assert_eq!(
            plan_relative(&root, &plan).as_deref(),
            Some("public-plan.md")
        );
        assert_eq!(
            plan_relative(&root, &temp.path().join("elsewhere.md")),
            None
        );
    }
}
