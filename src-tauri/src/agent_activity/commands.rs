//! Renderer commands for the Agent Sessions sidebar. Each forwards to the
//! service owner and converts its replies into camelCase DTOs.
use super::ActivityWatch;
use crate::reader::ReaderBackend;
use mdreader_core::agent_activity as core;
use mdreader_service::{
    agents::activity::{ActivityFile, ActivityRequest, TrackingState},
    Request,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Claude,
    Codex,
}
impl From<AgentKind> for core::Agent {
    fn from(value: AgentKind) -> Self {
        match value {
            AgentKind::Claude => core::Agent::Claude,
            AgentKind::Codex => core::Agent::Codex,
        }
    }
}
impl From<core::Agent> for AgentKind {
    fn from(value: core::Agent) -> Self {
        match value {
            core::Agent::Claude => AgentKind::Claude,
            core::Agent::Codex => AgentKind::Codex,
        }
    }
}

/// Which source won the session title (highest first: user, custom, ai,
/// prompt, fallback).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AgentTitleSource {
    User,
    Custom,
    Ai,
    Prompt,
    Fallback,
}
impl From<core::TitleSource> for AgentTitleSource {
    fn from(value: core::TitleSource) -> Self {
        match value {
            core::TitleSource::User => Self::User,
            core::TitleSource::Custom => Self::Custom,
            core::TitleSource::Ai => Self::Ai,
            core::TitleSource::Prompt => Self::Prompt,
            core::TitleSource::Fallback => Self::Fallback,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AgentFileOp {
    Create,
    Edit,
}

/// Why a listed file has no reference: it is gone, in a private location
/// (only the Claude Code plans folder is open), or not readable Markdown.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AgentFileUnavailable {
    Missing,
    Private,
    NotMarkdown,
    TooLarge,
    Unreadable,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivitySession {
    pub id: String,
    pub agent: AgentKind,
    pub title: String,
    pub title_source: AgentTitleSource,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub started_at_ms: f64,
    pub last_activity_ms: f64,
    pub ended_at_ms: Option<f64>,
    pub file_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivityPage {
    /// Sessions with at least one file, most recent activity first.
    pub sessions: Vec<AgentActivitySession>,
    pub next_offset: Option<u32>,
    /// Listed sessions per agent under the same filter, across all pages.
    pub totals: AgentActivityTotals,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivityTotals {
    pub claude: u32,
    pub codex: u32,
}

/// An exact catalog reference: open with
/// `catalogReference({ kind: 'exact', ...reference }, name)`.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileReference {
    pub source_id: String,
    pub path: String,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivityFile {
    pub abs_path: String,
    /// Relative to the session's working directory when inside it.
    pub display_path: String,
    pub op: AgentFileOp,
    pub first_at_ms: f64,
    pub last_at_ms: f64,
    pub exists: bool,
    /// In this machine's Claude Code plans folder.
    pub is_plan: bool,
    pub reference: Option<AgentFileReference>,
    pub reason: Option<AgentFileUnavailable>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentTrackingState {
    pub agent: AgentKind,
    /// "Track sessions" switch; off until the user enables it.
    pub enabled: bool,
    /// The agent's log folder exists on this machine.
    pub available: bool,
    /// Logs older than this are not read until "load older".
    pub since_ms: Option<f64>,
    pub sessions: u32,
}

fn ms(value: i64) -> f64 {
    value as f64
}

impl From<core::SessionSummary> for AgentActivitySession {
    fn from(s: core::SessionSummary) -> Self {
        Self {
            id: s.id,
            agent: s.agent.into(),
            title: s.title,
            title_source: s.title_source.into(),
            cwd: s.cwd,
            git_branch: s.git_branch,
            started_at_ms: ms(s.started_at_ms),
            last_activity_ms: ms(s.last_activity_ms),
            ended_at_ms: s.ended_at_ms.map(ms),
            file_count: s.file_count,
        }
    }
}
impl From<ActivityFile> for AgentActivityFile {
    fn from(f: ActivityFile) -> Self {
        Self {
            abs_path: f.abs_path,
            display_path: f.display_path,
            op: match f.op {
                core::FileOp::Create => AgentFileOp::Create,
                core::FileOp::Edit => AgentFileOp::Edit,
            },
            first_at_ms: ms(f.first_at_ms),
            last_at_ms: ms(f.last_at_ms),
            exists: f.exists,
            is_plan: f.is_plan,
            reference: f.reference.map(|r| AgentFileReference {
                source_id: r.source_id,
                path: r.path,
                revision: r.revision,
            }),
            reason: f.unavailable.map(|u| match u {
                core::FileUnavailable::Missing => AgentFileUnavailable::Missing,
                core::FileUnavailable::Private => AgentFileUnavailable::Private,
                core::FileUnavailable::NotMarkdown => AgentFileUnavailable::NotMarkdown,
                core::FileUnavailable::TooLarge => AgentFileUnavailable::TooLarge,
                core::FileUnavailable::Unreadable => AgentFileUnavailable::Unreadable,
            }),
        }
    }
}
impl From<TrackingState> for AgentTrackingState {
    fn from(t: TrackingState) -> Self {
        Self {
            agent: t.agent.into(),
            enabled: t.enabled,
            available: t.available,
            since_ms: t.since_ms.map(ms),
            sessions: t.sessions,
        }
    }
}

async fn activity<T: serde::de::DeserializeOwned + Send + 'static>(
    reader: &ReaderBackend,
    request: ActivityRequest,
) -> Result<T, String> {
    reader.invoke(Request::AgentActivity { request }).await
}

async fn tracking(
    reader: &ReaderBackend,
    watch: &ActivityWatch,
    request: ActivityRequest,
) -> Result<Vec<AgentTrackingState>, String> {
    let state: Vec<TrackingState> = activity(reader, request).await?;
    // Re-arm watches and rescan the history window for the new state.
    watch.kick();
    Ok(state.into_iter().map(Into::into).collect())
}

#[tauri::command]
#[specta::specta]
/// `utc_offset_minutes` is the viewer's offset east of UTC (JS:
/// `-new Date().getTimezoneOffset()`), so fallback titles show the local date.
pub async fn agent_activity_list(
    agent: Option<AgentKind>,
    offset: u32,
    limit: u32,
    filter: Option<String>,
    utc_offset_minutes: Option<i32>,
    reader: State<'_, ReaderBackend>,
) -> Result<AgentActivityPage, String> {
    let page: core::Page = activity(
        &reader,
        ActivityRequest::List {
            agent: agent.map(Into::into),
            offset,
            limit,
            filter,
            utc_offset_minutes,
        },
    )
    .await?;
    Ok(AgentActivityPage {
        sessions: page.sessions.into_iter().map(Into::into).collect(),
        next_offset: page.next_offset,
        totals: AgentActivityTotals {
            claude: page.totals.claude,
            codex: page.totals.codex,
        },
    })
}

#[tauri::command]
#[specta::specta]
pub async fn agent_activity_files(
    session_id: String,
    reader: State<'_, ReaderBackend>,
) -> Result<Vec<AgentActivityFile>, String> {
    let files: Vec<ActivityFile> = activity(&reader, ActivityRequest::Files { session_id }).await?;
    Ok(files.into_iter().map(Into::into).collect())
}

/// A user title wins over every automatic one; an empty title reverts to
/// automatic naming.
#[tauri::command]
#[specta::specta]
pub async fn agent_activity_rename(
    session_id: String,
    title: String,
    reader: State<'_, ReaderBackend>,
) -> Result<(), String> {
    activity::<serde_json::Value>(&reader, ActivityRequest::Rename { session_id, title })
        .await
        .map(drop)
}

#[tauri::command]
#[specta::specta]
pub async fn agent_activity_hide(
    session_id: String,
    reader: State<'_, ReaderBackend>,
) -> Result<(), String> {
    activity::<serde_json::Value>(&reader, ActivityRequest::Hide { session_id })
        .await
        .map(drop)
}

/// Open the session's recorded working directory in Finder/Explorer. Keyed by
/// session so the renderer never names a path.
#[tauri::command]
#[specta::specta]
pub async fn agent_activity_reveal_cwd(
    session_id: String,
    reader: State<'_, ReaderBackend>,
) -> Result<(), String> {
    let cwd: Option<String> = activity(&reader, ActivityRequest::SessionCwd { session_id }).await?;
    let cwd = std::path::PathBuf::from(cwd.ok_or("This session has no recorded folder")?);
    if !cwd.is_absolute() || !cwd.is_dir() {
        return Err("The session's folder no longer exists".into());
    }
    tauri_plugin_opener::open_path(&cwd, None::<&str>)
        .map_err(|_| "The folder could not be opened".into())
}

#[tauri::command]
#[specta::specta]
pub async fn agent_activity_tracking_state(
    reader: State<'_, ReaderBackend>,
) -> Result<Vec<AgentTrackingState>, String> {
    let state: Vec<TrackingState> = activity(&reader, ActivityRequest::Tracking).await?;
    Ok(state.into_iter().map(Into::into).collect())
}

#[tauri::command]
#[specta::specta]
pub async fn set_agent_activity_tracking(
    agent: AgentKind,
    enabled: bool,
    reader: State<'_, ReaderBackend>,
    watch: State<'_, ActivityWatch>,
) -> Result<Vec<AgentTrackingState>, String> {
    tracking(
        &reader,
        &watch,
        ActivityRequest::SetTracking {
            agent: agent.into(),
            enabled,
        },
    )
    .await
}

/// Read enabled agents' logs back to `days` ago (the first run reads 30).
#[tauri::command]
#[specta::specta]
pub async fn agent_activity_load_older(
    days: u32,
    reader: State<'_, ReaderBackend>,
    watch: State<'_, ActivityWatch>,
) -> Result<Vec<AgentTrackingState>, String> {
    tracking(&reader, &watch, ActivityRequest::LoadOlder { days }).await
}

/// "Remove tracked sessions": forget one agent's sessions and read offsets.
/// Transcripts and the Markdown files themselves are untouched.
#[tauri::command]
#[specta::specta]
pub async fn agent_activity_forget(
    agent: AgentKind,
    reader: State<'_, ReaderBackend>,
    watch: State<'_, ActivityWatch>,
) -> Result<Vec<AgentTrackingState>, String> {
    tracking(
        &reader,
        &watch,
        ActivityRequest::Forget {
            agent: agent.into(),
        },
    )
    .await
}
