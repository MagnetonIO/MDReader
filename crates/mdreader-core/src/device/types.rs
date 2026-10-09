use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Serialize)]
pub struct DeviceRead {
    pub document: DeviceDocument,
    pub markdown: String,
}
#[derive(Debug, Serialize)]
pub struct DeviceHit {
    pub document: DeviceDocument,
    pub line: u32,
    pub text: String,
}
#[derive(Debug, Serialize)]
pub struct PendingDeviceImport {
    pub id: String,
    pub name: String,
    pub bytes: u32,
}
#[derive(Debug, Serialize)]
pub struct DeviceLibrary {
    pub documents: Vec<DeviceDocument>,
    pub pending: Vec<PendingDeviceImport>,
}
#[derive(Clone)]
pub(super) struct ImportIntent {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) revision: String,
    pub(super) bytes: u32,
    pub(super) parent_identity: String,
    pub(super) root_identity: Option<String>,
}
