//! Explicit headless agent setup (`--mdreader-setup-agents`). It never runs at
//! ordinary startup and drives the same commands the Settings UI uses, so a
//! rerun converges on the same installed, granted state.
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager};

pub(crate) const FLAG: &str = "--mdreader-setup-agents";
const CLIENTS: [&str; 2] = ["claude", "codex"];
/// Reported when a configured client's effective read grant lacks a source
/// setup requested (the Claude plans folder).
const PLANS_NOT_GRANTED: &str = "Claude plans source is not in the agent read grant";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SetupRequest {
    pub clients: Vec<String>,
    pub result: Option<PathBuf>,
    /// `--track on|off`: set session tracking for each client; absent leaves it.
    pub track: Option<bool>,
}

/// `argv` includes the executable. `Ok(None)` means an ordinary launch.
pub(crate) fn parse(argv: &[String]) -> Result<Option<SetupRequest>, String> {
    if argv.get(1).map(String::as_str) != Some(FLAG) {
        return Ok(None);
    }
    let mut clients = None;
    let mut result = None;
    let mut track = None;
    let mut rest = argv[2..].iter();
    while let Some(flag) = rest.next() {
        let value = rest.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--clients" if clients.is_none() => {
                let mut chosen = Vec::new();
                for client in value.split(',').map(str::trim) {
                    if !CLIENTS.contains(&client) {
                        return Err(format!("Unknown client {client}; use claude,codex"));
                    }
                    if !chosen.iter().any(|c| c == client) {
                        chosen.push(client.to_string());
                    }
                }
                clients = Some(chosen);
            }
            "--result" if result.is_none() => {
                let path = PathBuf::from(value);
                if !path.is_absolute() {
                    return Err("--result must be an absolute path".into());
                }
                result = Some(path);
            }
            "--track" if track.is_none() => {
                track = Some(match value.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => return Err("--track must be on or off".into()),
                });
            }
            _ => return Err(format!("Unsupported argument {flag}")),
        }
    }
    Ok(Some(SetupRequest {
        clients: clients.unwrap_or_else(|| CLIENTS.iter().map(|c| c.to_string()).collect()),
        result,
        track,
    }))
}

