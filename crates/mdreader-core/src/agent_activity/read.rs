//! Listings for the sidebar: sessions with at least one file, newest first.
use super::{clock, schema, Agent, FileOp, SessionSummary, TitleSource, MAX_PAGE};
use crate::{ReaderError, ReaderStore, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Files listed per session are bounded; the newest are kept.
const MAX_LISTED_FILES: i64 = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub sessions: Vec<SessionSummary>,
    pub next_offset: Option<u32>,
}

/// A recorded file as stored; existence and openability are resolved later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileState {
    pub abs_path: String,
    pub exact_id: String,
    pub op: FileOp,
    pub first_at_ms: i64,
    pub last_at_ms: i64,
}

fn like(value: &str) -> String {
    let mut out = String::from("%");
    for c in value.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// `<cwd basename> · <date>` for sessions without any recorded title.
fn fallback_title(agent: Agent, cwd: Option<&str>, started_ms: i64) -> String {
    let base = cwd
        .and_then(|c| Path::new(c).file_name())
        .and_then(|n| n.to_str())
        .unwrap_or(match agent {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
        });
    format!("{base} · {}", clock::utc_date(started_ms))
}

impl ReaderStore {
    pub fn agent_activity_page(
        &mut self,
        agent: Option<Agent>,
        offset: u32,
        limit: u32,
        filter: Option<&str>,
    ) -> Result<Page> {
        schema::initialize(self)?;
        let limit = limit.clamp(1, MAX_PAGE);
        let filter = filter
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(|f| like(&f.chars().take(256).collect::<String>()));
        let rows = self.with_library_connection(|db| -> Result<Vec<_>> {
            let rows = db
                .prepare(
                    "SELECT s.id,s.agent,s.title,s.title_source,s.cwd,s.git_branch,s.started_at_ms,s.last_activity_ms,s.ended_at_ms,count(*) \
                     FROM reader_agent_activity_sessions s JOIN reader_agent_activity_files f ON f.session_id=s.id \
                     WHERE s.hidden=0 AND (?1 IS NULL OR s.agent=?1) \
                     AND (?2 IS NULL OR s.title LIKE ?2 ESCAPE '\\' OR s.cwd LIKE ?2 ESCAPE '\\' OR EXISTS(SELECT 1 FROM reader_agent_activity_files g WHERE g.session_id=s.id AND g.name LIKE ?2 ESCAPE '\\')) \
                     GROUP BY s.id ORDER BY s.last_activity_ms DESC,s.id LIMIT ?3 OFFSET ?4",
                )?
                .query_map(
                    params![agent.map(Agent::as_str), filter, i64::from(limit) + 1, i64::from(offset)],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, Option<String>>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, Option<String>>(4)?,
                            r.get::<_, Option<String>>(5)?,
                            r.get::<_, i64>(6)?,
                            r.get::<_, i64>(7)?,
                            r.get::<_, Option<i64>>(8)?,
                            r.get::<_, u32>(9)?,
                        ))
                    },
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        let more = rows.len() > limit as usize;
        let sessions = rows
            .into_iter()
            .take(limit as usize)
            .map(
                |(id, agent, title, source, cwd, git_branch, started, last, ended, files)| {
                    let agent = Agent::parse(&agent).ok_or(ReaderError::Unavailable)?;
                    let title_source =
                        TitleSource::parse(&source).ok_or(ReaderError::Unavailable)?;
                    Ok(SessionSummary {
                        title: title
                            .unwrap_or_else(|| fallback_title(agent, cwd.as_deref(), started)),
                        id,
                        agent,
                        title_source,
                        cwd,
                        git_branch,
                        started_at_ms: started,
                        last_activity_ms: last,
                        ended_at_ms: ended,
                        file_count: files,
                    })
                },
            )
            .collect::<Result<Vec<_>>>()?;
        Ok(Page {
            sessions,
            next_offset: more.then(|| offset.saturating_add(limit)),
        })
    }

    /// The session's working directory and its recorded files, newest first.
    pub fn agent_activity_files(
        &mut self,
        session: &str,
    ) -> Result<(Option<String>, Vec<FileState>)> {
        schema::initialize(self)?;
        self.with_library_connection(|db| {
            let cwd: Option<String> = db
                .query_row(
                    "SELECT cwd FROM reader_agent_activity_sessions WHERE id=?1 AND hidden=0",
                    [session],
                    |r| r.get(0),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => ReaderError::Unavailable,
                    e => e.into(),
                })?;
            let files = db
                .prepare("SELECT abs_path,exact_id,op,first_at_ms,last_at_ms FROM reader_agent_activity_files WHERE session_id=?1 ORDER BY last_at_ms DESC,abs_path LIMIT ?2")?
                .query_map(params![session, MAX_LISTED_FILES], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
                .into_iter()
                .map(|(abs_path, exact_id, op, first, last)| {
                    Ok(FileState {
                        abs_path,
                        exact_id,
                        op: FileOp::parse(&op).ok_or(ReaderError::Unavailable)?,
                        first_at_ms: first,
                        last_at_ms: last,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok((cwd, files))
        })
    }

    /// The client's own id and working directory for an activity session
    /// of `agent`; unavailable for another agent's session.
    pub fn agent_session_ref(
        &mut self,
        agent: Agent,
        session: &str,
    ) -> Result<(String, Option<String>)> {
        schema::initialize(self)?;
        self.with_library_connection(|db| {
            db.query_row(
                "SELECT client_session_id,cwd FROM reader_agent_activity_sessions WHERE id=?1 AND agent=?2",
                params![session, agent.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => ReaderError::Unavailable,
                e => e.into(),
            })
        })
    }

    /// The activity id for a client's own session id, if recorded.
    pub fn agent_session_id(
        &mut self,
        agent: Agent,
        client_session_id: &str,
    ) -> Result<Option<String>> {
        schema::initialize(self)?;
        self.with_library_connection(|db| {
            Ok(db
                .query_row(
                    "SELECT id FROM reader_agent_activity_sessions WHERE agent=?1 AND client_session_id=?2",
                    params![agent.as_str(), client_session_id],
                    |r| r.get(0),
                )
                .optional()?)
        })
    }
}
