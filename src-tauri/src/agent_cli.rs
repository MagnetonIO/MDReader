//! Bundled command and bounded client version probe; never starts a model turn.
use mdreader_service::agents::{Action, Agent, Scope, Selection};
use std::{
    ffi::OsString,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
const USAGE: &str = "--mdreader-agent install|uninstall --agent claude|codex --scope project|user --data-dir ABS_PROFILE [--project ABS_FOLDER]";
pub(crate) fn home() -> Result<PathBuf, String> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let path = PathBuf::from(std::env::var_os(key).ok_or("Home folder unavailable")?);
    if !path.is_absolute() {
        return Err("Home folder unavailable".into());
    }
    Ok(path)
}
pub(crate) fn selection(
    agent: Agent,
    scope: Scope,
    project: Option<PathBuf>,
) -> Result<Selection, String> {
    let path = match scope {
        Scope::User => home()?,
        Scope::Project => project.ok_or("Choose a project folder")?,
    };
    let root =
        mdreader_core::SourceRoot::capture(path).map_err(|_| "Selected folder unavailable")?;
    root.verify().map_err(|_| "Selected folder changed")?;
    Ok(Selection {
        agent,
        scope,
        root: root.path,
        identity: root.identity,
    })
}
pub(crate) fn probe(agent: Agent) -> Option<String> {
    let program = match agent {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
    };
    // Output goes to a temporary file so a hostile/broken child cannot block on a pipe.
    let mut output = tempfile::tempfile().ok()?;
    let mut child = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(output.try_clone().ok()?)
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                break;
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    use std::io::{Read, Seek};
    output.rewind().ok()?;
    let mut version = String::new();
    output.take(129).read_to_string(&mut version).ok()?;
    let version = version.trim();
    if version.is_empty()
        || version.len() > 128
        || version.chars().any(char::is_control)
        || !version.contains(|c: char| c.is_ascii_digit())
    {
        return None;
    }
    Some(version.into())
}
pub fn run(args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let mut args = args;
    let action = args
        .next()
        .and_then(|s| s.into_string().ok())
        .ok_or(USAGE)?;
    if action == "session-hook" {
        return crate::agent_session_hook::run(args);
    }
    let (mut agent, mut scope, mut directory, mut project) = (None, None, None, None);
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(USAGE)?;
        match flag.to_str() {
            Some("--agent") if agent.is_none() => {
                agent = Some(match value.to_str() {
                    Some("claude") => Agent::Claude,
                    Some("codex") => Agent::Codex,
                    _ => return Err(USAGE.into()),
                })
            }
            Some("--scope") if scope.is_none() => {
                scope = Some(match value.to_str() {
                    Some("project") => Scope::Project,
                    Some("user") => Scope::User,
                    _ => return Err(USAGE.into()),
                })
            }
            Some("--data-dir") if directory.is_none() => directory = Some(PathBuf::from(value)),
            Some("--project") if project.is_none() => project = Some(PathBuf::from(value)),
            _ => return Err(USAGE.into()),
        }
    }
    let agent = agent.ok_or(USAGE)?;
    let scope = scope.ok_or(USAGE)?;
    if scope == Scope::User && project.is_some() {
        return Err(USAGE.into());
    }
    if scope == Scope::Project && project.is_none() {
        project = Some(std::env::current_dir().map_err(|_| "Project unavailable")?);
    }
    let selection = selection(agent, scope, project)?;
    let directory = directory.ok_or(USAGE)?;
    if !directory.is_absolute() {
        return Err(USAGE.into());
    }
    let executable = std::env::current_exe().map_err(|_| "Executable unavailable")?;
    let installing = action == "install";
    let action = match action.as_str() {
        "install" => Action::Install {
            selection,
            executable: executable.clone(),
            client_version: probe(agent)
                .ok_or("Agent CLI unavailable; install Claude Code or Codex first")?,
        },
        "uninstall" => Action::Uninstall { selection },
        _ => return Err(USAGE.into()),
    };
    mdreader_service::ensure_running(&directory, &executable, &["--mdreader-service".into()])
        .map_err(|_| "Open MDReader once to prepare its library")?;
    mdreader_service::call(
        &directory,
        &mdreader_service::Request::AgentSetup { action },
    )
    .map_err(|_| "Agent setup could not finish; existing files were retained")?;
    if installing {
        println!(
            "MDReader agent setup complete. Enable MCP in Settings to share selected sources."
        );
    } else {
        println!("MDReader agent integration removed. MCP access is off.");
    }
    Ok(())
}
