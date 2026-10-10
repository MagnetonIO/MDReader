//! Strict read-only attachment reopening. Never create or adopt a namespace.
use super::*;

impl AssetStore {
    /// None means both the asset namespace and selected directory are absent.
    /// A partial, future or malformed namespace is an error, never initialized.
    pub fn open_existing(store: &mut ReaderStore, directory: &Path) -> Result<Option<Self>> {
        #[cfg(windows)]
        store.private_library().map_err(|_| Error::Privacy)?;
        let db = store.device_connection();
        let metadata_definition = definition(db, "device_asset_meta_v1")?;
        if metadata_definition.is_none() {
            let count: u32 = db.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name LIKE 'device_asset_%'",
                [],
                |r| r.get(0),
            )?;
            let absent = matches!(fs::symlink_metadata(directory), Err(error) if error.kind() == std::io::ErrorKind::NotFound);
            if count != 0 || !absent {
                return Err(unavailable());
            }
            return Ok(None);
        }
        validate_schema(db)?;
        #[cfg(windows)]
        super::windows_bytes::check(db)?;
        let root: String = db.query_row("SELECT value FROM device_asset_meta_v1 WHERE key='root_identity' AND length(CAST(value AS BLOB))<=16384", [], |r| r.get(0))?;
        let identity = serde_json::from_str(&root).map_err(|_| unavailable())?;
        let metadata = fs::symlink_metadata(directory).map_err(|_| unavailable())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(unavailable());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(Error::Privacy);
            }
        }
        #[cfg(windows)]
        mdreader_capture::PrivateDirectory::open(directory).map_err(|_| Error::Privacy)?;
        let root = SourceRoot::restore(
            directory.canonicalize().map_err(|_| unavailable())?,
            identity,
        )
        .map_err(|_| unavailable())?;
        Ok(Some(Self {
            root,
            owner: store.device_owner_session(),
        }))
    }
}
pub(super) fn validate_schema(db: &rusqlite::Connection) -> Result<()> {
    let metadata_definition = definition(db, "device_asset_meta_v1")?.ok_or_else(unavailable)?;
    let expected = "CREATE TABLE device_asset_meta_v1(key TEXT PRIMARY KEY,value TEXT NOT NULL)";
    if normalized(&metadata_definition) != normalized(expected) {
        return Err(unavailable());
    }
    let version: Option<String> = db
        .query_row(
            "SELECT value FROM device_asset_meta_v1 WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if version.as_deref() != Some("1") {
        return Err(Error::UnsupportedSchema);
    }
    let metadata_rows: u32 =
        db.query_row("SELECT count(*) FROM device_asset_meta_v1", [], |r| {
            r.get(0)
        })?;
    if metadata_rows != 2 {
        return Err(unavailable());
    }
    for statement in include_str!("schema-v1.sql")
        .split(';')
        .filter(|s| !s.trim().is_empty())
    {
        let words: Vec<_> = statement.split_whitespace().collect();
        let name = words
            .iter()
            .position(|word| *word == "EXISTS")
            .and_then(|i| words.get(i + 1))
            .ok_or_else(unavailable)?;
        let actual = definition(db, name)?.ok_or_else(unavailable)?;
        if normalized(&actual) != normalized(statement) {
            return Err(unavailable());
        }
    }
    Ok(())
}
fn definition(db: &rusqlite::Connection, name: &str) -> Result<Option<String>> {
    let row: Option<(String, Option<String>)> = db
        .query_row(
            "SELECT type,sql FROM sqlite_schema WHERE name=?1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some((kind, Some(sql))) if matches!(kind.as_str(), "table" | "index") => Ok(Some(sql)),
        _ => Err(unavailable()),
    }
}
pub(super) fn normalized(sql: &str) -> String {
    let mut quote = false;
    let mut out = String::new();
    for character in sql
        .replace("IF NOT EXISTS", "")
        .trim_end_matches(';')
        .chars()
    {
        if character == '\'' {
            quote = !quote;
        }
        if quote || !character.is_whitespace() {
            out.push(if quote {
                character
            } else {
                character.to_ascii_lowercase()
            });
        }
    }
    out
}
