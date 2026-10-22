//! Agent project choices and shared agent-report validation. Session rows now
//! live in `agent_activity`. The prior project-scoped `reader_agent_sessions`
//! tables are no longer read or written here; `agent_activity` imports them
//! once. Removal gate: a later migration drops those tables after one release.
use crate::{ReaderError, ReaderStore, Result};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub monitoring: bool,
}
pub fn now_ms() -> Result<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ReaderError::Unavailable)?
        .as_millis()
        .try_into()
        .map_err(|_| ReaderError::Unavailable)
}
fn label(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
pub fn markdown_path(path: &str) -> bool {
    label(path, 4096)
        && !path.contains(['\\', ':'])
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        && Path::new(path)
            .extension()
            .and_then(|v| v.to_str())
            .is_some_and(|v| {
                matches!(
                    v.to_ascii_lowercase().as_str(),
                    "md" | "markdown" | "mdown" | "mkd" | "mkdn"
                )
            })
}
impl ReaderStore {
    pub fn agent_projects(&self) -> Result<Vec<Project>> {
        self.list_sources()?
            .into_iter()
            .filter(|s| self.chosen_local_root(&s.id).is_ok())
            .map(|s| {
                Ok(Project {
                    id: s.id.clone(),
                    name: s.name,
                    enabled: s.enabled,
                    monitoring: self.folder_index_status(&s.id)?.monitor_enabled,
                })
            })
            .collect()
    }
}