/// Best-effort `--result` lookup for reporting a request that did not parse.
fn result_path(argv: &[String]) -> Option<PathBuf> {
    argv.windows(2)
        .find(|pair| pair[0] == "--result")
        .map(|pair| PathBuf::from(&pair[1]))
        .filter(|path| path.is_absolute())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ClientOutcome {
    pub agent: String,
    /// `configured`, or the step that failed: `install`, `mcp`, `permissions`,
    /// `verify` or `tracking`.
    pub phase: String,
    pub error: Option<String>,
    /// Stable machine code for `error` (see [`error_code`]).
    pub code: Option<String>,
}

/// Map a native error display string to its stable code: the reader service
/// wire codes, `agent_setup` for public setup refusals, or a Library error
/// code, which is already code-shaped. Anything else is `error`.
pub(crate) fn error_code(message: &str) -> &str {
    match message {
        "Document revision changed" => "stale_revision",
        PLANS_NOT_GRANTED => "plans_not_granted",
        "Source unavailable" => "source_unavailable",
        "Cancelled" => "cancelled",
        "Local embedding model unavailable" => "model_unavailable",
        _ if message.starts_with("Source authority:") => "source_changed",
        _ if message.starts_with("Invalid request: Agent") => "agent_setup",
        _ if message.starts_with("Invalid request:") => "invalid_request",
        _ if !message.is_empty()
            && message
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') =>
        {
            message
        }
        _ => "error",
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SetupOutcome {
    pub ok: bool,
    pub clients: Vec<ClientOutcome>,
    pub plans_source_id: Option<String>,
    pub plans_error: Option<String>,
    pub plans_error_code: Option<String>,
    pub read_sources: Vec<String>,
    pub error: Option<String>,
}

impl SetupOutcome {
    fn failed(error: String) -> Self {
        Self {
            ok: false,
            clients: vec![],
            plans_source_id: None,
            plans_error: None,
            plans_error_code: None,
            read_sources: vec![],
            error: Some(error),
        }
    }
}

/// The enabled Claude plans source: Library member and reader source ids.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PlansSource {
    pub member: Option<String>,
    pub reader: Option<String>,
}

/// The Settings operations the orchestration needs; a seam for tests.
pub(crate) trait SetupOps {
    fn enable_plans(&mut self) -> Result<PlansSource, String>;
    fn install(&mut self, agent: &str) -> Result<(), String>;
    fn set_mcp(&mut self, agent: &str, sources: &[String]) -> Result<(), String>;
    /// Read = `sources`; edit and reporting projects stay empty. Returns the
    /// effective read grant as the service resolves it.
    fn set_permissions(&mut self, agent: &str, sources: &[String]) -> Result<Vec<String>, String>;
    fn set_tracking(&mut self, agent: &str, enabled: bool) -> Result<(), String>;
}

pub(crate) fn orchestrate(ops: &mut dyn SetupOps, request: &SetupRequest) -> SetupOutcome {
    let (plans, plans_error) = match ops.enable_plans() {
        Ok(plans) => (plans, None),
        Err(error) => (PlansSource::default(), Some(error)),
    };
    // Every enabled source, resolved live by the service: no enumeration, no
    // truncation, and sources added later (or the plans folder) are included.
    let all = [mdreader_service::agents::ALL_SOURCES.to_string()];
    let mut read_sources = BTreeSet::new();
    let mut clients = Vec::new();
    for agent in &request.clients {
        let configured = (|| -> Result<Vec<String>, (&str, String)> {
            ops.install(agent).map_err(|e| ("install", e))?;
            ops.set_mcp(agent, &all).map_err(|e| ("mcp", e))?;
            let granted = ops
                .set_permissions(agent, &all)
                .map_err(|e| ("permissions", e))?;
            if plans
                .reader
                .as_ref()
                .is_some_and(|plans| !granted.contains(plans))
            {
                return Err(("verify", PLANS_NOT_GRANTED.into()));
            }
            if let Some(enabled) = request.track {
                ops.set_tracking(agent, enabled)
                    .map_err(|e| ("tracking", e))?;
            }
            Ok(granted)
        })();
        clients.push(match configured {
            Ok(sources) => {
                read_sources.extend(sources);
                ClientOutcome {
                    agent: agent.clone(),
                    phase: "configured".into(),
                    error: None,
                    code: None,
                }
            }
            Err((phase, error)) => ClientOutcome {
                agent: agent.clone(),
                phase: phase.into(),
                code: Some(error_code(&error).to_string()),
                error: Some(error),
            },
        });
    }
    // The plans source leads the list so it is never lost in a long one.
    let read_sources = plans
        .reader
        .iter()
        .filter(|plans| read_sources.contains(*plans))
        .cloned()
        .chain(
            read_sources
                .iter()
                .filter(|id| Some(*id) != plans.reader.as_ref())
                .cloned(),
        )
        .collect();
    SetupOutcome {
        ok: clients.iter().all(|client| client.error.is_none()),
        clients,
        plans_source_id: plans.member,
        plans_error_code: plans_error.as_deref().map(|e| error_code(e).to_string()),
        plans_error,
        read_sources,
        error: None,
    }
}

fn write_result(path: &Path, outcome: &SetupOutcome) {
    let Ok(bytes) = serde_json::to_vec_pretty(outcome) else {
        return;
    };
    let staged = path.with_extension("partial");
    if std::fs::write(&staged, bytes).is_err() || std::fs::rename(&staged, path).is_err() {
        let _ = std::fs::remove_file(&staged);
        eprintln!("Agent setup result could not be written");
    }
}

/// Report a startup failure to a headless caller instead of a dialog.
/// Returns false when the launch was not a headless setup request.
pub(crate) fn report_startup_failure(argv: &[String], message: &str) -> bool {
    if argv.get(1).map(String::as_str) != Some(FLAG) {
        return false;
    }
    if let Some(path) = result_path(argv) {
        write_result(&path, &SetupOutcome::failed(message.to_string()));
    }
    true
}

struct AppOps(AppHandle);

impl AppOps {
    fn selection(agent: &str) -> crate::reader_agent_setup::SetupSelection {
        crate::reader_agent_setup::SetupSelection {
            agent: agent.into(),
            scope: "user".into(),
            project_root: None,
        }
    }
}

impl SetupOps for AppOps {
    fn enable_plans(&mut self) -> Result<PlansSource, String> {
        let app = &self.0;
        crate::library::enable_agent_plans_source(app.clone(), app.state(), app.state()).map(
            |plans| PlansSource {
                member: plans.source_id,
                reader: plans.reader_source_id,
            },
        )
    }
    fn install(&mut self, agent: &str) -> Result<(), String> {
        let app = &self.0;
        tauri::async_runtime::block_on(crate::reader_agent_setup::agent_setup_install(
            app.clone(),
            Self::selection(agent),
            app.state(),
            app.state(),
            app.state(),
        ))
        .map(drop)
    }
    fn set_mcp(&mut self, agent: &str, sources: &[String]) -> Result<(), String> {
        let app = &self.0;
        tauri::async_runtime::block_on(crate::reader_agent_setup::agent_setup_set_mcp(
            app.clone(),
            Self::selection(agent),
            sources.to_vec(),
            true,
            app.state(),
            app.state(),
            app.state(),
        ))
        .map(drop)
    }
    fn set_permissions(&mut self, agent: &str, sources: &[String]) -> Result<Vec<String>, String> {
        let app = &self.0;
        tauri::async_runtime::block_on(crate::reader_agent_setup::agent_setup_set_permissions(
            app.clone(),
            Self::selection(agent),
            true,
            vec![],
            sources.to_vec(),
            vec![],
            app.state(),
            app.state(),
            app.state(),
        ))
        .map(|state| state.source_ids)
    }
    fn set_tracking(&mut self, agent: &str, enabled: bool) -> Result<(), String> {
        use crate::agent_activity::commands::{set_agent_activity_tracking, AgentKind};
        let kind = match agent {
            "claude" => AgentKind::Claude,
            "codex" => AgentKind::Codex,
            _ => return Err(format!("Unknown client {agent}")),
        };
        let app = &self.0;
        tauri::async_runtime::block_on(set_agent_activity_tracking(
            kind,
            enabled,
            app.state(),
            app.state(),
        ))
        .map(drop)
    }
}

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Handle a launch or forwarded argv. Ordinary launches do nothing.
pub(crate) fn handle(app: &AppHandle, argv: &[String]) {
    let request = match parse(argv) {
        Ok(Some(request)) => request,
        Ok(None) => return,
        Err(error) => {
            eprintln!("Agent setup refused: {error}");
            if let Some(path) = result_path(argv) {
                write_result(&path, &SetupOutcome::failed(error));
            }
            return;
        }
    };
    if RUNNING.swap(true, Ordering::AcqRel) {
        if let Some(path) = &request.result {
            write_result(
                path,
                &SetupOutcome::failed("Agent setup is already running".into()),
            );
        }
        return;
    }
    let app = app.clone();
    // Commands block on the reader service; keep them off the main thread.
    std::thread::spawn(move || {
        let outcome = orchestrate(&mut AppOps(app), &request);
        RUNNING.store(false, Ordering::Release);
        if let Some(path) = &request.result {
            write_result(path, &outcome);
        }
    });
}

/// Single-instance callback: a second launch focuses this window, or runs a
/// forwarded headless setup without stealing focus.
pub(crate) fn on_second_instance(app: &AppHandle, argv: Vec<String>, _cwd: String) {
    if argv.get(1).map(String::as_str) == Some(FLAG) {
        handle(app, &argv);
        return;
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        std::iter::once("/Applications/MDReader.app/Contents/MacOS/mdreader-app")
            .chain(args.iter().copied())
            .map(String::from)
            .collect()
    }

    #[test]
    fn parser_is_explicit_and_strict() {
        assert_eq!(parse(&argv(&[])), Ok(None));
        assert_eq!(parse(&argv(&["-psn_0_1234"])), Ok(None));
        let result = std::env::temp_dir().join("public-setup.json");
        let result_arg = result.to_str().unwrap();
        assert_eq!(
            parse(&argv(&[FLAG])),
            Ok(Some(SetupRequest {
                clients: vec!["claude".into(), "codex".into()],
                result: None,
                track: None,
            }))
        );
        assert_eq!(
            parse(&argv(&[
                FLAG,
                "--clients",
                "codex, claude,codex",
                "--result",
                result_arg
            ])),
            Ok(Some(SetupRequest {
                clients: vec!["codex".into(), "claude".into()],
                result: Some(result.clone()),
                track: None,
            }))
        );
        assert!(parse(&argv(&[FLAG, "--clients", "cursor"])).is_err());
        assert!(parse(&argv(&[FLAG, "--result", "relative.json"])).is_err());
        assert!(parse(&argv(&[FLAG, "--clients"])).is_err());
        assert!(parse(&argv(&[FLAG, "--other", "x"])).is_err());
        assert_eq!(
            parse(&argv(&[FLAG, "--track", "on"]))
                .unwrap()
                .unwrap()
                .track,
            Some(true)
        );
        assert_eq!(
            parse(&argv(&[FLAG, "--track", "off"]))
                .unwrap()
                .unwrap()
                .track,
            Some(false)
        );
        assert!(parse(&argv(&[FLAG, "--track", "yes"])).is_err());
        assert!(parse(&argv(&[FLAG, "--track", "on", "--track", "off"])).is_err());
        assert!(parse(&argv(&[FLAG, "--clients", "claude", "--clients", "codex"])).is_err());
        // The flag is only honored first, never buried in another launch's argv.
        assert_eq!(parse(&argv(&["--reader-profile", FLAG])), Ok(None));
        assert_eq!(
            result_path(&argv(&[FLAG, "--bad", "--result", result_arg])),
            Some(result)
        );
    }

    /// A service fake: `*` resolves to every enabled source, like the real one.
    #[derive(Default)]
    struct FakeOps {
        sources: Vec<String>,
        failing_install: Option<String>,
        plans_error: Option<String>,
        drop_plans_from_grant: bool,
        failing_tracking: bool,
        calls: Vec<String>,
    }

    impl SetupOps for FakeOps {
        fn enable_plans(&mut self) -> Result<PlansSource, String> {
            self.calls.push("plans".into());
            if let Some(error) = &self.plans_error {
                return Err(error.clone());
            }
            if !self.sources.iter().any(|s| s == "zz-plans-reader") {
                self.sources.push("zz-plans-reader".into());
            }
            Ok(PlansSource {
                member: Some("plans-member".into()),
                reader: Some("zz-plans-reader".into()),
            })
        }
        fn install(&mut self, agent: &str) -> Result<(), String> {
            self.calls.push(format!("install:{agent}"));
            if self.failing_install.as_deref() == Some(agent) {
                return Err("Install the agent CLI, then try again".into());
            }
            Ok(())
        }
        fn set_mcp(&mut self, agent: &str, sources: &[String]) -> Result<(), String> {
            self.calls
                .push(format!("mcp:{agent}:{}", sources.join("+")));
            Ok(())
        }
        fn set_permissions(
            &mut self,
            agent: &str,
            sources: &[String],
        ) -> Result<Vec<String>, String> {
            self.calls
                .push(format!("permissions:{agent}:{}", sources.join("+")));
            assert_eq!(sources, [mdreader_service::agents::ALL_SOURCES]);
            Ok(self
                .sources
                .iter()
                .filter(|s| !(self.drop_plans_from_grant && s.as_str() == "zz-plans-reader"))
                .cloned()
                .collect())
        }
        fn set_tracking(&mut self, agent: &str, enabled: bool) -> Result<(), String> {
            self.calls.push(format!("tracking:{agent}:{enabled}"));
            if self.failing_tracking {
                return Err("Source unavailable".into());
            }
            Ok(())
        }
    }

    #[test]
    fn tracking_is_set_only_when_requested_and_reports_its_phase() {
        let mut ops = FakeOps::default();
        let request = parse(&argv(&[FLAG, "--clients", "claude"]))
            .unwrap()
            .unwrap();
        assert!(orchestrate(&mut ops, &request).ok);
        assert!(!ops.calls.iter().any(|call| call.starts_with("tracking:")));

        let request = parse(&argv(&[FLAG, "--clients", "claude,codex", "--track", "on"]))
            .unwrap()
            .unwrap();
        ops.calls.clear();
        assert!(orchestrate(&mut ops, &request).ok);
        assert!(ops.calls.contains(&"tracking:claude:true".to_string()));
        assert!(ops.calls.contains(&"tracking:codex:true".to_string()));

        let mut failing = FakeOps {
            failing_tracking: true,
            ..FakeOps::default()
        };
        let outcome = orchestrate(&mut failing, &request);
        assert!(!outcome.ok);
        assert_eq!(outcome.clients[0].phase, "tracking");
        assert_eq!(
            outcome.clients[0].code.as_deref(),
            Some("source_unavailable")
        );
    }

    #[test]
    fn orchestration_grants_all_sources_with_plans_first_and_is_idempotent() {
        let request = parse(&argv(&[FLAG])).unwrap().unwrap();
        // More sources than any explicit list bound, plans sorting last.
        let mut ops = FakeOps {
            sources: (0..600).map(|i| format!("doc-{i:03}")).collect(),
            ..FakeOps::default()
        };
        let first = orchestrate(&mut ops, &request);
        assert!(first.ok, "{first:?}");
        assert_eq!(first.plans_source_id.as_deref(), Some("plans-member"));
        assert_eq!(first.read_sources.len(), 601);
        assert_eq!(first.read_sources[0], "zz-plans-reader");
        assert_eq!(
            ops.calls,
            [
                "plans",
                "install:claude",
                "mcp:claude:*",
                "permissions:claude:*",
                "install:codex",
                "mcp:codex:*",
                "permissions:codex:*",
            ]
        );
        ops.calls.clear();
        let second = orchestrate(&mut ops, &request);
        assert_eq!(second, first);
        assert_eq!(ops.calls.len(), 7);
    }

    #[test]
    fn a_grant_missing_the_plans_source_fails_loudly() {
        let request = parse(&argv(&[FLAG, "--clients", "claude"]))
            .unwrap()
            .unwrap();
        let mut ops = FakeOps {
            sources: vec!["notes".into()],
            drop_plans_from_grant: true,
            ..FakeOps::default()
        };
        let outcome = orchestrate(&mut ops, &request);
        assert!(!outcome.ok);
        assert_eq!(outcome.clients[0].phase, "verify");
        assert_eq!(
            outcome.clients[0].code.as_deref(),
            Some("plans_not_granted")
        );
    }

    #[test]
    fn one_failing_client_or_missing_plans_does_not_stop_the_others() {
        let request = parse(&argv(&[FLAG, "--clients", "codex,claude"]))
            .unwrap()
            .unwrap();
        let mut ops = FakeOps {
            failing_install: Some("codex".into()),
            plans_error: Some("No Claude Code plans folder".into()),
            ..FakeOps::default()
        };
        let outcome = orchestrate(&mut ops, &request);
        assert!(!outcome.ok);
        assert_eq!(outcome.plans_source_id, None);
        assert_eq!(
            outcome.plans_error.as_deref(),
            Some("No Claude Code plans folder")
        );
        assert_eq!(outcome.clients[0].phase, "install");
        assert!(outcome.clients[0].error.is_some());
        assert_eq!(outcome.clients[1].phase, "configured");
        assert!(ops.calls.contains(&"permissions:claude:*".to_string()));
        assert!(!ops.calls.iter().any(|call| call.starts_with("mcp:codex")));
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["clients"][0]["agent"], "codex");
        assert_eq!(json["clients"][0]["code"], "error");
        assert_eq!(json["plans_error_code"], "error");
    }

    #[test]
    fn errors_carry_stable_codes() {
        assert_eq!(error_code("Document revision changed"), "stale_revision");
        assert_eq!(error_code("invalid-library-root"), "invalid-library-root");
        assert_eq!(
            error_code("Invalid request: Agent integration conflicts with existing settings. Existing files were kept."),
            "agent_setup"
        );
        assert_eq!(
            error_code("Source authority: identity-changed"),
            "source_changed"
        );
        assert_eq!(error_code("Install the agent CLI, then try again"), "error");
    }

    #[test]
    fn headless_startup_failure_is_written_instead_of_a_dialog() {
        let dir = tempfile::tempdir().unwrap();
        let result = dir.path().join("result.json");
        let args = argv(&[FLAG, "--result", result.to_str().unwrap()]);
        assert!(report_startup_failure(
            &args,
            "Selected library unavailable; libraries were kept"
        ));
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&result).unwrap()).unwrap();
        assert_eq!(written["ok"], false);
        assert_eq!(
            written["error"],
            "Selected library unavailable; libraries were kept"
        );
        assert!(!report_startup_failure(&argv(&[]), "ignored"));
    }
}
