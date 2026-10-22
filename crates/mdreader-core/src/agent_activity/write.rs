//! Durable writes: observations from logs, hooks and MCP, plus user edits.
use super::{
    exact, schema, Agent, Cursor, SessionObservation, TitleSource, MAX_FILES_PER_SESSION, MAX_PATH,
    MAX_SESSIONS,
};
use crate::{ReaderError, ReaderStore, Result};
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::BTreeSet;

/// Activity this recent counts as a visible change, so "live" stays current
/// without emitting an event for every transcript line.
const ACTIVITY_STEP_MS: i64 = 15_000;

fn bump(tx: &Transaction<'_>) -> Result<()> {
    tx.execute(
        "UPDATE reader_agent_activity_meta SET revision=revision+1 WHERE id=1",
        [],
    )?;
    Ok(())
}

fn valid_session_id(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

/// Apply one observation; returns the session id when anything visible changed.
fn apply(tx: &Transaction<'_>, o: &SessionObservation) -> Result<Option<String>> {
    if !valid_session_id(&o.client_session_id) {
        return Err(ReaderError::Invalid("invalid agent session id".into()));
    }
    let cwd = o.cwd.as_deref().filter(|c| c.len() <= MAX_PATH);
    let branch = o.git_branch.as_deref().filter(|b| b.len() <= 256);
    let existing: Option<(String, i64)> = tx
        .query_row(
            "SELECT id,last_activity_ms FROM reader_agent_activity_sessions WHERE agent=?1 AND client_session_id=?2",
            params![o.agent.as_str(), o.client_session_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let mut changed = false;
    let id = match existing {
        Some((id, last)) => {
            tx.execute(
                "UPDATE reader_agent_activity_sessions SET cwd=COALESCE(cwd,?2),git_branch=COALESCE(?3,git_branch),started_at_ms=min(started_at_ms,?4),last_activity_ms=max(last_activity_ms,?5) WHERE id=?1",
                params![id, cwd, branch, o.first_at_ms, o.last_at_ms],
            )?;
            changed |= o.last_at_ms >= last + ACTIVITY_STEP_MS;
            // Activity after an end marker means the session was resumed.
            if !o.ended {
                changed |= tx.execute(
                    "UPDATE reader_agent_activity_sessions SET ended_at_ms=NULL WHERE id=?1 AND ended_at_ms IS NOT NULL AND ended_at_ms<?2",
                    params![id, o.last_at_ms],
                )? > 0;
            }
            id
        }
        None if !o.has_activity() => return Ok(None),
        None => {
            let count: i64 = tx.query_row(
                "SELECT count(*) FROM reader_agent_activity_sessions",
                [],
                |r| r.get(0),
            )?;
            if count >= MAX_SESSIONS {
                return Ok(None);
            }
            let id = uuid::Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO reader_agent_activity_sessions(id,agent,client_session_id,title,title_source,cwd,git_branch,started_at_ms,last_activity_ms,ended_at_ms) VALUES(?1,?2,?3,NULL,'fallback',?4,?5,?6,?7,NULL)",
                params![id, o.agent.as_str(), o.client_session_id, cwd, branch, o.first_at_ms, o.last_at_ms],
            )?;
            changed = true;
            id
        }
    };
    if let Some((title, source)) = &o.title {
        // Client-maintained names refresh at equal rank; prompts never do.
        let refresh = matches!(source, TitleSource::Ai | TitleSource::Custom);
        changed |= tx.execute(
            &format!("UPDATE reader_agent_activity_sessions SET title=?2,title_source=?3 WHERE id=?1 AND ({rank}<?4 OR ({rank}=?4 AND ?5 AND title IS NOT ?2))", rank = TitleSource::RANK_SQL),
            params![id, title, source.as_str(), source.rank(), refresh],
        )? > 0;
    }
    if o.ended && o.has_activity() {
        changed |= tx.execute(
            "UPDATE reader_agent_activity_sessions SET ended_at_ms=?2 WHERE id=?1 AND ended_at_ms IS NULL",
            params![id, o.last_at_ms],
        )? > 0;
    }
    let mut files: i64 = tx.query_row(
        "SELECT count(*) FROM reader_agent_activity_files WHERE session_id=?1",
        [&id],
        |r| r.get(0),
    )?;
    for file in &o.files {
        let Some(path) = file.abs_path.to_str().filter(|p| p.len() <= MAX_PATH) else {
            continue;
        };
        let Some(name) = file.abs_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !file.abs_path.is_absolute() || !super::is_markdown(&file.abs_path) {
            continue;
        }
        let known: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM reader_agent_activity_files WHERE session_id=?1 AND abs_path=?2)",
            params![id, path],
            |r| r.get(0),
        )?;
        if !known && files >= MAX_FILES_PER_SESSION {
            continue;
        }
        // The first operation names the file's role; later edits only extend it.
        changed |= tx.execute(
            "INSERT INTO reader_agent_activity_files VALUES(?1,?2,?3,?4,?5,?6,?6) ON CONFLICT(session_id,abs_path) DO UPDATE SET first_at_ms=min(first_at_ms,excluded.first_at_ms),last_at_ms=max(last_at_ms,excluded.last_at_ms) WHERE excluded.last_at_ms>last_at_ms OR excluded.first_at_ms<first_at_ms",
            params![id, path, name, exact::exact_id(path), file.op.as_str(), file.at_ms],
        )? > 0;
        files += i64::from(!known);
    }
    Ok(changed.then_some(id))
}

