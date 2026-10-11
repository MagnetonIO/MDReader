//! Explicit, project-scoped configuration edits. Never discovers user configuration.
use mdreader_capture::ProjectPolicy;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;
mod admission;
mod config;
mod durable;
mod json;
#[cfg(unix)]
mod unixfs;
#[cfg(windows)]
mod windows;
pub use durable::{Installer, PreparedChange};
#[derive(Debug, Error)]
pub enum IntegrationError {
    #[error("integration state is not ready")]
    NotReady,
    #[error("integration input or schema is invalid")]
    Invalid,
    #[error("integration conflicts with existing settings or subsequent edits")]
    Conflict,
    #[error("integration project is not opted in")]
    OptOut,
    #[error("integration capability or host is unsupported")]
    Unsupported,
    #[error("integration bound exceeded")]
    Bounds,
    #[error("integration state is busy")]
    Busy,
    #[cfg(windows)]
    #[error("integration database persistence failed")]
    Database(#[from] rusqlite::Error),
    #[error("integration persistence failed")]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, IntegrationError>;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntegrationKind {
    ClaudeHooks,
    ClaudeMcp,
    ClaudeUserMcp,
    CodexHooks,
    CodexMcp,
    CodexNotify,
}
#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    pub local_commands: bool,
    pub claude_exec_hooks: bool,
    pub codex_lifecycle_hooks: bool,
    pub codex_notify: bool,
    pub client_version: String,
}
#[derive(Debug, Clone)]
pub struct InstallRequest {
    pub kind: IntegrationKind,
    pub worktree_root: PathBuf,
    pub executable: PathBuf,
    pub data_dir: PathBuf,
    /// Explicit service-owned artifact; installer never publishes policy authority.
    pub policy_path: PathBuf,
    pub spool_root: PathBuf,
    pub source_ids: Vec<String>,
    /// Service-owned, revocable MCP scope; installation alone grants no reads.
    pub mcp_grant_id: Option<String>,
    pub capabilities: Capabilities,
}
impl IntegrationKind {
    pub fn config_path(self, root: &Path) -> PathBuf {
        root.join(match self {
            Self::ClaudeHooks => ".claude/settings.local.json",
            Self::ClaudeMcp => ".mcp.json",
            Self::ClaudeUserMcp => ".claude.json",
            Self::CodexHooks => ".codex/hooks.json",
            Self::CodexMcp | Self::CodexNotify => ".codex/config.toml",
        })
    }
}
