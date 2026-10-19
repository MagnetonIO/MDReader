//! Durable metadata and file associations for explicitly chosen projects.
//! This namespace contains neither chat histories nor document bodies.
mod schema;
use crate::{ReaderError, ReaderStore, Result};
use rusqlite::{params, OptionalExtension};
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
    pub fn register_agent_session(
        &mut self,
        project: &str,
        connection: &str,
        client: &str,
        agent: &str,
        title: &str,
    ) -> Result<Session> {
        let root = self.chosen_local_root(project)?;
        if !self.folder_index_status(project)?.monitor_enabled
            || !label(client, 256)
            || !label(title, 256)
            || !matches!(agent, "claude" | "codex")
            || uuid::Uuid::parse_str(connection).is_err()
        {
            return Err(ReaderError::Unavailable);
        }
        schema::initialize(self)?;
        let now = now_ms()?;
        let result=self.with_library_connection(|db|->Result<Session>{
            let tx=db.transaction()?;
            let count:usize=tx.query_row("SELECT count(*) FROM reader_agent_sessions",[],|r|r.get(0))?;
            if count>=10000 && !tx.query_row("SELECT EXISTS(SELECT 1 FROM reader_agent_sessions WHERE project_id=?1 AND connection_id=?2 AND client_session_id=?3)",params![project,connection,client],|r|r.get::<_,bool>(0))? {return Err(ReaderError::Unavailable);}
            tx.execute("INSERT INTO reader_agent_sessions VALUES(?1,?2,?3,?4,?5,?6,?7,?7,NULL) ON CONFLICT(project_id,connection_id,client_session_id) DO UPDATE SET title=excluded.title,last_seen_ms=excluded.last_seen_ms,ended_at_ms=NULL",params![uuid::Uuid::new_v4().to_string(),project,connection,client,agent,title,now])?;
            let result=tx.query_row("SELECT id,project_id,agent,title,started_at_ms,last_seen_ms,ended_at_ms FROM reader_agent_sessions WHERE project_id=?1 AND connection_id=?2 AND client_session_id=?3",params![project,connection,client],row)?;
            root.verify()?;
            tx.commit()?;
            Ok(result)
        })?;
        Ok(result)
    }
    pub fn agent_session_project(&mut self, connection: &str, session: &str) -> Result<String> {
        schema::initialize(self)?;
        self.with_library_connection(|db| {
            db.query_row(
                "SELECT project_id FROM reader_agent_sessions WHERE id=?1 AND connection_id=?2",
                params![session, connection],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(ReaderError::Unavailable)
        })
    }
    pub fn register_agent_document(
        &mut self,
        connection: &str,
        session: &str,
        path: &str,
    ) -> Result<()> {
        let project = self.agent_session_project(connection, session)?;
        let root = self.chosen_local_root(&project)?;
        if !self.folder_index_status(&project)?.monitor_enabled || !markdown_path(path) {
            return Err(ReaderError::Unavailable);
        }
        let file = root
            .open_optional_file_relative(Path::new(path))?
            .ok_or(ReaderError::Unavailable)?;
        crate::source_root::file_stamp::single_link_stamp(&file)?;
        let now = now_ms()?;
        self.with_library_connection(|db|->Result<()> {
            let tx=db.transaction()?;
            let count:usize=tx.query_row("SELECT count(*) FROM reader_agent_documents",[],|r|r.get(0))?;
            if count>=100000 && !tx.query_row("SELECT EXISTS(SELECT 1 FROM reader_agent_documents WHERE session_id=?1 AND path=?2)",params![session,path],|r|r.get::<_,bool>(0))?{return Err(ReaderError::Unavailable);}
            tx.execute("INSERT INTO reader_agent_documents VALUES(?1,?2,?3) ON CONFLICT(session_id,path) DO UPDATE SET reported_at_ms=excluded.reported_at_ms",params![session,path,now])?;
            tx.execute("UPDATE reader_agent_sessions SET last_seen_ms=?2 WHERE id=?1",params![session,now])?;
            root.verify()?;
            crate::source_root::file_stamp::single_link_stamp(&root.open_optional_file_relative(Path::new(path))?.ok_or(ReaderError::Unavailable)?)?;
            tx.commit()?;
            Ok(())
        })?;
        self.queue_folder_changes(&project, &[path.into()])
    }
    /// Associate a Markdown file in another chosen source, such as the Claude
    /// Code plans folder, with a session of a still-reporting project.
    pub fn register_agent_source_document(
        &mut self,
        connection: &str,
        session: &str,
        source: &str,
        path: &str,
    ) -> Result<()> {
        let project = self.agent_session_project(connection, session)?;
        if source == project {
            return self.register_agent_document(connection, session, path);
        }
        self.chosen_local_root(&project)?;
        let root = self.chosen_local_root(source)?;
        if !self.folder_index_status(&project)?.monitor_enabled
            || !self.folder_index_status(source)?.monitor_enabled
            || !markdown_path(path)
        {
            return Err(ReaderError::Unavailable);
        }
        let file = root
            .open_optional_file_relative(Path::new(path))?
            .ok_or(ReaderError::Unavailable)?;
        crate::source_root::file_stamp::single_link_stamp(&file)?;
        let now = now_ms()?;
        self.with_library_connection(|db|->Result<()> {
            let tx=db.transaction()?;
            let count:usize=tx.query_row("SELECT count(*) FROM reader_agent_source_documents",[],|r|r.get(0))?;
            if count>=100000 && !tx.query_row("SELECT EXISTS(SELECT 1 FROM reader_agent_source_documents WHERE session_id=?1 AND source_id=?2 AND path=?3)",params![session,source,path],|r|r.get::<_,bool>(0))?{return Err(ReaderError::Unavailable);}
            tx.execute("INSERT INTO reader_agent_source_documents VALUES(?1,?2,?3,?4) ON CONFLICT(session_id,source_id,path) DO UPDATE SET reported_at_ms=excluded.reported_at_ms",params![session,source,path,now])?;
            tx.execute("UPDATE reader_agent_sessions SET last_seen_ms=?2 WHERE id=?1",params![session,now])?;
            root.verify()?;
            crate::source_root::file_stamp::single_link_stamp(&root.open_optional_file_relative(Path::new(path))?.ok_or(ReaderError::Unavailable)?)?;
            tx.commit()?;
            Ok(())
        })?;
        self.queue_folder_changes(source, &[path.into()])
    }
    pub fn end_agent_session(&mut self, connection: &str, session: &str) -> Result<()> {
        let project = self.agent_session_project(connection, session)?;
        self.chosen_local_root(&project)?.verify()?;
        let now = now_ms()?;
        self.with_library_connection(|db| {db.execute("UPDATE reader_agent_sessions SET ended_at_ms=COALESCE(ended_at_ms,?2),last_seen_ms=?2 WHERE id=?1",params![session,now])?;Ok(())})
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
