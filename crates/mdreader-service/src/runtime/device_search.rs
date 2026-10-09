//! Mobile model consent and retrieval on the shared embedded owner.
use super::Runtime;
use crate::runtime::model;
use crate::{protocol_error, SearchMode};
use mdreader_core::{device, ReaderStore, Result};
use serde::Serialize;

#[derive(Serialize)]
pub struct DeviceSearchStatus {
    pub enabled: bool,
    pub model: &'static str,
    pub documents: u64,
    pub embedded_documents: u64,
}
#[derive(Serialize)]
pub struct DeviceSearchOutcome {
    pub hits: Vec<device::DeviceHit>,
    pub semantic_unavailable: Option<String>,
}

pub(super) fn initialize(store: &mut ReaderStore) -> Result<bool> {
    Ok(store.with_library_connection(|db| {
        db.execute_batch("CREATE TABLE IF NOT EXISTS reader_device_search_v1(id INTEGER PRIMARY KEY CHECK(id=1),enabled INTEGER NOT NULL CHECK(enabled IN(0,1))); INSERT OR IGNORE INTO reader_device_search_v1 VALUES(1,0);")?;
        db.query_row("SELECT enabled FROM reader_device_search_v1 WHERE id=1", [], |row| row.get(0))
    })?)
}
fn permission(store: &mut ReaderStore) -> Result<bool> {
    Ok(store.with_library_connection(|db| {
        db.query_row(
            "SELECT enabled FROM reader_device_search_v1 WHERE id=1",
            [],
            |row| row.get(0),
        )
    })?)
}
pub(super) fn status(runtime: &Runtime) -> Result<DeviceSearchStatus> {
    let mut store = runtime.store()?;
    let enabled = permission(&mut store)?;
    let documents =
        device::list(&mut store).map_err(|_| protocol_error("Device library unavailable"))?;
    let embedded_documents = documents
        .iter()
        .filter(|doc| {
            store
                .index_status(std::slice::from_ref(&doc.source_id))
                .is_ok_and(|index| {
                    index.documents > 0 && index.embedded == index.chunks && index.chunks > 0
                })
        })
        .count() as u64;
    let model = match runtime.model.as_ref().map(model::ModelActor::phase) {
        _ if !enabled => "disabled",
        Some(model::ModelState::Dormant) => "not_downloaded",
        Some(model::ModelState::Loading) => "loading",
        Some(model::ModelState::Ready | model::ModelState::Busy) => "ready",
        Some(model::ModelState::RestartRequired) => "restart_required",
        _ => "unavailable",
    };
    Ok(DeviceSearchStatus {
        enabled,
        model,
        documents: documents.len() as u64,
        embedded_documents,
    })
}
pub(super) fn set_enabled(runtime: &Runtime, enabled: bool) -> Result<DeviceSearchStatus> {
    let _effect = runtime
        .daemon_admission
        .enter()
        .map_err(|_| protocol_error("Reader is suspended"))?;
    let model = runtime
        .model
        .as_ref()
        .ok_or(mdreader_core::ReaderError::ModelUnavailable)?;
    {
        let mut store = runtime.store()?;
        permission(&mut store)?;
        store.with_library_connection(|db| {
            db.execute(
                "UPDATE reader_device_search_v1 SET enabled=?1 WHERE id=1",
                [enabled],
            )
        })?;
        model.set_enabled(enabled);
        if enabled {
            for doc in device::list(&mut store)
                .map_err(|_| protocol_error("Device library unavailable"))?
            {
                store.queue_index(&doc.source_id)?;
            }
        }
    }
    if enabled {
        model.ensure_loaded_owned(Some(&_effect));
    }
    let _ = runtime.wake.try_send(());
    status(runtime)
}
pub(super) fn search(
    runtime: &Runtime,
    query: &str,
    mode: SearchMode,
) -> Result<DeviceSearchOutcome> {
    let effect = runtime
        .daemon_admission
        .enter()
        .map_err(|_| protocol_error("Reader is suspended"))?;
    if query.len() > 1024 {
        return Err(protocol_error("Search query is too long"));
    }
    let wants_model = matches!(mode, SearchMode::Hybrid | SearchMode::Semantic);
    let text = ReaderStore::semantic_query_text(query)?;
    let vector = if wants_model && !text.is_empty() {
        runtime.model.as_ref().and_then(|model| {
            model.ensure_loaded_owned(Some(&effect));
            model
                .query_owned(text, model::QUERY_DEADLINE, Some(&effect))
                .ok()
        })
    } else {
        None
    };
    // Take fresh receipts AFTER inference; no SQLite mutex spans model work.
    let documents = device::list(&mut *runtime.store()?)
        .map_err(|_| protocol_error("Device library unavailable"))?;
    let hits = device::ranked_search(
        &*runtime.queries()?,
        &documents,
        query,
        vector.as_deref(),
        matches!(mode, SearchMode::Semantic),
    )?;
    Ok(DeviceSearchOutcome {
        hits,
        semantic_unavailable: (wants_model && vector.is_none() && !query.trim().is_empty())
            .then(|| "Semantic search is not ready. Text search remains available.".into()),
    })
}
