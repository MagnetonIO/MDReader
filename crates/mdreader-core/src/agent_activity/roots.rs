//! Where recorded files are indexed and opened. A file inside an existing
//! source keeps that source's reference. Any other file gets an "Agent
//! files" root for its folder: a store source whose scans visit only the
//! recorded files in it, so siblings are never indexed or readable through
//! it. Roots stay out of source listings, grants and project choices; a
//! library folder admitted at the same path claims the root as its own.
use super::schema;
use crate::store::{decode_path, encode_path};
use crate::{ReaderError, ReaderStore, Result, SourceRoot};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::path::{Path, PathBuf};

/// Recorded files a root's scan visits; the scanner's target budget.
const MAX_TARGETS: i64 = 256;
/// Roots searched together, most recently recorded first.
const MAX_SEARCH_ROOTS: usize = 256;
const ROOT_NAME: &str = "Agent files";

fn namespace(db: &Connection) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='table' AND name='reader_agent_activity_roots')",
        [],
        |r| r.get(0),
    )?)
}

/// Whether `source` is an "Agent files" root.
pub(crate) fn is_root(db: &Connection, source: &str) -> Result<bool> {
    Ok(namespace(db)?
        && db.query_row(
            "SELECT EXISTS(SELECT 1 FROM reader_agent_activity_roots WHERE source_id=?1)",
            [source],
            |r| r.get(0),
        )?)
}

/// SQL excluding roots from a `sources` query aliased `s`, when they exist.
pub(crate) fn exclude_roots(db: &Connection) -> Result<&'static str> {
    Ok(if namespace(db)? {
        " AND s.id NOT IN (SELECT source_id FROM reader_agent_activity_roots)"
    } else {
        ""
    })
}

/// The only paths a root's scan may visit; `None` for other sources.
pub(crate) fn targets(db: &Connection, source: &str) -> Result<Option<Vec<PathBuf>>> {
    if !is_root(db, source)? {
        return Ok(None);
    }
    let paths = db
        .prepare("SELECT path FROM reader_agent_activity_locations WHERE source_id=?1 ORDER BY path LIMIT ?2")?
        .query_map(params![source, MAX_TARGETS], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Some(paths.into_iter().map(PathBuf::from).collect()))
}

/// A root serves only recorded paths.
pub(crate) fn allows(db: &Connection, source: &str, path: &str) -> Result<bool> {
    Ok(!is_root(db, source)?
        || db.query_row(
            "SELECT EXISTS(SELECT 1 FROM reader_agent_activity_locations WHERE source_id=?1 AND path=?2)",
            [source, path],
            |r| r.get(0),
        )?)
}

/// A library folder admitted at a root's path claims it: its scans become
/// ordinary full scans and recorded files stay addressable under the same id.
pub(crate) fn release(tx: &Transaction<'_>, source: &str) -> Result<()> {
    if namespace(tx)? {
        tx.execute(
            "DELETE FROM reader_agent_activity_roots WHERE source_id=?1",
            [source],
        )?;
    }
    Ok(())
}

