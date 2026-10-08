//! Thin mobile IPC adapter. File validation and snapshot hashing run outside the
//! embedded reader owner's mutex; native authority is checked again afterwards.
use crate::{
    mobile_assets::{AssetReply, AssetRequest, AssetRuntime},
    reader::ReaderBackend,
};
use std::{path::PathBuf, time::Instant};
use tauri::Manager;

async fn owner<T: Send + 'static>(
    backend: ReaderBackend,
    operation: impl FnOnce(&mut mdreader_core::ReaderStore) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || backend.with_mobile_store(operation))
        .await
        .map_err(|_| "Device asset worker failed.".to_string())?
}
async fn work<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|_| "Device asset worker failed.".to_string())?
}
/// Called only with the profile's embedded owner and its private app directory.
/// Kept separate from the command wrapper so tests can drive the real owner.
async fn dispatch(
    backend: ReaderBackend,
    runtime: AssetRuntime,
    directory: PathBuf,
    request: AssetRequest,
) -> Result<AssetReply, String> {
    match request {
        AssetRequest::Commit { intent_id } => {
            let permit = runtime.validation_permit()?;
            let prepare_runtime = runtime.clone();
            let prepare_directory = directory.clone();
            let prepared = owner(backend.clone(), move |store| {
                prepare_runtime
                    .assets(store, &prepare_directory)?
                    .prepare_commit(store, &intent_id, permit)
            })
            .await?;
            let validated = work(move || prepared.validate()).await?;
            owner(backend, move |store| {
                let assets = runtime.assets(store, &directory)?;
                let (receipt, manifest) = assets.publish(store, validated)?;
                Ok(AssetReply::Published { receipt, manifest })
            })
            .await
        }
        AssetRequest::Open {
            document_id,
            revision,
            asset_id,
            manifest,
        } => {
            let permit = runtime.read_permit(Instant::now())?;
            let prepare_runtime = runtime.clone();
            let prepare_directory = directory.clone();
            let prepared = owner(backend.clone(), move |store| {
                prepare_runtime
                    .assets(store, &prepare_directory)?
                    .prepare_open(store, &document_id, &revision, &asset_id, &manifest, permit)
            })
            .await?;
            let snapshot = work(move || prepared.verify()).await?;
            owner(backend, move |store| {
                let assets = runtime.assets(store, &directory)?;
                let (session_id, receipt) =
                    runtime.install(&assets, store, snapshot, Instant::now())?;
                Ok(AssetReply::Opened {
                    session_id,
                    receipt,
                })
            })
            .await
        }
        other => {
            owner(backend, move |store| {
                let assets = runtime.assets(store, &directory)?;
                match other {
                    AssetRequest::Inventory => Ok(AssetReply::Inventory {
                        value: assets.inventory(store)?,
                    }),
                    AssetRequest::Forget { asset_id } => {
                        assets.forget(store, &asset_id)?;
                        Ok(AssetReply::Complete)
                    }
                    AssetRequest::Manifest {
                        document_id,
                        revision,
                    } => Ok(AssetReply::Manifest {
                        value: assets.manifest(store, &document_id, &revision)?,
                    }),
                    AssetRequest::Begin {
                        document_id,
                        revision,
                        target,
                        bytes,
                        expected_manifest,
                    } => Ok(AssetReply::Intent {
                        intent_id: assets.begin(
                            store,
                            &document_id,
                            &revision,
                            &target,
                            bytes,
                            &expected_manifest,
                        )?,
                        received: 0,
                    }),
                    AssetRequest::Write {
                        intent_id,
                        offset,
                        base64,
                    } => Ok(AssetReply::Intent {
                        received: assets.write(store, &intent_id, offset, &base64)?,
                        intent_id,
                    }),
                    AssetRequest::Discard { intent_id } => {
                        assets.discard(store, &intent_id)?;
                        Ok(AssetReply::Complete)
                    }
                    AssetRequest::Remove {
                        document_id,
                        revision,
                        asset_id,
                        expected_manifest,
                    } => {
                        assets.remove(
                            store,
                            &document_id,
                            &revision,
                            &asset_id,
                            &expected_manifest,
                        )?;
                        Ok(AssetReply::Complete)
                    }
                    AssetRequest::Chunk { session_id, offset } => {
                        runtime.chunk(&assets, store, &session_id, offset, Instant::now())
                    }
                    AssetRequest::Close { session_id } => {
                        runtime.close(&session_id)?;
                        Ok(AssetReply::Complete)
                    }
                    AssetRequest::Commit { .. } | AssetRequest::Open { .. } => {
                        Err("Invalid device asset operation.".into())
                    }
                }
            })
            .await
        }
    }
}
#[tauri::command]
#[specta::specta]
pub async fn mobile_reader_assets(
    app: tauri::AppHandle,
    backend: tauri::State<'_, ReaderBackend>,
    runtime: tauri::State<'_, AssetRuntime>,
    request: AssetRequest,
) -> Result<AssetReply, String> {
    if !cfg!(mobile) {
        return Err("Selected device assets are supported by the native mobile reader.".into());
    }
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|_| "Device asset directory is unavailable.".to_string())?
        .join("device-assets-v1");
    dispatch(
        backend.inner().clone(),
        runtime.inner().clone(),
        directory,
        request,
    )
    .await
}
