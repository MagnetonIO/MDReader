//! Mandatory private-namespace exclusions, independent of user ignore files.
//! Only native-owned admission can exempt a capture/device projection. Ordinary
//! folder grants cannot turn a private original into a local document.
use crate::store::{decode_path, encode_path, invalid, ReaderStore, Result};
use crate::{SourceRoot, SourceRootIdentity};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
pub(crate) mod sqlite_match;
const SCHEMA: &str = include_str!("schema.sql");

pub(crate) fn initialize(db: &Connection, directory: &Path) -> Result<()> {
    let found: usize = db.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE name GLOB 'reader_local_*'",
        [],
        |r| r.get(0),
    )?;
    if found == 0 {
        let tx = db.unchecked_transaction()?;
        tx.execute_batch(SCHEMA)?;
        tx.execute("INSERT INTO reader_local_policy_v1 VALUES(1,1)", [])?;
        tx.commit()?;
    }
    check(db)?;
    let mut private = vec![std::fs::canonicalize(directory)?];
    if directory.file_name().is_some_and(|n| n == "reader-v1") {
        if let Some(parent) = directory.parent() {
            private.push(parent.canonicalize()?);
        }
    }
    // Native process environment locates standard namespaces; no discovery or
    // grant derives from them. Windows uses its own profile/cache convention.
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    if let Some(home) = home {
        let home = PathBuf::from(home);
        #[cfg(target_os = "macos")]
        private.push(home.join("Library/Caches"));
        #[cfg(all(not(windows), not(target_os = "macos")))]
        private.push(home.join(".cache"));
        for name in [".codex", ".claude", ".agents"] {
            private.push(home.join(name));
        }
    }
    #[cfg(windows)]
    if let Some(cache) = std::env::var_os("LOCALAPPDATA") {
        let cache = PathBuf::from(cache);
        for name in ["MDReader", "dev.mdreader.app", "ort.pyke.io"] {
            private.push(cache.join(name));
        }
    }
    let tx = db.unchecked_transaction()?;
    for path in private {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        tx.execute(
            "INSERT OR IGNORE INTO reader_local_private_v1 VALUES(?1)",
            [encode_path(&path)],
        )?;
    }
    refresh(&tx)?;
    tx.commit()?;
    Ok(())
}
pub(crate) fn check(db: &Connection) -> Result<()> {
    let names: usize = db.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE name GLOB 'reader_local_*'",
        [],
        |r| r.get(0),
    )?;
    if names != 4 {
        return Err(invalid("Unsupported local authority schema"));
    }
    for definition in SCHEMA.split(';').filter(|s| !s.trim().is_empty()) {
        let name = definition
            .trim()
            .split(['(', ' '])
            .nth(2)
            .ok_or_else(|| invalid("Invalid local authority schema"))?;
        let sql: String = db.query_row(
            "SELECT sql FROM main.sqlite_schema WHERE type='table' AND name=?1",
            [name],
            |r| r.get(0),
        )?;
        if sql.trim() != definition.trim() {
            return Err(invalid("Unsupported local authority schema"));
        }
        let temp: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE name=?1)",
            [name],
            |r| r.get(0),
        )?;
        if temp {
            return Err(invalid("Unsupported local authority schema"));
        }
    }
    let marker: (usize, Option<i64>) = db.query_row(
        "SELECT count(*),max(version) FROM reader_local_policy_v1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let producers:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='trigger' AND tbl_name GLOB 'reader_local_*') OR EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE tbl_name GLOB 'reader_local_*')",[],|r|r.get(0))?;
    if producers || marker != (1, Some(1)) {
        return Err(invalid("Unsupported local authority schema"));
    }
    Ok(())
}
pub(crate) fn private_paths(db: &Connection) -> Result<Vec<PathBuf>> {
    check(db)?;
    let paths = db
        .prepare("SELECT path FROM reader_local_private_v1 ORDER BY path LIMIT 257")?
        .query_map([], |r| r.get::<_, Vec<u8>>(0))?
        .map(|p| decode_path(p?))
        .collect::<Result<Vec<_>>>()?;
    if paths.len() > 256 {
        return Err(invalid("Private namespace budget exceeded"));
    }
    Ok(paths)
}
pub(crate) fn protect(db: &Connection, root: SourceRoot) -> Result<SourceRoot> {
    let private = policy_paths(db, &root.path)?;
    Ok(root.excluding(private)?)
}
fn policy_paths(db: &Connection, root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = private_paths(db)?;
    #[cfg(unix)]
    if root == Path::new("/") {
        for directory in [
            "/System", "/Library", "/private", "/dev", "/proc", "/sys", "/run",
        ] {
            paths.push(PathBuf::from(directory));
        }
    }
    #[cfg(windows)]
    if root.parent().is_none() {
        for directory in [
            "Windows",
            "Program Files",
            "Program Files (x86)",
            "ProgramData",
            "$Recycle.Bin",
            "System Volume Information",
        ] {
            paths.push(root.join(directory));
        }
    }
    Ok(paths)
}
pub(crate) fn is_owned(db: &Connection, source: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM reader_local_owned_v1 WHERE source_id=?1)",
        [source],
        |r| r.get(0),
    )?)
}
pub(crate) fn refresh(db: &Connection) -> Result<()> {
    let sources=db.prepare("SELECT id,path FROM sources WHERE id NOT LIKE 'provider:%' AND id NOT IN(SELECT source_id FROM reader_local_owned_v1)")?.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Vec<u8>>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
    db.execute("DELETE FROM reader_local_denied_v1", [])?;
    for (id, path) in sources {
        if crate::device::is_device_source(db, &id)? {
            continue;
        }
        let root = decode_path(path)?;
        for denied in &policy_paths(db, &root)? {
            if crate::source_root::exclusion_relative(&root, denied).is_some() {
                db.execute(
                    "INSERT OR IGNORE INTO reader_local_denied_v1 VALUES(?1,'')",
                    [&id],
                )?;
            } else if let Some(relative) = crate::source_root::exclusion_relative(denied, &root) {
                let prefix = relative
                    .components()
                    .map(|c| {
                        c.as_os_str()
                            .to_str()
                            .ok_or_else(|| invalid("Invalid exclusion path"))
                    })
                    .collect::<Result<Vec<_>>>()?
                    .join("/");
                db.execute(
                    "INSERT OR IGNORE INTO reader_local_denied_v1 VALUES(?1,?2)",
                    params![id, prefix],
                )?;
            }
        }
    }
    Ok(())
}
impl ReaderStore {
    /// Native adapter policy, never a new folder grant or a renderer pathname.
    pub fn local_root_policy(
        &self,
        path: PathBuf,
        identity: SourceRootIdentity,
    ) -> Result<Vec<PathBuf>> {
        let root = protect(&self.library, SourceRoot::restore(path, identity)?)?;
        root.verify()?;
        policy_paths(&self.library, &root.path)
    }
    /// Used only after an owning capture adapter has proved its private spool.
    pub fn add_owned_local_source(
        &mut self,
        name: &str,
        root: SourceRoot,
    ) -> Result<crate::Source> {
        let tx = self.library.transaction()?;
        let (source, ()) = crate::store::publish_local_source(&tx, name, root, |tx, s| {
            tx.execute(
                "INSERT OR IGNORE INTO reader_local_owned_v1 VALUES(?1)",
                [&s.id],
            )?;
            Ok(())
        })?;
        refresh(&tx)?;
        tx.commit()?;
        Ok(source)
    }
}

#[cfg(test)]
mod tests;
