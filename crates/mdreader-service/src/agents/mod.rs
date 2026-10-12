//! Explicit installation and revocable local retrieval grants. No source discovery.
use mdreader_core::{ReaderError, ReaderStore, Result, SourceRoot, SourceRootIdentity};
use mdreader_integrations::{Capabilities, InstallRequest, Installer, IntegrationKind};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
mod assets;
mod errors;
mod grants;
use errors::integration_error;
pub(super) use errors::public_error;
mod documents;
mod hooks;
mod operation_store;
mod permissions;
pub mod projects;
mod schema;
pub use grants::{authorize, sources};
pub(crate) use permissions::WriteFence;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    Claude,
    Codex,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Project,
    User,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub agent: Agent,
    pub scope: Scope,
    pub root: PathBuf,
    pub identity: SourceRootIdentity,
}
impl Selection {
    pub fn id(&self) -> Result<String> {
        let bytes = serde_json::to_vec(self).map_err(|_| invalid())?;
        Ok(uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, &bytes).to_string())
    }
    fn checked(&self) -> Result<SourceRoot> {
        let root = SourceRoot::restore(self.root.clone(), self.identity.clone())?;
        root.verify()?;
        Ok(root)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    State {
        selection: Selection,
    },
    Install {
        selection: Selection,
        executable: PathBuf,
        client_version: String,
    },
    SetMcp {
        selection: Selection,
        sources: Vec<String>,
        enabled: bool,
    },
    SetPermissions {
        selection: Selection,
        enabled: bool,
        project_ids: Vec<String>,
        read_source_ids: Vec<String>,
        edit_source_ids: Vec<String>,
    },
    Uninstall {
        selection: Selection,
    },
}
#[derive(Debug, Serialize, Deserialize)]
pub struct State {
    pub installed: bool,
    pub enabled: bool,
    pub source_ids: Vec<String>,
    pub sources: Vec<mdreader_core::Source>,
    pub reporting_project_ids: Vec<String>,
    pub edit_source_ids: Vec<String>,
    pub editable_source_ids: Vec<String>,
    pub connected: bool,
    pub last_seen_ms: Option<i64>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    selection: Selection,
    executable: PathBuf,
    client_version: String,
    phase: String,
    #[serde(default)]
    skill_conflict: bool,
    // A legacy completed installation proves ownership; an incomplete intent does not.
    #[serde(default)]
    skill_owned: Option<bool>,
}
impl Record {
    fn owns_skill(&self) -> bool {
        self.skill_owned.unwrap_or(self.phase == "installed")
    }
}
fn invalid() -> ReaderError {
    super::protocol_error("Agent setup is unavailable or changed")
}
fn policy(selection: &Selection) -> Result<mdreader_capture::ProjectPolicy> {
    Ok(mdreader_capture::ProjectPolicy {
        enabled: false,
        project_id: format!("agent-{}", selection.id()?),
        project_root: selection.root.clone(),
        repository_root: selection.root.clone(),
        worktree_roots: vec![selection.root.clone()],
        capture_subagents: false,
        excluded_paths: vec![],
        excluded_sessions: vec![],
        selected_backfill_sessions: vec![],
    })
}
fn request(directory: &Path, record: &Record) -> Result<InstallRequest> {
    let kind = match (record.selection.agent, record.selection.scope) {
        (Agent::Claude, Scope::Project) => IntegrationKind::ClaudeMcp,
        (Agent::Claude, Scope::User) => IntegrationKind::ClaudeUserMcp,
        (Agent::Codex, _) => IntegrationKind::CodexMcp,
    };
    let policy = policy(&record.selection)?;
    Ok(InstallRequest {
        kind,
        worktree_root: record.selection.root.clone(),
        executable: record.executable.clone(),
        data_dir: directory.to_owned(),
        policy_path: super::capture::policy_path(directory, &policy.project_id),
        spool_root: directory.join("captures"),
        source_ids: vec![],
        mcp_grant_id: Some(record.selection.id()?),
        capabilities: Capabilities {
            local_commands: true,
            client_version: record.client_version.clone(),
            ..Capabilities::default()
        },
    })
}
fn installer(directory: &Path) -> Result<Installer> {
    let root = SourceRoot::capture(directory.to_owned())?;
    let state = root.create_or_capture_directory_relative(Path::new("agent-settings"))?;
    Installer::open(&state.path).map_err(|_| invalid())
}
fn state(store: &mut ReaderStore, directory: &Path, selection: &Selection) -> Result<State> {
    selection.checked()?;
    let id = selection.id()?;
    let record = grants::record(store, &id)?;
    let installed = match record {
        Some(mut record) if matches!(record.phase.as_str(), "installed" | "installing") => {
            let req = request(directory, &record)?;
            let installed = assets::present(selection,record.owns_skill()).unwrap_or(false)
                && installer(directory)?
                    .verify_installed(&req, &policy(selection)?)
                    .unwrap_or(false);
            if installed && record.phase == "installing" {
                record.phase = "installed".into();
                grants::save_install(store, &id, &record)?;
            }
            installed
        }
        _ => false,
    };
    let (enabled, source_ids, _) = grants::grant(store, &id)?;
    let permissions = permissions::get(store, &id)?;
    let sources:Vec<_> = store
        .list_sources()?
        .into_iter()
        .filter(|s| s.enabled)
        .collect();
    let editable_source_ids=sources.iter().filter_map(|s|documents::editable(store,&s.id).ok().filter(|v|*v).map(|_|s.id.clone())).collect();
    Ok(State {
        installed,
        enabled,
        source_ids,
        sources,
        reporting_project_ids: permissions.projects,
        edit_source_ids: permissions.edits,
        editable_source_ids,
        connected: installed && enabled && permissions.last_seen_ms.is_some(),
        last_seen_ms: permissions.last_seen_ms,
    })
}
pub fn execute(store: &mut ReaderStore, directory: &Path, action: Action) -> Result<State> {
    match action {
        Action::State { selection } => state(store, directory, &selection),
        Action::Install {
            selection,
            executable,
            client_version,
        } => {
            selection.checked()?;
            if !executable.is_absolute()
                || client_version.is_empty()
                || client_version.len() > 128
                || client_version.chars().any(char::is_control)
            {
                return Err(invalid());
            }
            let parent = SourceRoot::capture(executable.parent().ok_or_else(invalid)?.to_owned())?;
            let file = parent
                .open_optional_file_relative(Path::new(
                    executable.file_name().ok_or_else(invalid)?,
                ))?
                .ok_or_else(invalid)?;
            if !file.metadata()?.is_file() {
                return Err(invalid());
            }
            let id = selection.id()?;
            let previous = grants::record(store, &id)?;
            let skill_conflict = previous
                .as_ref()
                .is_some_and(|record| record.skill_conflict);
            if skill_conflict {
                return Err(ReaderError::Conflict);
            }
            assets::check_compatible(&selection,previous.as_ref().is_some_and(Record::owns_skill))?;
            // A new or removed record cannot claim a preexisting external skill.
            // Persist this conservative intent before configuration changes. If
            // interrupted after exclusive creation, a retry may retain the copy
            // rather than infer ownership from identical bytes alone.
            let skill_owned = previous
                .as_ref()
                .is_some_and(|record| record.phase != "removed" && record.owns_skill());
            let record = Record {
                selection: selection.clone(),
                executable,
                client_version,
                phase: "installing".into(),
                skill_conflict,
                skill_owned: Some(skill_owned),
            };
            // A failed retry must not demote a valid completed installation.
            if previous.as_ref().is_none_or(|p| p.phase != "installed") {
                grants::save_install(store, &id, &record)?;
            }
            // Disabled policy supports receipt-backed configuration; it never enables capture.
            super::capture::configure(store, directory, policy(&selection)?)?;
            let req = request(directory, &record)?;
            let mut installer = installer(directory)?;
            let prepared = match installer.prepare(&req, &policy(&selection)?) {
                Ok(prepared) => prepared,
                Err(mdreader_integrations::IntegrationError::Conflict) => {
                    installer
                        .refresh_launcher(&req, &policy(&selection)?)
                        .map_err(integration_error)?;
                    installer
                        .prepare(&req, &policy(&selection)?)
                        .map_err(integration_error)?
                }
                Err(error) => return Err(integration_error(error)),
            };
            installer
                .apply(prepared, &policy(&selection)?)
                .map_err(integration_error)?;
            hooks::install(directory, &record)?;
            let skill_owned = assets::install(&selection, record.owns_skill())?;
            grants::save_install(
                store,
                &id,
                &Record {
                    phase: "installed".into(),
                    skill_owned: Some(skill_owned),
                    ..record
                },
            )?;
            state(store, directory, &selection)
        }
        Action::SetMcp {
            selection,
            sources,
            enabled,
        } => {
            // Revocation remains available after owned config/skill edits.
            if !enabled {
                selection.checked()?;
                grants::set(store, &selection.id()?, false, &[])?;
                return state(store, directory, &selection);
            }
            let current = state(store, directory, &selection)?;
            if enabled && !current.installed {
                return Err(super::protocol_error("Install the agent integration first"));
            }
            if enabled
                && (sources.is_empty()
                    || sources.len() > 32
                    || sources
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != sources.len()
                    || sources
                        .iter()
                        .any(|id| !current.sources.iter().any(|s| &s.id == id)))
            {
                return Err(invalid());
            }
            if enabled {
                store.index_status(&sources)?;
            }
            grants::set(
                store,
                &selection.id()?,
                enabled,
                if enabled { &sources } else { &[] },
            )?;
            state(store, directory, &selection)
        }
        Action::SetPermissions {
            selection,
            enabled,
            project_ids,
            read_source_ids,
            edit_source_ids,
        } => {
            selection.checked()?;
            let current = state(store, directory, &selection)?;
            if enabled && !current.installed {
                return Err(super::protocol_error("Install the agent integration first"));
            }
            let valid = |values: &[String]| {
                values.len() <= 32
                    && values
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        == values.len()
                    && values
                        .iter()
                        .all(|id| current.sources.iter().any(|s| &s.id == id))
            };
            if enabled
                && (!valid(&project_ids) || !valid(&read_source_ids) || !valid(&edit_source_ids))
            {
                return Err(invalid());
            }
            if enabled {
                for id in &project_ids {
                    store.chosen_local_root(id)?.verify()?;
                    if !store.folder_index_status(id)?.monitor_enabled {
                        return Err(invalid());
                    }
                }
                for id in &edit_source_ids {if !documents::editable(store,id)?{return Err(invalid());}}
            }
            permissions::set(
                store,
                &selection.id()?,
                enabled,
                if enabled { &project_ids } else { &[] },
                if enabled { &read_source_ids } else { &[] },
                if enabled { &edit_source_ids } else { &[] },
            )?;
            state(store, directory, &selection)
        }
        Action::Uninstall { selection } => {
            selection.checked()?;
            let id = selection.id()?;
            grants::set(store, &id, false, &[])?;
            if let Some(mut record) = grants::record(store, &id)? {
                let skill_owned = record.owns_skill();
                record.skill_owned = Some(skill_owned);
                record.phase = "removing".into();
                grants::save_install(store, &id, &record)?;
                installer(directory)?
                    .uninstall(&request(directory, &record)?)
                    .map_err(integration_error)?;
                hooks::uninstall(directory, &record)?;
                if skill_owned {
                    assets::uninstall(&selection)?;
                }
                record.phase = "removed".into();
                grants::save_install(store, &id, &record)?;
            }
            state(store, directory, &selection)
        }
    }
}

pub(crate) fn dispatch(
    runtime: &crate::runtime::Runtime,
    id: &str,
    request: projects::AgentRequest,
) -> Result<serde_json::Value> {
    if matches!(
        request,
        projects::AgentRequest::CreateDocument { .. }
            | projects::AgentRequest::UpdateDocument { .. }
            | projects::AgentRequest::InspectOperation { .. }
    ) {
        documents::execute(runtime, id, request)
    } else {
        projects::report(&mut *runtime.store()?, id, request)
    }
}
#[cfg(test)]
mod tests;
