//! App-owned Markdown imports, with SQLite metadata and the embedded reader owner.
//! Original bytes remain durable files; paths are never supplied as authority.
use crate::reader::ReaderBackend;
use mdreader_core::{device, ReaderStore};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::Manager;

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

impl From<device::DeviceDocument> for DeviceDocument {
    fn from(value: device::DeviceDocument) -> Self {
        Self {
            id: value.id,
            name: value.name,
            source_id: value.source_id,
            revision: value.revision,
            bytes: value.bytes,
            pinned: value.pinned,
            line: value.line,
            indexed: value.indexed,
        }
    }
}
impl From<device::DeviceRead> for DeviceRead {
    fn from(value: device::DeviceRead) -> Self {
        Self {
            document: value.document.into(),
            markdown: value.markdown,
        }
    }
}
impl From<device::DeviceHit> for DeviceHit {
    fn from(value: device::DeviceHit) -> Self {
        Self {
            document: value.document.into(),
            line: value.line,
            text: value.text,
        }
    }
}
impl From<device::PendingDeviceImport> for PendingDeviceImport {
    fn from(value: device::PendingDeviceImport) -> Self {
        Self {
            id: value.id,
            name: value.name,
            bytes: value.bytes,
        }
    }
}
impl From<device::DeviceLibrary> for DeviceLibrary {
    fn from(value: device::DeviceLibrary) -> Self {
        Self {
            documents: value.documents.into_iter().map(Into::into).collect(),
            pending: value.pending.into_iter().map(Into::into).collect(),
        }
    }
}

async fn invoke<T: Send + 'static>(
    backend: ReaderBackend,
    operation: impl FnOnce(&mut ReaderStore) -> device::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || {
        backend.with_mobile_store(|store| operation(store).map_err(|error| error.to_string()))
    })
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
        .map_err(|_| "Device library directory is unavailable".to_string())?
        .join("device-markdown-v1");
    invoke(backend.inner().clone(), move |store| {
        device::recover_imports(store, &directory).map(Into::into)
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
        .map_err(|_| "Device library directory is unavailable".to_string())?
        .join("device-markdown-v1");
    invoke(backend.inner().clone(), move |store| {
        device::discard_import(store, &directory, &id)
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
        .map_err(|_| "Device library directory is unavailable".to_string())?
        .join("device-markdown-v1");
    invoke(backend.inner().clone(), move |store| {
        device::import(store, &directory, name, content).map(Into::into)
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
        device::read(store, &id, revision.as_deref()).map(Into::into)
    })
    .await
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_reader_search(
    backend: tauri::State<'_, ReaderBackend>,
    query: String,
) -> Result<Vec<DeviceHit>, String> {
    invoke(backend.inner().clone(), move |store| {
        device::search(store, &query).map(|hits| hits.into_iter().map(Into::into).collect())
    })
    .await
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_reader_rebuild(backend: tauri::State<'_, ReaderBackend>) -> Result<(), String> {
    invoke(backend.inner().clone(), device::rebuild).await
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_reader_pin(
    backend: tauri::State<'_, ReaderBackend>,
    id: String,
    pinned: bool,
) -> Result<(), String> {
    invoke(backend.inner().clone(), move |store| {
        device::pin(store, &id, pinned)
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
        device::position(store, &id, &revision, line)
    })
    .await
}
