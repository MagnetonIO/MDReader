//! Per-agent tracking switches and per-log ingest cursors.
use super::{schema, Agent};
use crate::{ReaderError, ReaderStore, Result};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

const DAY_MS: i64 = 86_400_000;
/// First-run history window.
pub const DEFAULT_HISTORY_DAYS: i64 = 30;
const MAX_HISTORY_DAYS: i64 = 3650;

/// Opt-in transcript reading for one agent. Off until the user enables it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tracking {
    pub agent: Agent,
    pub enabled: bool,
    /// Logs modified before this are not read unless "load older" moves it.
    pub since_ms: Option<i64>,
    /// Listed sessions (with at least one file) for this agent.
    pub sessions: u32,
}

/// How far one log has been read. `size`/`mtime_ms` detect rotation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub log_path: String,
    pub agent: Agent,
    /// The session the log belongs to, once known (Codex names it only in
    /// its first entry, so later chunks need it from here).
    pub client_session_id: Option<String>,
    pub byte_offset: u64,
    pub mtime_ms: i64,
    pub size: u64,
}

fn as_i64(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| ReaderError::Unavailable)
}

pub(super) fn save_cursor(tx: &Transaction<'_>, cursor: &Cursor) -> Result<()> {
    tx.execute(
        "INSERT INTO reader_agent_ingest_cursor VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(log_path) DO UPDATE SET agent=excluded.agent,client_session_id=COALESCE(excluded.client_session_id,client_session_id),byte_offset=excluded.byte_offset,mtime_ms=excluded.mtime_ms,size=excluded.size",
        params![
            cursor.log_path,
            cursor.agent.as_str(),
            cursor.client_session_id,
            as_i64(cursor.byte_offset)?,
            cursor.mtime_ms,
            as_i64(cursor.size)?
        ],
    )?;
    Ok(())
}

impl ReaderStore {
    pub fn agent_tracking(&mut self) -> Result<Vec<Tracking>> {
        schema::initialize(self)?;
        self.with_library_connection(|db| {
            Agent::ALL
                .iter()
                .map(|agent| {
                    let row: Option<(bool, i64)> = db
                        .query_row(
                            "SELECT enabled,since_ms FROM reader_agent_activity_tracking WHERE agent=?1",
                            [agent.as_str()],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .optional()?;
                    let sessions: u32 = db.query_row(
                        "SELECT count(*) FROM reader_agent_activity_sessions s WHERE s.agent=?1 AND s.hidden=0 AND EXISTS(SELECT 1 FROM reader_agent_activity_files f WHERE f.session_id=s.id)",
                        [agent.as_str()],
                        |r| r.get(0),
                    )?;
                    Ok(Tracking {
                        agent: *agent,
                        enabled: row.is_some_and(|r| r.0),
                        since_ms: row.map(|r| r.1),
                        sessions,
                    })
                })
                .collect()
        })
    }

    /// Enabling starts from the last 30 days, or keeps an older window the
    /// user already loaded. Disabling stops reading but keeps rows.
    pub fn set_agent_tracking(&mut self, agent: Agent, enabled: bool, now_ms: i64) -> Result<()> {
        schema::initialize(self)?;
        let since = now_ms - DEFAULT_HISTORY_DAYS * DAY_MS;
        self.with_library_connection(|db| {
            db.execute(
                "INSERT INTO reader_agent_activity_tracking VALUES(?1,?2,?3) ON CONFLICT(agent) DO UPDATE SET enabled=excluded.enabled,since_ms=min(since_ms,excluded.since_ms)",
                params![agent.as_str(), enabled, since],
            )?;
            Ok(())
        })
    }

    /// Move every enabled agent's window back to `days` before now.
    pub fn extend_agent_history(&mut self, days: i64, now_ms: i64) -> Result<()> {
        if !(1..=MAX_HISTORY_DAYS).contains(&days) {
            return Err(ReaderError::Invalid("history window out of range".into()));
        }
        schema::initialize(self)?;
        self.with_library_connection(|db| {
            db.execute(
                "UPDATE reader_agent_activity_tracking SET since_ms=min(since_ms,?1) WHERE enabled=1",
                [now_ms - days * DAY_MS],
            )?;
            Ok(())
        })
    }

    pub fn agent_ingest_cursor(&mut self, log_path: &str) -> Result<Option<Cursor>> {
        schema::initialize(self)?;
        self.with_library_connection(|db| {
            db.query_row(
                "SELECT agent,client_session_id,byte_offset,mtime_ms,size FROM reader_agent_ingest_cursor WHERE log_path=?1",
                [log_path],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()?
            .map(|(agent, client, offset, mtime, size)| {
                Ok(Cursor {
                    log_path: log_path.to_owned(),
                    agent: Agent::parse(&agent).ok_or(ReaderError::Unavailable)?,
                    client_session_id: client,
                    byte_offset: u64::try_from(offset).map_err(|_| ReaderError::Unavailable)?,
                    mtime_ms: mtime,
                    size: u64::try_from(size).map_err(|_| ReaderError::Unavailable)?,
                })
            })
            .transpose()
        })
    }

    /// Monotonic counter of visible activity changes, for cheap change polling
    /// by the app (hooks and MCP write from other processes).
    pub fn agent_activity_revision(&mut self) -> Result<i64> {
        schema::initialize(self)?;
        self.with_library_connection(|db| {
            Ok(db.query_row(
                "SELECT revision FROM reader_agent_activity_meta WHERE id=1",
                [],
                |r| r.get(0),
            )?)
        })
    }
}
