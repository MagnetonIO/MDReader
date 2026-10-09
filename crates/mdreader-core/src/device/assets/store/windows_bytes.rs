//! Windows attachments are canonical private SQLite bytes. No directory flush
//! or projected file is treated as a durable publication receipt.
use super::*;
use rusqlite::{Connection, Transaction};

const DDL: &str = include_str!("windows-bytes-v1.sql");
const TABLES: [&str; 2] = [
    "device_asset_windows_schema_v1",
    "device_asset_windows_bytes_v1",
];
fn unsupported(phase: &'static str) -> Error {
    #[cfg(test)]
    eprintln!("Windows asset schema refused at {phase}");
    #[cfg(not(test))]
    let _ = phase;
    Error::UnsupportedSchema
}

pub(super) fn open(store: &mut ReaderStore, directory: &Path) -> Result<AssetStore> {
    store.private_library().map_err(|_| Error::Privacy)?;
    let present: bool = store.with_library_connection(|db| db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name GLOB 'device_asset_*' OR tbl_name GLOB 'device_asset_*')", [], |r| r.get(0)))?;
    if present {
        // Exact read-only admission precedes any initialization or mutation.
        let existing = AssetStore::open_existing(store, directory)?.ok_or_else(unavailable)?;
        store.with_library_connection(|db| -> Result<()> {
            let tx = db.transaction()?;
            initialize(&tx)?;
            existing.verify()?;
            tx.commit()?;
            Ok(())
        })?;
        return Ok(existing);
    }
    let private =
        mdreader_capture::PrivateDirectory::create(directory).map_err(|_| Error::Privacy)?;
    if fs::read_dir(directory)
        .map_err(|_| unavailable())?
        .next()
        .is_some()
    {
        return Err(unavailable());
    }
    let root = SourceRoot::capture(directory.canonicalize().map_err(|_| unavailable())?)
        .map_err(|_| unavailable())?;
    let identity = serde_json::to_string(&root.identity).map_err(|_| unavailable())?;
    store.with_library_connection(|db| -> Result<()> {
        let tx = db.transaction()?;
        tx.execute_batch("CREATE TABLE device_asset_meta_v1(key TEXT PRIMARY KEY,value TEXT NOT NULL);")?;
        tx.execute_batch(include_str!("schema-v1.sql"))?;
        tx.execute("INSERT INTO device_asset_meta_v1(key,value) VALUES('schema_version','1'),('root_identity',?1)", [identity])?;
        initialize(&tx)?;
        private.verify().map_err(|_| unavailable())?;
        root.verify().map_err(|_| unavailable())?;
        tx.commit()?;
        Ok(())
    })?;
    Ok(AssetStore {
        root,
        owner: store.device_owner_session(),
    })
}
pub(in crate::device::assets) fn check(db: &Connection) -> Result<bool> {
    let shadow: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE name GLOB 'device_asset_*' OR tbl_name GLOB 'device_asset_*' OR name IN('device_documents_v1','sources'))", [], |r| r.get(0))?;
    if shadow {
        return Err(unsupported("temporary_namespace"));
    }
    existing::validate_schema(db)?;
    for schema in ["sqlite_schema", "sqlite_temp_schema"] {
        let sql = format!("SELECT EXISTS(SELECT 1 FROM {schema} WHERE type='trigger' AND (tbl_name GLOB 'device_asset_*' OR tbl_name IN('device_documents_v1','sources')))");
        if db.query_row(&sql, [], |r| r.get::<_, bool>(0))? {
            return Err(unsupported("producer_trigger"));
        }
    }
    let base_names = [
        "device_asset_meta_v1",
        "device_asset_generations_v1",
        "device_asset_items_v1",
        "device_asset_active_target_v1",
        "device_asset_pending_target_v1",
        "device_asset_document_v1",
    ];
    let mut all = db.prepare("SELECT name,tbl_name,sql FROM sqlite_schema WHERE name GLOB 'device_asset_*' OR tbl_name GLOB 'device_asset_*'")?;
    let namespaces = all
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (name, table, sql) in namespaces {
        if sql.is_none()
            && (base_names[..3].contains(&table.as_str()) || TABLES.contains(&table.as_str()))
        {
            continue;
        }
        if !base_names.contains(&name.as_str()) && !TABLES.contains(&name.as_str()) {
            return Err(unsupported("unknown_namespace"));
        }
    }
    let mut query = db.prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name GLOB 'device_asset_windows_*' OR tbl_name GLOB 'device_asset_windows_*' ORDER BY name")?;
    let rows = query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        let count: u64 = db.query_row("SELECT count(*) FROM device_asset_items_v1", [], |r| {
            r.get(0)
        })?;
        if count != 0 {
            return Err(unsupported("missing_canonical_namespace"));
        }
        return Ok(false);
    }
    let definitions: Vec<_> = DDL.split(';').take(2).collect();
    let mut found = [false; 2];
    for (kind, name, table, sql) in rows {
        if kind == "index" && sql.is_none() && TABLES.contains(&table.as_str()) {
            continue;
        }
        let index = TABLES
            .iter()
            .position(|expected| *expected == name)
            .ok_or_else(|| unsupported("unknown_canonical_object"))?;
        if kind != "table"
            || table != name
            || found[index]
            || sql.as_ref().is_none_or(|sql| {
                existing::normalized(sql) != existing::normalized(definitions[index])
            })
        {
            return Err(unsupported("canonical_definition"));
        }
        found[index] = true;
    }
    if !found.iter().all(|found| *found) {
        return Err(unsupported("partial_canonical_namespace"));
    }
    let (count, version): (u64, Option<i64>) = db.query_row(
        "SELECT count(*),max(version) FROM device_asset_windows_schema_v1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != 1 || version != Some(1) {
        return Err(unsupported("canonical_version"));
    }
    let (count, bytes, invalid): (u64, u64, bool) = db.query_row("SELECT count(*),coalesce(sum(length(b.body)),0),coalesce(max(i.id IS NULL OR typeof(b.body)!='blob' OR length(b.body)!=i.received OR i.received NOT BETWEEN 0 AND i.bytes OR i.bytes NOT BETWEEN 1 AND 4194304 OR (i.state='ready' AND i.received!=i.bytes)),0) FROM device_asset_windows_bytes_v1 b LEFT JOIN device_asset_items_v1 i ON i.id=b.id", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let missing: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM device_asset_items_v1 i LEFT JOIN device_asset_windows_bytes_v1 b ON b.id=i.id WHERE b.id IS NULL)", [], |r| r.get(0))?;
    if count > 512 || bytes > MAX_LIBRARY_BYTES || invalid || missing {
        return Err(unavailable());
    }
    Ok(true)
}
pub(in crate::device::assets) fn initialize(tx: &Transaction<'_>) -> Result<()> {
    if !check(tx)? {
        tx.execute_batch(DDL)?;
    }
    check(tx)?;
    Ok(())
}
pub(super) fn reserve(tx: &Transaction<'_>, item: &Item) -> Result<()> {
    // The metadata row was just inserted in this same transaction. Check the
    // already initialized namespace before adding its exact empty body.
    tx.execute(
        "INSERT INTO device_asset_windows_bytes_v1(id,body) VALUES(?1,?2)",
        params![item.id, &[] as &[u8]],
    )?;
    if !check(tx)? {
        return Err(unavailable());
    }
    Ok(())
}
fn selected(db: &Connection, item: &Item) -> Result<Vec<u8>> {
    if !check(db)? {
        return Err(unavailable());
    }
    let current: Item = db.query_row(
        &format!("SELECT {ITEM_COLUMNS} FROM device_asset_items_v1 WHERE id=?1"),
        [&item.id],
        item_row,
    )?;
    if &current != item {
        return Err(Error::SelectionChanged);
    }
    let bytes: Vec<u8> = db.query_row(
        "SELECT body FROM device_asset_windows_bytes_v1 WHERE id=?1 AND length(body)<=4194304",
        [&item.id],
        |r| r.get(0),
    )?;
    if bytes.len() != item.received as usize {
        return Err(unavailable());
    }
    Ok(bytes)
}
pub(super) fn validate_publication(db: &Connection, item: &Item, bytes: &[u8]) -> Result<()> {
    if !check(db)? || bytes.len() != item.bytes as usize || item.received != item.bytes {
        return Err(Error::IncompleteUpload);
    }
    let exact: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM device_asset_items_v1 i JOIN device_asset_windows_bytes_v1 b ON b.id=i.id WHERE i.id=?1 AND i.document_id=?2 AND i.revision=?3 AND i.target=?4 AND i.bytes=?5 AND i.received=i.bytes AND i.expected_manifest=?6 AND i.state IN('reserved','ready') AND b.body=?7)", params![item.id,item.doc,item.revision,item.target,item.bytes,item.expected,bytes], |r| r.get(0))?;
    if !exact {
        return Err(Error::SelectionChanged);
    }
    Ok(())
}
impl AssetStore {
    pub(super) fn read_windows(&self, store: &mut ReaderStore, item: &Item) -> Result<Vec<u8>> {
        self.verify_owner(store)?;
        let bytes = store.with_library_connection(|db| selected(db, item))?;
        if bytes.len() != item.bytes as usize {
            return Err(Error::IncompleteUpload);
        }
        self.verify()?;
        Ok(bytes)
    }
    pub(super) fn write_windows(
        &self,
        store: &mut ReaderStore,
        item: &Item,
        offset: u32,
        chunk: &[u8],
    ) -> Result<u32> {
        store.with_library_connection(|db| {
            let tx = db.transaction()?;
            let mut bytes = selected(&tx, item)?;
            let start = offset as usize;
            if offset < item.received {
                if bytes.get(start..start + chunk.len()) != Some(chunk) {
                    return Err(Error::ReplayConflict);
                }
                return Ok(item.received);
            }
            bytes.extend_from_slice(chunk);
            let next = u32::try_from(bytes.len()).map_err(|_| Error::ChunkBounds)?;
            if next > item.bytes {
                return Err(Error::InvalidOffset);
            }
            if tx.execute("UPDATE device_asset_windows_bytes_v1 SET body=?2 WHERE id=?1 AND body=?3", params![item.id,bytes,&bytes[..start]])? != 1
                || tx.execute("UPDATE device_asset_items_v1 SET received=?2 WHERE id=?1 AND state='reserved' AND received=?3", params![item.id,next,item.received])? != 1 {
                return Err(Error::ReplayConflict);
            }
            check(&tx)?;
            self.verify()?;
            tx.commit()?;
            Ok(next)
        })
    }
    pub(super) fn discard_windows(&self, store: &mut ReaderStore, item: &Item) -> Result<()> {
        store.with_library_connection(|db| {
            let tx = db.transaction()?;
            selected(&tx, item)?;
            if item.state == "ready" {
                return Err(Error::PublishedAsset);
            }
            if tx.execute(
                "DELETE FROM device_asset_windows_bytes_v1 WHERE id=?1",
                [&item.id],
            )? != 1
                || tx.execute(
                    "DELETE FROM device_asset_items_v1 WHERE id=?1 AND state!='ready'",
                    [&item.id],
                )? != 1
            {
                return Err(unavailable());
            }
            check(&tx)?;
            self.verify()?;
            tx.commit()?;
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "windows_bytes_tests.rs"]
mod tests;
