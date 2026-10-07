//! App-owned Markdown imports, with SQLite metadata and the embedded reader owner.
//! Original bytes remain durable files; paths are never supplied as authority.
use crate::reader::ReaderBackend;
use mdreader_core::{ReaderStore, SourceRoot, SourceRootIdentity};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tauri::Manager;

const MAX_BYTES: usize = 1024 * 1024;
const MAX_LIBRARY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DOCUMENTS: u64 = 1000;

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct DeviceDocument {
    pub id: String,
    pub name: String,
    pub source_id: String,
    pub revision: String,
    pub bytes: u32,
    pub pinned: bool,
    pub line: u32,
    pub indexed: bool,
}
#[derive(Debug, Serialize, specta::Type)]
pub struct DeviceRead {
    pub document: DeviceDocument,
    pub markdown: String,
}
#[derive(Debug, Serialize, specta::Type)]
pub struct DeviceHit {
    pub document: DeviceDocument,
    pub line: u32,
    pub text: String,
}
#[derive(Debug, Serialize, specta::Type)]
pub struct PendingDeviceImport {
    pub id: String,
    pub name: String,
    pub bytes: u32,
}
#[derive(Debug, Serialize, specta::Type)]
pub struct DeviceLibrary {
    pub documents: Vec<DeviceDocument>,
    pub pending: Vec<PendingDeviceImport>,
}
struct ImportIntent {
    id: String,
    name: String,
    revision: String,
    bytes: u32,
    parent_identity: String,
    root_identity: Option<String>,
}

