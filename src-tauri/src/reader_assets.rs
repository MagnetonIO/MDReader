//! Exact saved-origin receipts and bounded retained-source asset adapters.
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Clone, Deserialize, Serialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceOriginReceipt {
    pub epoch: String,
    pub path: String,
    pub hash: String,
}
impl std::fmt::Debug for WorkspaceOriginReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkspaceOriginReceipt(<retained saved origin>)")
    }
}

impl WorkspaceOriginReceipt {
    pub(crate) fn from_read(
        epoch: &crate::engine::Epoch,
        path: &std::path::Path,
        hash: &str,
    ) -> Result<Self, String> {
        let parts = path
            .components()
            .map(|part| match part {
                std::path::Component::Normal(part) => part
                    .to_str()
                    .ok_or_else(|| "unsupported document name".to_owned()),
                _ => Err("invalid document name".to_owned()),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            epoch: epoch.read_receipt.to_string(),
            path: parts.join("/"),
            hash: hash.into(),
        })
    }
}

#[cfg(not(mobile))]
mod model;
#[cfg(not(mobile))]
mod path;
#[cfg(not(mobile))]
mod proof;
#[cfg(not(mobile))]
mod workspace;
#[cfg(not(mobile))]
pub use model::*;
#[cfg(not(mobile))]
pub(crate) use proof::{AssetObservation, BoundedFile, Permit};

#[cfg(all(test, not(mobile)))]
pub(crate) fn image_path(origin: &str, raw: &str) -> Result<std::path::PathBuf, AssetReadError> {
    let target = path::resolve(origin, raw)?;
    if target.fragment.is_some() {
        return Err(AssetFailure::Invalid.error());
    }
    Ok(target.path.into())
}
#[cfg(all(test, not(mobile)))]
pub(crate) fn image_referenced(markdown: &str, origin: &str, target: &std::path::Path) -> bool {
    target.to_str().is_some_and(|target| {
        proof::referenced(
            markdown,
            origin,
            &path::Target {
                path: target.into(),
                fragment: None,
            },
            AssetReferenceKind::Image,
        )
    })
}

#[cfg(not(mobile))]
#[tauri::command(async)]
#[specta::specta]
pub fn reader_workspace_asset(
    state: tauri::State<'_, crate::commands::WorkspaceState>,
    request: WorkspaceAssetRequest,
) -> Result<AssetPreview, AssetReadError> {
    workspace::read(&state, request)
}