impl ReaderStore {
    /// Record observations and, when reading a log, advance its cursor in the
    /// same transaction so a crash never skips or double-counts a chunk.
    /// Returns the ids of sessions whose visible state changed.
    pub fn record_agent_activity(
        &mut self,
        observations: &[SessionObservation],
        cursor: Option<&Cursor>,
    ) -> Result<Vec<String>> {
        schema::initialize(self)?;
        self.with_library_connection(|db| {
            let tx = db.transaction()?;
            let mut changed = BTreeSet::new();
            for observation in observations {
                changed.extend(apply(&tx, observation)?);
            }
            if let Some(cursor) = cursor {
                super::state::save_cursor(&tx, cursor)?;
            }
            if !changed.is_empty() {
                bump(&tx)?;
            }
            tx.commit()?;
            Ok(changed.into_iter().collect())
        })
    }

    /// A user rename wins over every automatic title. An empty title returns
    /// the session to automatic naming, which later log entries refill.
    pub fn rename_agent_session(&mut self, session: &str, title: &str) -> Result<()> {
        schema::initialize(self)?;
        let title = super::clean_title(title);
        let source = if title.is_some() {
            TitleSource::User
        } else {
            TitleSource::Fallback
        };
        self.with_library_connection(|db| {
            let tx = db.transaction()?;
            let updated = tx.execute(
                "UPDATE reader_agent_activity_sessions SET title=?2,title_source=?3 WHERE id=?1",
                params![session, title, source.as_str()],
            )?;
            if updated == 0 {
                return Err(ReaderError::Unavailable);
            }
            bump(&tx)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Hidden sessions stay recorded (so re-ingest cannot resurrect them) but
    /// are never listed.
    pub fn hide_agent_session(&mut self, session: &str) -> Result<()> {
        schema::initialize(self)?;
        self.with_library_connection(|db| {
            let tx = db.transaction()?;
            if tx.execute(
                "UPDATE reader_agent_activity_sessions SET hidden=1 WHERE id=?1",
                [session],
            )? == 0
            {
                return Err(ReaderError::Unavailable);
            }
            bump(&tx)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Remove every tracked session, file association and log cursor for one
    /// agent. Original files and transcripts are never touched.
    pub fn forget_agent_activity(&mut self, agent: Agent) -> Result<()> {
        schema::initialize(self)?;
        self.with_library_connection(|db| {
            let tx = db.transaction()?;
            tx.execute(
                "DELETE FROM reader_agent_activity_sessions WHERE agent=?1",
                [agent.as_str()],
            )?;
            tx.execute(
                "DELETE FROM reader_agent_ingest_cursor WHERE agent=?1",
                [agent.as_str()],
            )?;
            bump(&tx)?;
            tx.commit()?;
            Ok(())
        })
    }
}
