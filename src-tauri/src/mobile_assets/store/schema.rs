use super::*;

impl AssetStore {
    pub fn open(store: &mut ReaderStore, directory: &Path) -> Result<Self> {
        match fs::symlink_metadata(directory) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(_) => return Err(unavailable()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder.create(directory).map_err(|_| unavailable())?;
            }
            Err(_) => return Err(unavailable()),
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if fs::metadata(directory)
                .map_err(|_| unavailable())?
                .permissions()
                .mode()
                & 0o077
                != 0
            {
                return Err("Asset directory privacy could not be verified.".into());
            }
        }
        let directory = directory.canonicalize().map_err(|_| unavailable())?;
        store.with_library_connection(|db| {
            let tx = db.transaction()?;
            tx.execute_batch("CREATE TABLE IF NOT EXISTS device_asset_meta_v1(key TEXT PRIMARY KEY,value TEXT NOT NULL);")?;
            let version: Option<String> = tx.query_row(
                "SELECT value FROM device_asset_meta_v1 WHERE key='schema_version'", [], |r| r.get(0),
            ).optional()?;
            if version.as_deref().is_some_and(|version| version != "1") {
                return Err(rusqlite::Error::InvalidParameterName("unsupported asset schema".into()));
            }
            // Initial version and unpublished prototype layout: preserve rows,
            // create missing additive tables/indexes and publish the marker atomically.
            tx.execute_batch(include_str!("schema-v1.sql"))?;
            tx.execute("INSERT OR IGNORE INTO device_asset_meta_v1(key,value) VALUES('schema_version','1')", [])?;
            tx.commit()
        }).map_err(sql)?;
        let stored: Option<String> = store
            .with_library_connection(|db| {
                db.query_row(
                    "SELECT value FROM device_asset_meta_v1 WHERE key='root_identity'",
                    [],
                    |r| r.get(0),
                )
                .optional()
            })
            .map_err(sql)?;
        let root = if let Some(value) = stored {
            let identity: SourceRootIdentity =
                serde_json::from_str(&value).map_err(|_| unavailable())?;
            SourceRoot::restore(directory, identity).map_err(|_| unavailable())?
        } else {
            let root = SourceRoot::capture(directory).map_err(|_| unavailable())?;
            let value = serde_json::to_string(&root.identity).map_err(|_| unavailable())?;
            store
                .with_library_connection(|db| {
                    db.execute(
                        "INSERT INTO device_asset_meta_v1(key,value) VALUES('root_identity',?1)",
                        [value],
                    )
                })
                .map_err(sql)?;
            root
        };
        Ok(Self { root })
    }
}