fn root_id(dir: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"mdreader-agent-files-v1\0");
    hasher.update(dir.as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn relative(path: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = path
        .components()
        .map(|c| match c {
            std::path::Component::Normal(n) => n.to_str(),
            _ => None,
        })
        .collect();
    let joined = parts?.join("/");
    (!joined.is_empty() && joined.len() <= 4096).then_some(joined)
}

impl ReaderStore {
    /// The deepest enabled ordinary or root source containing `file`.
    fn containing(&mut self, file: &Path) -> Result<Option<(String, String)>> {
        let rows = self.with_library_connection(|db| -> Result<Vec<(String, PathBuf)>> {
            let rows = db
                .prepare(
                    "SELECT id,path FROM sources WHERE enabled=1 AND id NOT LIKE 'provider:%'",
                )?
                .query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut out = Vec::new();
            for (id, path) in rows {
                if !crate::local_authority::is_owned(db, &id)?
                    && !crate::device::is_device_source(db, &id)?
                {
                    out.push((id, decode_path(path)?));
                }
            }
            Ok(out)
        })?;
        let mut best: Option<(usize, String, String)> = None;
        for (id, root) in rows {
            let Ok(rest) = file.strip_prefix(&root) else {
                continue;
            };
            let Some(rel) = relative(rest) else { continue };
            let depth = root.components().count();
            if best.as_ref().is_none_or(|(d, _, _)| depth > *d)
                && self
                    .stored_root(&id, true)
                    .is_ok_and(|r| r.allows_relative(Path::new(&rel)))
            {
                best = Some((depth, id, rel));
            }
        }
        Ok(best.map(|(_, id, rel)| (id, rel)))
    }

    /// Give a recorded file its indexed, openable location and queue it for
    /// (re)indexing. `None` when it is private, gone or not representable.
    pub fn admit_agent_file(&mut self, abs_path: &str) -> Result<Option<(String, String)>> {
        schema::initialize(self)?;
        let private =
            self.with_library_connection(|db| crate::local_authority::private_paths(db))?;
        let abs = Path::new(abs_path);
        let (Some(parent), Some(name)) = (abs.parent(), abs.file_name().and_then(|n| n.to_str()))
        else {
            return Ok(None);
        };
        let Ok(dir) = std::fs::canonicalize(parent) else {
            return Ok(None);
        };
        let file = dir.join(name);
        if !super::is_markdown(&file)
            || !std::fs::metadata(&file).is_ok_and(|m| m.is_file())
            || crate::local_authority::private_document(&private, abs)
            || crate::local_authority::private_document(&private, &file)
        {
            return Ok(None);
        }
        let location = match self.containing(&file)? {
            Some(found) => found,
            None => {
                let Some(dir_text) = dir.to_str().map(str::to_owned) else {
                    return Ok(None);
                };
                let captured = SourceRoot::capture(dir.clone())?;
                let root = self
                    .with_library_connection(|db| crate::local_authority::protect(db, captured))?;
                root.verify()?;
                if !root.allows_relative(Path::new(name)) {
                    return Ok(None);
                }
                let id = root_id(&dir_text);
                let identity = serde_json::to_string(&root.observed_identity()?)
                    .map_err(|_| ReaderError::Unavailable)?;
                self.with_library_connection(|db| -> Result<()> {
                    let tx = db.transaction()?;
                    tx.execute(
                        "INSERT INTO sources VALUES(?1,?2,?3,?4,1) ON CONFLICT(id) DO UPDATE SET identity=excluded.identity,enabled=1",
                        params![id, ROOT_NAME, encode_path(&dir), identity],
                    )?;
                    tx.execute(
                        "INSERT OR IGNORE INTO reader_agent_activity_roots VALUES(?1,?2)",
                        params![id, dir_text],
                    )?;
                    crate::local_authority::refresh(&tx)?;
                    tx.commit()?;
                    Ok(())
                })?;
                (id, name.to_owned())
            }
        };
        let (source, path) = location;
        let root = self.with_library_connection(|db| -> Result<bool> {
            db.execute(
                "INSERT INTO reader_agent_activity_locations VALUES(?1,?2,?3) ON CONFLICT(abs_path) DO UPDATE SET source_id=excluded.source_id,path=excluded.path",
                params![abs_path, source, path],
            )?;
            is_root(db, &source)
        })?;
        if root {
            self.queue_folder_changes(&source, std::slice::from_ref(&path))?;
        }
        Ok(Some((source, path)))
    }

    /// Roots to include in a search scope: enabled, still present, most
    /// recently recorded first.
    pub fn agent_file_sources(&mut self) -> Result<Vec<String>> {
        schema::initialize(self)?;
        let ids = self.with_library_connection(|db| -> Result<Vec<String>> {
            Ok(db
            .prepare("SELECT r.source_id FROM reader_agent_activity_roots r JOIN sources s ON s.id=r.source_id WHERE s.enabled=1 ORDER BY (SELECT max(f.last_at_ms) FROM reader_agent_activity_locations l JOIN reader_agent_activity_files f ON f.abs_path=l.abs_path WHERE l.source_id=r.source_id) DESC")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
        })?;
        Ok(ids
            .into_iter()
            .filter(|id| self.stored_root(id, true).is_ok_and(|r| r.verify().is_ok()))
            .take(MAX_SEARCH_ROOTS)
            .collect())
    }

    /// Drop locations of files no session records any more, and retire roots
    /// left without files (their derived index rows go too).
    pub(super) fn prune_agent_roots(&mut self) -> Result<()> {
        let orphans = self.with_library_connection(|db| -> Result<Vec<String>> {
            db.execute(
                "DELETE FROM reader_agent_activity_locations WHERE abs_path NOT IN (SELECT abs_path FROM reader_agent_activity_files)",
                [],
            )?;
            Ok(db
                .prepare("SELECT source_id FROM reader_agent_activity_roots WHERE source_id NOT IN (SELECT source_id FROM reader_agent_activity_locations)")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?)
        })?;
        for id in orphans {
            self.forget_source_index(&id)?;
        }
        Ok(())
    }

    /// The admitted location of a recorded file, if any.
    pub fn agent_file_location(&mut self, abs_path: &str) -> Result<Option<(String, String)>> {
        schema::initialize(self)?;
        self.with_library_connection(|db| {
            Ok(db
                .query_row(
                    "SELECT source_id,path FROM reader_agent_activity_locations WHERE abs_path=?1",
                    [abs_path],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?)
        })
    }
}
