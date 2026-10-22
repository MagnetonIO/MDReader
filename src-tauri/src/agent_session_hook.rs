//! Bounded hook metadata. Records the session and the Markdown files it
//! wrote straight into agent activity (no project needs to contain `cwd`),
//! then asks the owner to read the transcript's new lines. The hook itself
//! never opens the transcript or retains tool bodies.
use mdreader_core::agent_activity::FileOp;
use mdreader_service::{
    agents::{activity::ReportedFile, projects::AgentRequest},
    Request,
};
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
    #[serde(default)]
    transcript_path: Option<PathBuf>,
    /// A client-given session name, when the client sends one.
    #[serde(default)]
    session_title: Option<String>,
}
fn paths(event: &Event) -> Vec<(PathBuf, FileOp)> {
    if event.hook_event_name != "PostToolUse" {
        return vec![];
    }
    let input = event.tool_input.as_ref();
    let named = |key: &str| {
        input
            .and_then(|v| v.get(key))
            .and_then(Value::as_str)
            .map(PathBuf::from)
    };
    let mut files = vec![];
    match event.tool_name.as_deref() {
        Some("Write" | "write_file") => {
            files.extend(
                named("file_path")
                    .or_else(|| named("path"))
                    .map(|p| (p, FileOp::Create)),
            );
        }
        Some("Edit" | "MultiEdit" | "edit_file") => {
            files.extend(
                named("file_path")
                    .or_else(|| named("path"))
                    .map(|p| (p, FileOp::Edit)),
            );
        }
        // Claude Code injects the plan file path for hooks. Without it the
        // plan's own Write event already reported the file.
        Some("ExitPlanMode") => files.extend(named("planFilePath").map(|p| (p, FileOp::Create))),
        Some("apply_patch") => {
            if let Some(patch) = input.and_then(|v| v.get("command")).and_then(Value::as_str) {
                for line in patch.lines() {
                    let found = if let Some(path) = line.strip_prefix("*** Add File: ") {
                        Some((path, FileOp::Create))
                    } else {
                        line.strip_prefix("*** Update File: ")
                            .or_else(|| line.strip_prefix("*** Move to: "))
                            .map(|path| (path, FileOp::Edit))
                    };
                    if let Some((path, op)) = found {
                        files.push((PathBuf::from(path), op));
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
        .filter_map(|(path, op)| {
            let text = path.to_str()?;
            let name = path.file_name()?.to_str()?;
            if !mdreader_core::agent_sessions::markdown_path(name) || text.len() > 4096 {
                return None;
            }
            let path = if path.is_absolute() {
                path
            } else {
                event.cwd.join(path)
            };
            Some((path, op))
        })
        .collect()
}
fn report(directory: &Path, grant: &str, event: Event) -> Result<(), ()> {
    attribute(
        event,
        &mut |request| {
            mdreader_service::call(
                directory,
                &Request::AgentCall {
                    grant_id: grant.into(),
                    request,
                },
            )
            .map_err(|_| ())
        },
        &mut |transcript| {
            // Tracking off or an unknown location is refused by the owner.
            let _ = mdreader_service::call(
                directory,
                &Request::AgentActivity {
                    request: mdreader_service::agents::activity::ActivityRequest::Ingest {
                        paths: vec![transcript],
                    },
                },
            );
        },
    )
}
/// Service calls are injected so attribution is testable without a service.
fn attribute(
    event: Event,
    call: &mut dyn FnMut(AgentRequest) -> Result<Value, ()>,
    ingest: &mut dyn FnMut(PathBuf),
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
    let files = paths(&event)
        .into_iter()
        .map(|(path, op)| ReportedFile { path, op })
        .collect();
    call(AgentRequest::RecordActivity {
        client_session_id: event.session_id.clone(),
        cwd: event.cwd.clone(),
        files,
        ended: event.hook_event_name == "SessionEnd",
        title: event.session_title.clone(),
    })?;
    if let Some(transcript) = event.transcript_path.filter(|p| p.is_absolute()) {
        ingest(transcript);
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
        report(&directory, &grant, event)
    })();
    let _ = outcome;
    println!("{{}}");
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn event(value: Value) -> Event {
        serde_json::from_value(value).unwrap()
    }
    #[test]
    fn recognized_paths_only_and_transcripts_are_ignored() {
        let event = event(
            serde_json::json!({"cwd":"/public/project","session_id":"synthetic","hook_event_name":"PostToolUse","tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Add File: docs/a.md\n+body\n*** Move to: docs/b.md\n*** Add File: src/code.rs\n*** End Patch"},"transcript_path":"/never/read"}),
        );
        assert_eq!(
            paths(&event),
            vec![
                (PathBuf::from("/public/project/docs/a.md"), FileOp::Create),
                (PathBuf::from("/public/project/docs/b.md"), FileOp::Edit)
            ]
        );
        let uppercase = Event {
            tool_name: Some("Write".into()),
            tool_input: Some(serde_json::json!({"file_path":"/public/project/NOTE.MD"})),
            ..event
        };
        assert_eq!(
            paths(&uppercase),
            vec![(PathBuf::from("/public/project/NOTE.MD"), FileOp::Create)]
        );
        let event = Event {
            tool_name: Some("Bash".into()),
            ..uppercase
        };
        assert!(paths(&event).is_empty());
    }
    #[test]
    fn plan_writes_and_exit_plan_mode_paths_are_recognized() {
        let write = event(
            serde_json::json!({"cwd":"/public/project","session_id":"synthetic","hook_event_name":"PostToolUse","tool_name":"Write","tool_input":{"file_path":"/public/home/.claude/plans/public-plan.md"}}),
        );
        let plan = PathBuf::from("/public/home/.claude/plans/public-plan.md");
        assert_eq!(paths(&write), vec![(plan.clone(), FileOp::Create)]);
        let exit = Event {
            tool_name: Some("ExitPlanMode".into()),
            tool_input: Some(serde_json::json!({"plan":"# Public plan","planFilePath":plan})),
            ..write
        };
        assert_eq!(paths(&exit), vec![(plan, FileOp::Create)]);
        let bare = Event {
            tool_input: Some(serde_json::json!({"plan":"# Public plan"})),
            ..exit
        };
        assert!(paths(&bare).is_empty());
    }
    #[test]
    fn a_cwd_outside_every_project_is_recorded_and_its_transcript_ingested() {
        let event = event(
            serde_json::json!({"cwd":"/public/unlisted","session_id":"synthetic","hook_event_name":"PostToolUse","tool_name":"Edit","tool_input":{"file_path":"notes.md"},"transcript_path":"/public/home/.claude/projects/-public-unlisted/synthetic.jsonl"}),
        );
        let mut requests = vec![];
        let mut ingested = vec![];
        attribute(
            event,
            &mut |request| {
                requests.push(request);
                Ok(serde_json::json!({"session_id":"activity"}))
            },
            &mut |path| ingested.push(path),
        )
        .unwrap();
        let [AgentRequest::RecordActivity {
            client_session_id,
            cwd,
            files,
            ended,
            title,
        }] = requests.as_slice()
        else {
            panic!("expected one activity record");
        };
        assert_eq!(client_session_id, "synthetic");
        assert_eq!(cwd, Path::new("/public/unlisted"));
        assert_eq!(
            files,
            &[ReportedFile {
                path: PathBuf::from("/public/unlisted/notes.md"),
                op: FileOp::Edit
            }]
        );
        assert!(!ended);
        assert_eq!(title, &None);
        assert_eq!(
            ingested,
            [PathBuf::from(
                "/public/home/.claude/projects/-public-unlisted/synthetic.jsonl"
            )]
        );
        let end = event_end();
        let mut ended_requests = vec![];
        attribute(
            end,
            &mut |r| {
                ended_requests.push(r);
                Ok(Value::Null)
            },
            &mut |_| {},
        )
        .unwrap();
        assert!(matches!(
            ended_requests.as_slice(),
            [AgentRequest::RecordActivity { ended: true, .. }]
        ));
    }
    fn event_end() -> Event {
        event(
            serde_json::json!({"cwd":"/public/unlisted","session_id":"synthetic","hook_event_name":"SessionEnd"}),
        )
    }
}