fn schema(store: &mut ReaderStore) -> Result<(), String> {
    store.with_library_connection(|db| db.execute_batch("CREATE TABLE IF NOT EXISTS device_documents_v1(
        id TEXT PRIMARY KEY, name TEXT NOT NULL, source_id TEXT NOT NULL UNIQUE REFERENCES sources(id),
        revision TEXT NOT NULL, bytes INTEGER NOT NULL CHECK(bytes>=0), pinned INTEGER NOT NULL DEFAULT 0 CHECK(pinned IN(0,1)),
        line INTEGER NOT NULL DEFAULT 1 CHECK(line>0), indexed INTEGER NOT NULL DEFAULT 0 CHECK(indexed IN(0,1)));
        CREATE TABLE IF NOT EXISTS device_import_intents_v1(
        id TEXT PRIMARY KEY, name TEXT NOT NULL, revision TEXT NOT NULL CHECK(length(revision)=64),
        bytes INTEGER NOT NULL CHECK(bytes BETWEEN 0 AND 1048576), parent_identity TEXT NOT NULL,
        root_identity TEXT, state TEXT NOT NULL CHECK(state IN('reserved','published')));").map_err(|e| e.to_string()))
}
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DeviceDocument> {
    Ok(DeviceDocument {
        id: r.get(0)?,
        name: r.get(1)?,
        source_id: r.get(2)?,
        revision: r.get(3)?,
        bytes: r.get(4)?,
        pinned: r.get(5)?,
        line: r.get(6)?,
        indexed: r.get(7)?,
    })
}
fn list(store: &mut ReaderStore) -> Result<Vec<DeviceDocument>, String> {
    schema(store)?;
    let mut documents = store.with_library_connection(|db| {
        db.prepare("SELECT d.id,d.name,d.source_id,d.revision,d.bytes,d.pinned,d.line,d.indexed FROM device_documents_v1 d JOIN sources s ON s.id=d.source_id WHERE s.enabled=1 ORDER BY d.pinned DESC,d.rowid DESC LIMIT 1000")?
            .query_map([],row)?.collect::<Result<Vec<_>,_>>()
    }).map_err(|e:rusqlite::Error|e.to_string())?;
    for document in &mut documents {
        document.indexed = store
            .index_status(std::slice::from_ref(&document.source_id))
            .is_ok_and(|status| status.documents == 1);
    }
    Ok(documents)
}
fn document(store: &mut ReaderStore, id: &str) -> Result<DeviceDocument, String> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err("Invalid document identity".into());
    }
    schema(store)?;
    store.with_library_connection(|db| db.query_row("SELECT d.id,d.name,d.source_id,d.revision,d.bytes,d.pinned,d.line,d.indexed FROM device_documents_v1 d JOIN sources s ON s.id=d.source_id WHERE d.id=?1 AND s.enabled=1",[id],row).optional())
        .map_err(|e| e.to_string())?.ok_or_else(||"Document is unavailable".into())
}
fn read(store: &mut ReaderStore, id: &str, expected: Option<&str>) -> Result<DeviceRead, String> {
    let document = document(store, id)?;
    if expected.is_some_and(|value| value != document.revision) {
        return Err("Document revision changed".into());
    }
    let body = store
        .read_document(&document.source_id, "document.md", Some(&document.revision))
        .map_err(|e| e.to_string())?;
    Ok(DeviceRead {
        document,
        markdown: body.markdown,
    })
}
fn private_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err("Device library path is not a private directory".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(path).map_err(|e| e.to_string())?;
        }
        Err(error) => return Err(error.to_string()),
    }
    Ok(())
}
// A reservation is durable before any file write and consumes the same quota as
// a published document. An interrupted copy never creates a source grant.
fn reserve_import(
    store: &mut ReaderStore,
    parent: &SourceRoot,
    name: String,
    content: &str,
) -> Result<ImportIntent, String> {
    schema(store)?;
    let intent = ImportIntent {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        revision: blake3::hash(content.as_bytes()).to_hex().to_string(),
        bytes: content.len() as u32,
        parent_identity: serde_json::to_string(&parent.identity).map_err(|e| e.to_string())?,
        root_identity: None,
    };
    store.with_library_connection(|db| {
        let tx = db.transaction()?;
        let (count, bytes): (u64,u64) = tx.query_row(
            "SELECT count(*),coalesce(sum(bytes),0) FROM (SELECT bytes FROM device_documents_v1 UNION ALL SELECT bytes FROM device_import_intents_v1 WHERE state='reserved')",
            [], |r| Ok((r.get(0)?,r.get(1)?)))?;
        if count >= MAX_DOCUMENTS || bytes + u64::from(intent.bytes) > MAX_LIBRARY_BYTES {
            return Err(rusqlite::Error::InvalidParameterName("Device library is full (1,000 documents or 64 MiB), including interrupted imports".into()));
        }
        tx.execute("INSERT INTO device_import_intents_v1(id,name,revision,bytes,parent_identity,state) VALUES(?1,?2,?3,?4,?5,'reserved')",
            params![intent.id,intent.name,intent.revision,intent.bytes,intent.parent_identity])?;
        tx.commit()
    }).map_err(|e|e.to_string())?;
    Ok(intent)
}
fn write_import(
    store: &mut ReaderStore,
    parent: &SourceRoot,
    intent: &mut ImportIntent,
    content: &str,
) -> Result<(), String> {
    let root = parent
        .create_or_capture_directory_relative(Path::new(&intent.id))
        .map_err(|e| e.to_string())?;
    parent
        .try_clone_directory_handle()
        .map_err(|e| e.to_string())?
        .sync_all()
        .map_err(|e| e.to_string())?;
    let identity = serde_json::to_string(&root.identity).map_err(|e| e.to_string())?;
    store.with_library_connection(|db|db.execute("UPDATE device_import_intents_v1 SET root_identity=?2 WHERE id=?1 AND state='reserved'",params![intent.id,identity])).map_err(|e|e.to_string())?;
    intent.root_identity = Some(identity);
    let mut file = root
        .create_new_relative(Path::new("document.md"))
        .map_err(|e| e.to_string())?;
    file.write_all(content.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    root.try_clone_directory_handle()
        .map_err(|e| e.to_string())?
        .sync_all()
        .map_err(|e| e.to_string())?;
    root.verify().map_err(|e| e.to_string())
}
fn intent_root(directory: &Path, intent: &ImportIntent) -> Result<SourceRoot, String> {
    uuid::Uuid::parse_str(&intent.id).map_err(|_| "Invalid import identity")?;
    let parent_identity: SourceRootIdentity = serde_json::from_str(&intent.parent_identity)
        .map_err(|_| "Invalid import directory identity")?;
    let parent = SourceRoot::restore(
        directory.canonicalize().map_err(|e| e.to_string())?,
        parent_identity,
    )
    .map_err(|e| e.to_string())?;
    let identity: SourceRootIdentity = serde_json::from_str(
        intent
            .root_identity
            .as_deref()
            .ok_or("Import copy did not start")?,
    )
    .map_err(|_| "Invalid import copy identity")?;
    parent.verify().map_err(|e| e.to_string())?;
    SourceRoot::restore(parent.path.join(&intent.id), identity).map_err(|e| e.to_string())
}
fn publish_import(
    store: &mut ReaderStore,
    directory: &Path,
    intent: &ImportIntent,
) -> Result<DeviceDocument, String> {
    let root = intent_root(directory, intent)?;
    let mut bytes = Vec::new();
    let mut file = root
        .open_optional_file_relative(Path::new("document.md"))
        .map_err(|e| e.to_string())?
        .ok_or("Interrupted import copy is missing")?;
    Read::by_ref(&mut file)
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() != intent.bytes as usize
        || blake3::hash(&bytes).to_hex().as_str() != intent.revision
        || std::str::from_utf8(&bytes).is_err()
    {
        return Err("Interrupted import copy is incomplete or changed".into());
    }
    // Sync recovery bytes too; source grant, metadata and completion share one
    // SQLite transaction and final retained-root verification.
    file.sync_all().map_err(|e| e.to_string())?;
    let (source,()) = store.add_local_source_with(&intent.name,root,|tx,source| {
        if !source.enabled { return Err(mdreader_core::ReaderError::Unavailable); }
        tx.execute("INSERT INTO device_documents_v1(id,name,source_id,revision,bytes) VALUES(?1,?2,?3,?4,?5)",params![intent.id,intent.name,source.id,intent.revision,intent.bytes])?;
        if tx.execute("UPDATE device_import_intents_v1 SET state='published' WHERE id=?1 AND state='reserved'",[&intent.id])? != 1 {
            return Err(mdreader_core::ReaderError::Conflict);
        }
        Ok(())
    }).map_err(|e|e.to_string())?;
    let indexed = store.index_source(&source.id).is_ok();
    if indexed {
        store
            .with_library_connection(|db| {
                db.execute(
                    "UPDATE device_documents_v1 SET indexed=1 WHERE id=?1",
                    [&intent.id],
                )
            })
            .map_err(|e| e.to_string())?;
    }
    document(store, &intent.id)
}
fn pending_imports(store: &mut ReaderStore) -> Result<Vec<ImportIntent>, String> {
    schema(store)?;
    store.with_library_connection(|db|db.prepare("SELECT id,name,revision,bytes,parent_identity,root_identity FROM device_import_intents_v1 WHERE state='reserved' ORDER BY rowid LIMIT 1000")?
        .query_map([],|r|Ok(ImportIntent{id:r.get(0)?,name:r.get(1)?,revision:r.get(2)?,bytes:r.get(3)?,parent_identity:r.get(4)?,root_identity:r.get(5)?}))?.collect::<Result<Vec<_>,_>>()).map_err(|e:rusqlite::Error|e.to_string())
}
fn recover_imports(store: &mut ReaderStore, directory: &Path) -> Result<DeviceLibrary, String> {
    // Only exact persisted intents are considered. Never crawl an app directory
    // or infer consent from a leftover file. Failed copies stay reserved/visible.
    for intent in pending_imports(store)? {
        let _ = publish_import(store, directory, &intent);
    }
    let pending = pending_imports(store)?
        .into_iter()
        .map(|intent| PendingDeviceImport {
            id: intent.id,
            name: intent.name,
            bytes: intent.bytes,
        })
        .collect();
    Ok(DeviceLibrary {
        documents: list(store)?,
        pending,
    })
}
fn discard_import(store: &mut ReaderStore, directory: &Path, id: &str) -> Result<(), String> {
    let intent = pending_imports(store)?
        .into_iter()
        .find(|intent| intent.id == id)
        .ok_or("Interrupted import is unavailable")?;
    uuid::Uuid::parse_str(id).map_err(|_| "Invalid import identity")?;
    let parent_identity = serde_json::from_str(&intent.parent_identity)
        .map_err(|_| "Invalid import directory identity")?;
    let parent = SourceRoot::restore(
        directory.canonicalize().map_err(|e| e.to_string())?,
        parent_identity,
    )
    .map_err(|e| e.to_string())?;
    if intent.root_identity.is_some() && fs::symlink_metadata(parent.path.join(id)).is_ok() {
        let root = intent_root(directory, &intent)?;
        if root
            .open_optional_file_relative(Path::new("document.md"))
            .map_err(|e| e.to_string())?
            .is_some()
        {
            root.remove_file_relative(Path::new("document.md"))
                .map_err(|e| e.to_string())?;
        }
        root.try_clone_directory_handle()
            .map_err(|e| e.to_string())?
            .sync_all()
            .map_err(|e| e.to_string())?;
        root.verify().map_err(|e| e.to_string())?;
    }
    // No recursive deletion: unexpected files prevent removal and keep the
    // quota reservation. The user's original selection is never touched.
    match fs::remove_dir(parent.path.join(id)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    parent
        .try_clone_directory_handle()
        .map_err(|e| e.to_string())?
        .sync_all()
        .map_err(|e| e.to_string())?;
    parent.verify().map_err(|e| e.to_string())?;
    store
        .with_library_connection(|db| {
            db.execute(
                "DELETE FROM device_import_intents_v1 WHERE id=?1 AND state='reserved'",
                [id],
            )
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}
fn import(
    store: &mut ReaderStore,
    directory: &Path,
    name: String,
    content: String,
) -> Result<DeviceDocument, String> {
    if name.is_empty()
        || name.len() > 240
        || name
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
        || !name.to_ascii_lowercase().ends_with(".md")
        || content.len() > MAX_BYTES
    {
        return Err("Select a UTF-8 Markdown file up to 1 MiB with a plain filename".into());
    }
    private_directory(directory)?;
    let parent = SourceRoot::capture(directory.canonicalize().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut intent = reserve_import(store, &parent, name, &content)?;
    write_import(store, &parent, &mut intent, &content)?;
    publish_import(store, &parent.path, &intent)
}
fn search(store: &mut ReaderStore, query: &str) -> Result<Vec<DeviceHit>, String> {
    if query.len() > 1024 {
        return Err("Search is too long".into());
    }
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let docs = list(store)?;
    if docs.is_empty() {
        return Ok(Vec::new());
    }
    let sources = docs
        .iter()
        .map(|doc| doc.source_id.clone())
        .collect::<Vec<_>>();
    let mut hits = Vec::new();
    // Keep each query below the core's 128-source scope bound.
    for selected in sources.chunks(64) {
        for evidence in store
            .search(query, selected, 50)
            .map_err(|e| e.to_string())?
        {
            if let Some(document) = docs.iter().find(|doc| {
                doc.source_id == evidence.source_id && doc.revision == evidence.revision
            }) {
                hits.push((
                    evidence.score,
                    DeviceHit {
                        document: document.clone(),
                        line: evidence.line_start,
                        text: evidence.text,
                    },
                ));
            }
        }
    }
    hits.sort_by(|a, b| b.0.total_cmp(&a.0));
    hits.truncate(50);
    Ok(hits.into_iter().map(|(_, hit)| hit).collect())
}
fn position(store: &mut ReaderStore, id: &str, revision: &str, line: u32) -> Result<(), String> {
    let body = read(store, id, Some(revision))?;
    let max_line = body.markdown.lines().count().max(1) as u32;
    if line == 0 || line > max_line {
        return Err("Reading position is outside this revision".into());
    }
    store
        .with_library_connection(|db| {
            db.execute(
                "UPDATE device_documents_v1 SET line=?2 WHERE id=?1",
                params![id, line],
            )
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}
async fn invoke<T: Send + 'static>(
    backend: ReaderBackend,
    operation: impl FnOnce(&mut ReaderStore) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || backend.with_mobile_store(operation))
        .await
        .map_err(|_| "Device library worker failed".to_string())?
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_reader_list(
    app: tauri::AppHandle,
    backend: tauri::State<'_, ReaderBackend>,
) -> Result<DeviceLibrary, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("device-markdown-v1");
    invoke(backend.inner().clone(), move |store| {
        recover_imports(store, &directory)
    })
    .await
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_reader_discard_import(
    app: tauri::AppHandle,
    backend: tauri::State<'_, ReaderBackend>,
    id: String,
) -> Result<(), String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("device-markdown-v1");
    invoke(backend.inner().clone(), move |store| {
        discard_import(store, &directory, &id)
    })
    .await
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_reader_import(
    app: tauri::AppHandle,
    backend: tauri::State<'_, ReaderBackend>,
    name: String,
    content: String,
) -> Result<DeviceDocument, String> {
    let directory: PathBuf = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("device-markdown-v1");
    invoke(backend.inner().clone(), move |store| {
        import(store, &directory, name, content)
    })
    .await
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_reader_read(
    backend: tauri::State<'_, ReaderBackend>,
    id: String,
    revision: Option<String>,
) -> Result<DeviceRead, String> {
    invoke(backend.inner().clone(), move |store| {
        read(store, &id, revision.as_deref())
    })
    .await
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_reader_search(
    backend: tauri::State<'_, ReaderBackend>,
    query: String,
) -> Result<Vec<DeviceHit>, String> {
    invoke(backend.inner().clone(), move |store| search(store, &query)).await
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_reader_rebuild(backend: tauri::State<'_, ReaderBackend>) -> Result<(), String> {
    invoke(backend.inner().clone(), |store| {
        for document in list(store)? {
            store
                .index_source(&document.source_id)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .await
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_reader_pin(
    backend: tauri::State<'_, ReaderBackend>,
    id: String,
    pinned: bool,
) -> Result<(), String> {
    invoke(backend.inner().clone(), move |store| {
        document(store, &id)?;
        store
            .with_library_connection(|db| {
                db.execute(
                    "UPDATE device_documents_v1 SET pinned=?2 WHERE id=?1",
                    params![id, pinned],
                )
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_reader_position(
    backend: tauri::State<'_, ReaderBackend>,
    id: String,
    revision: String,
    line: u32,
) -> Result<(), String> {
    invoke(backend.inner().clone(), move |store| {
        position(store, &id, &revision, line)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interrupted_imports_reserve_quota_and_publish_grant_atomically_after_reopen() {
        let profile = tempfile::tempdir().unwrap();
        let docs = tempfile::tempdir().unwrap();
        let parent = SourceRoot::capture(docs.path().canonicalize().unwrap()).unwrap();
        let mut store = ReaderStore::open(profile.path()).unwrap();
        let mut complete =
            reserve_import(&mut store, &parent, "complete.md".into(), "# Complete").unwrap();
        write_import(&mut store, &parent, &mut complete, "# Complete").unwrap();
        store.with_library_connection(|db| db.execute_batch("CREATE TEMP TRIGGER reject_device_metadata BEFORE INSERT ON device_documents_v1 BEGIN SELECT RAISE(ABORT,'simulated metadata failure'); END;")).unwrap();
        assert!(publish_import(&mut store, docs.path(), &complete).is_err());
        assert!(store.list_sources().unwrap().is_empty());
        assert_eq!(pending_imports(&mut store).unwrap().len(), 1);
        drop(store); // The durable complete copy recovers; the failed grant does not.
        let mut store = ReaderStore::open(profile.path()).unwrap();
        let recovered = recover_imports(&mut store, docs.path()).unwrap();
        assert!(recovered.pending.is_empty());
        assert_eq!(recovered.documents.len(), 1);
        assert_eq!(
            read(&mut store, &complete.id, None).unwrap().markdown,
            "# Complete"
        );
        let mut partial = reserve_import(
            &mut store,
            &parent,
            "partial.md".into(),
            "# Expected complete body",
        )
        .unwrap();
        write_import(&mut store, &parent, &mut partial, "# Partial").unwrap();
        drop(store);
        let mut store = ReaderStore::open(profile.path()).unwrap();
        let snapshot = recover_imports(&mut store, docs.path()).unwrap();
        assert_eq!(snapshot.documents.len(), 1);
        assert_eq!(snapshot.pending[0].id, partial.id);
        assert_eq!(store.list_sources().unwrap().len(), 1);
        assert!(document(&mut store, &partial.id).is_err());
        // Quota checks include unpublished copies without thousands of imports.
        store.with_library_connection(|db|db.execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<998) INSERT INTO device_import_intents_v1 SELECT 'quota-'||i,'quota.md',?1,1,?2,NULL,'reserved' FROM n",params![complete.revision,complete.parent_identity])).unwrap();
        assert!(reserve_import(&mut store, &parent, "over-limit.md".into(), "x").is_err());
        store
            .with_library_connection(|db| {
                db.execute(
                    "DELETE FROM device_import_intents_v1 WHERE id LIKE 'quota-%'",
                    [],
                )
            })
            .unwrap();
        // Simulate a crash after deleting the incomplete directory but before
        // releasing its SQLite reservation: retry is safe and bounded.
        fs::remove_file(parent.path.join(&partial.id).join("document.md")).unwrap();
        fs::remove_dir(parent.path.join(&partial.id)).unwrap();
        discard_import(&mut store, docs.path(), &partial.id).unwrap();
        assert!(pending_imports(&mut store).unwrap().is_empty());
        assert!(parent.path.join(&complete.id).join("document.md").exists());
        assert!(discard_import(&mut store, docs.path(), &complete.id).is_err());
    }
    #[test]
    fn imported_bytes_fulltext_pins_and_revision_bound_position_survive_reopen_and_index_loss() {
        let profile = tempfile::tempdir().unwrap();
        let docs = tempfile::tempdir().unwrap();
        let mut store = ReaderStore::open(profile.path()).unwrap();
        let text = "\u{feff}# Mobile\r\n\r\nOffline searchable naïve.\r\n";
        let imported = import(&mut store, docs.path(), "notes.md".into(), text.into()).unwrap();
        assert_eq!(read(&mut store, &imported.id, None).unwrap().markdown, text);
        assert_eq!(
            search(&mut store, "searchable").unwrap()[0].document.id,
            imported.id
        );
        position(&mut store, &imported.id, &imported.revision, 3).unwrap();
        assert!(position(&mut store, &imported.id, "stale", 2).is_err());
        assert!(position(&mut store, &imported.id, &imported.revision, 999).is_err());
        store
            .with_library_connection(|db| {
                db.execute(
                    "UPDATE device_documents_v1 SET pinned=1 WHERE id=?1",
                    [&imported.id],
                )
            })
            .unwrap();
        drop(store);
        fs::remove_file(profile.path().join("index.sqlite")).unwrap();
        let mut store = ReaderStore::open(profile.path()).unwrap();
        let actual = list(&mut store).unwrap().pop().unwrap();
        assert!(actual.pinned);
        assert_eq!(actual.line, 3);
        assert_eq!(read(&mut store, &imported.id, None).unwrap().markdown, text);
        store.rebuild_index().unwrap();
        assert_eq!(search(&mut store, "searchable").unwrap().len(), 1);
        store
            .set_source_enabled(&imported.source_id, false)
            .unwrap();
        assert!(list(&mut store).unwrap().is_empty());
        assert!(read(&mut store, &imported.id, None).is_err());
        assert!(search(&mut store, "searchable").unwrap().is_empty());
        assert!(docs.path().join(imported.id).join("document.md").exists());
    }
    #[test]
    fn import_bounds_duplicate_names_and_changed_body_are_explicit() {
        let profile = tempfile::tempdir().unwrap();
        let docs = tempfile::tempdir().unwrap();
        let mut store = ReaderStore::open(profile.path()).unwrap();
        assert!(import(&mut store, docs.path(), "../escape.md".into(), "x".into()).is_err());
        assert!(import(
            &mut store,
            docs.path(),
            "huge.md".into(),
            "x".repeat(MAX_BYTES + 1)
        )
        .is_err());
        let a = import(&mut store, docs.path(), "same.md".into(), "# First".into()).unwrap();
        let b = import(&mut store, docs.path(), "same.md".into(), "# Second".into()).unwrap();
        assert_ne!(a.id, b.id);
        fs::write(docs.path().join(&a.id).join("document.md"), "# Changed").unwrap();
        assert!(read(&mut store, &a.id, Some(&a.revision)).is_err());
        assert_eq!(read(&mut store, &b.id, None).unwrap().markdown, "# Second");
        assert!(document(&mut store, "../outside").is_err());
    }
}
