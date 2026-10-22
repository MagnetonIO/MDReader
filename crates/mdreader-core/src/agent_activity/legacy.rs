//! One-time import of project-scoped session rows (`reader_agent_sessions`
//! and its document tables). Project-relative paths become absolute through
//! the source's stored root. The old tables stay in place, read-only, for one
//! release; nothing writes them any more.
use super::{Agent, FileOp, TitleSource};
use crate::{store::decode_path, ReaderError, Result};
use rusqlite::{params, OptionalExtension, Transaction};
use std::path::{Component, Path, PathBuf};

/// Titles the old hook wrote for every session; they carry no information.
const GENERIC_TITLES: [&str; 2] = ["Claude Code session", "Codex session"];

fn table_exists(tx: &Transaction<'_>, name: &str) -> Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='table' AND name=?1)",
        [name],
        |r| r.get(0),
    )?)
}

fn source_root(tx: &Transaction<'_>, source: &str) -> Result<Option<PathBuf>> {
    let path: Option<Vec<u8>> = tx
        .query_row("SELECT path FROM sources WHERE id=?1", [source], |r| {
            r.get(0)
        })
        .optional()?;
    path.map(decode_path).transpose()
}

/// A stored relative path joined onto its root, refusing anything that could
/// escape it. Unrepresentable rows are skipped rather than failing the import.
fn absolute(root: &Path, relative: &str) -> Option<String> {
    let relative = Path::new(relative);
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    root.join(relative).to_str().map(str::to_owned)
}

struct LegacySession {
    id: String,
    project: String,
    client: String,
    agent: Agent,
    title: String,
    started: i64,
    last: i64,
    ended: Option<i64>,
}

pub(super) fn import(tx: &Transaction<'_>) -> Result<()> {
    if !table_exists(tx, "reader_agent_sessions")? {
        return Ok(());
    }
    let sessions = tx
        .prepare("SELECT id,project_id,client_session_id,agent,title,started_at_ms,last_seen_ms,ended_at_ms FROM reader_agent_sessions ORDER BY started_at_ms,id")?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, Option<i64>>(7)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|(id, project, client, agent, title, started, last, ended)| {
            Some(LegacySession {
                agent: Agent::parse(&agent)?,
                id,
                project,
                client,
                title,
                started,
                last,
                ended,
            })
        });
    let documents = table_exists(tx, "reader_agent_documents")?;
    let source_documents = table_exists(tx, "reader_agent_source_documents")?;
    for legacy in sessions {
        let root = source_root(tx, &legacy.project)?;
        let cwd = root.as_deref().and_then(Path::to_str).map(str::to_owned);
        let title =
            super::clean_title(&legacy.title).filter(|t| !GENERIC_TITLES.contains(&t.as_str()));
        let source = if title.is_some() {
            TitleSource::Custom
        } else {
            TitleSource::Fallback
        };
        // The same client session reported to two projects merges into one.
        tx.execute(
            "INSERT INTO reader_agent_activity_sessions(id,agent,client_session_id,title,title_source,cwd,git_branch,started_at_ms,last_activity_ms,ended_at_ms) VALUES(?1,?2,?3,?4,?5,?6,NULL,?7,?8,?9) ON CONFLICT(agent,client_session_id) DO UPDATE SET started_at_ms=min(started_at_ms,excluded.started_at_ms),last_activity_ms=max(last_activity_ms,excluded.last_activity_ms)",
            params![
                uuid::Uuid::new_v4().to_string(),
                legacy.agent.as_str(),
                legacy.client,
                title,
                source.as_str(),
                cwd,
                legacy.started,
                legacy.last,
                legacy.ended
            ],
        )?;
        let session: String = tx.query_row(
            "SELECT id FROM reader_agent_activity_sessions WHERE agent=?1 AND client_session_id=?2",
            params![legacy.agent.as_str(), legacy.client],
            |r| r.get(0),
        )?;
        let mut files: Vec<(String, i64)> = Vec::new();
        if let (true, Some(root)) = (documents, root.as_deref()) {
            let rows = tx
                .prepare(
                    "SELECT path,reported_at_ms FROM reader_agent_documents WHERE session_id=?1",
                )?
                .query_map([&legacy.id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            files.extend(
                rows.into_iter()
                    .filter_map(|(path, at)| Some((absolute(root, &path)?, at))),
            );
        }
        if source_documents {
            let rows = tx
                .prepare("SELECT source_id,path,reported_at_ms FROM reader_agent_source_documents WHERE session_id=?1")?
                .query_map([&legacy.id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for (source, path, at) in rows {
                if let Some(root) = source_root(tx, &source)? {
                    files.extend(absolute(&root, &path).map(|p| (p, at)));
                }
            }
        }
        for (path, at) in files {
            let name = Path::new(&path)
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or(ReaderError::Unavailable)?
                .to_owned();
            tx.execute(
                "INSERT INTO reader_agent_activity_files VALUES(?1,?2,?3,?4,?5,?5) ON CONFLICT(session_id,abs_path) DO UPDATE SET first_at_ms=min(first_at_ms,excluded.first_at_ms),last_at_ms=max(last_at_ms,excluded.last_at_ms)",
                params![session, path, name, FileOp::Edit.as_str(), at],
            )?;
        }
    }
    Ok(())
}
