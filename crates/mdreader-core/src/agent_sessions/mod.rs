//! Project-scoped session rows from before `agent_activity`. Read-only: hooks
//! and MCP now write `agent_activity`, which imported these rows. Removal gate:
//! drop these tables in a later migration once one release has shipped reading
//! them only for that import.
mod schema;
use crate::{ReaderError, ReaderStore, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub monitoring: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub project_id: String,
    pub agent: String,
    pub title: String,
    pub started_at_ms: i64,
    pub last_seen_ms: i64,
    pub ended_at_ms: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDocument {
    pub source_id: String,
    pub path: String,
    pub title: String,
    pub revision: String,
    pub session_ids: Vec<String>,
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
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        id: r.get(0)?,
        project_id: r.get(1)?,
        agent: r.get(2)?,
        title: r.get(3)?,
        started_at_ms: r.get(4)?,
        last_seen_ms: r.get(5)?,
        ended_at_ms: r.get(6)?,
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
    pub fn agent_sessions(&mut self, project: &str) -> Result<Vec<Session>> {
        self.chosen_local_root(project)?.verify()?;
        schema::initialize(self)?;
        self.with_library_connection(|db| {
            Ok(db.prepare("SELECT id,project_id,agent,title,started_at_ms,last_seen_ms,ended_at_ms FROM reader_agent_sessions WHERE project_id=?1 ORDER BY last_seen_ms DESC,id LIMIT 256")?.query_map([project],row)?.collect::<std::result::Result<_,_>>()?)
        })
    }
    pub fn agent_documents(
        &mut self,
        project: &str,
        session: Option<&str>,
    ) -> Result<Vec<SessionDocument>> {
        self.chosen_local_root(project)?.verify()?;
        schema::initialize(self)?;
        if let Some(session) = session {
            let exists:bool=self.with_library_connection(|db|db.query_row("SELECT EXISTS(SELECT 1 FROM reader_agent_sessions WHERE id=?1 AND project_id=?2)",params![session,project],|r|r.get(0)))?;
            if !exists {
                return Err(ReaderError::Unavailable);
            }
        }
        let documents = self.list_documents(project)?;
        let mut result = Vec::new();
        for d in documents {
            let ids:Vec<String>=self.with_library_connection(|db| ->Result<_> {Ok(db.prepare("SELECT d.session_id FROM reader_agent_documents d JOIN reader_agent_sessions s ON s.id=d.session_id WHERE s.project_id=?1 AND d.path=?2 ORDER BY s.last_seen_ms DESC,s.id")?.query_map(params![project,d.path],|r|r.get(0))?.collect::<std::result::Result<_,_>>()?)})?;
            if session.is_none_or(|s| ids.iter().any(|id| id == s)) {
                let title = Path::new(&d.path)
                    .file_name()
                    .and_then(|v| v.to_str())
                    .unwrap_or(&d.path)
                    .to_owned();
                result.push(SessionDocument {
                    source_id: project.into(),
                    path: d.path,
                    title,
                    revision: d.revision,
                    session_ids: ids,
                });
            }
        }
        result.extend(self.agent_source_documents(project, session)?);
        Ok(result)
    }
    /// Files from other sources linked to this project's sessions. A source
    /// that is no longer chosen contributes nothing rather than failing.
    fn agent_source_documents(
        &mut self,
        project: &str,
        session: Option<&str>,
    ) -> Result<Vec<SessionDocument>> {
        let links:Vec<(String,String,String)>=self.with_library_connection(|db| ->Result<_> {Ok(db.prepare("SELECT d.source_id,d.path,d.session_id FROM reader_agent_source_documents d JOIN reader_agent_sessions s ON s.id=d.session_id WHERE s.project_id=?1 ORDER BY s.last_seen_ms DESC,s.id")?.query_map([project],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect::<std::result::Result<_,_>>()?)})?;
        let mut grouped: Vec<(String, String, Vec<String>)> = Vec::new();
        for (source, path, id) in links {
            match grouped
                .iter_mut()
                .find(|(s, p, _)| *s == source && *p == path)
            {
                Some((_, _, ids)) => ids.push(id),
                None => grouped.push((source, path, vec![id])),
            }
        }
        let mut result = Vec::new();
        let mut documents: std::collections::HashMap<String, Vec<crate::ListedDocument>> =
            std::collections::HashMap::new();
        for (source, path, ids) in grouped {
            if session.is_some_and(|s| !ids.iter().any(|id| id == s)) {
                continue;
            }
            if !documents.contains_key(&source) {
                let listed = if self.chosen_local_root(&source).is_ok() {
                    self.list_documents(&source).unwrap_or_default()
                } else {
                    Vec::new()
                };
                documents.insert(source.clone(), listed);
            }
            let Some(d) = documents[&source].iter().find(|d| d.path == path) else {
                continue;
            };
            let title = Path::new(&d.path)
                .file_name()
                .and_then(|v| v.to_str())
                .unwrap_or(&d.path)
                .to_owned();
            result.push(SessionDocument {
                source_id: source,
                path,
                title,
                revision: d.revision.clone(),
                session_ids: ids,
            });
        }
        Ok(result)
    }
}
#[cfg(test)]
mod tests;
