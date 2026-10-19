//! Short, local-only catalog observations and bounded opaque continuations.
#[cfg(feature = "sync-native")]
mod device;
mod model;
mod observation;
use crate::provider;
use mdreader_core::ReaderStore;
use mdreader_provider::SourceId;
pub use model::*;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;
type Result<T> = std::result::Result<T, CatalogError>;
#[derive(Clone)]
enum Snapshot {
    #[cfg(feature = "sync-native")]
    Device(mdreader_core::device::catalog::Snapshot),
    Local(mdreader_core::catalog::Snapshot),
    Provider(provider::catalog::CatalogSnapshot),
}
#[derive(Clone)]
struct Cursor {
    catalog_id: String,
    after: String,
    snapshot: Snapshot,
    limit: u32,
    created: Instant,
}
pub struct Coordinator {
    #[cfg(feature = "sync-native")]
    device: Option<Arc<crate::device::Coordinator>>,
    owner: Arc<Mutex<ReaderStore>>,
    queries: Arc<Mutex<ReaderStore>>,
    cursors: Mutex<HashMap<Uuid, Cursor>>,
    captures: Option<std::path::PathBuf>,
}
impl Coordinator {
    pub fn new(owner: Arc<Mutex<ReaderStore>>, queries: Arc<Mutex<ReaderStore>>) -> Self {
        Self {
            #[cfg(feature = "sync-native")]
            device: None,
            owner,
            queries,
            cursors: Mutex::new(HashMap::new()),
            captures: None,
        }
    }
    pub(crate) fn with_captures(mut self, directory: std::path::PathBuf) -> Self {
        self.captures = Some(directory);
        self
    }
    #[cfg(feature = "sync-native")]
    pub fn enable_device_catalog(&mut self, device: Arc<crate::device::Coordinator>) {
        self.device = Some(device);
    }
    pub fn execute(&self, request: CatalogRequest) -> Result<CatalogReply> {
        match request {
            CatalogRequest::Close { cursor } => {
                let id = Uuid::parse_str(&cursor).map_err(|_| CatalogError::Invalid)?;
                self.cursors
                    .lock()
                    .map_err(|_| CatalogError::Persistence)?
                    .remove(&id);
                Ok(CatalogReply::Complete)
            }
            CatalogRequest::List { local_sources } => self.list(local_sources),
            CatalogRequest::Page {
                catalog_id,
                cursor,
                limit,
            } => self.page(catalog_id, cursor, limit),
        }
    }
    fn list(&self, local: Vec<String>) -> Result<CatalogReply> {
        if local.len() > 128
            || local
                .iter()
                .any(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(CatalogError::Invalid);
        }
        let queries = self.queries.lock().map_err(|_| CatalogError::Persistence)?;
        let mut sources = Vec::new();
        #[cfg(feature = "sync-native")]
        if let Some(device) = &self.device {
            match device.with_catalog(|directory| self::device::source(&queries, directory)) {
                Ok(Some(source)) => sources.push(source),
                Ok(None) => {}
                Err(error) => sources.push(self::device::header(Some(
                    self::device::service_error(error),
                ))),
            }
        }
        let captures = self
            .captures
            .as_ref()
            .map(|directory| crate::capture::reader::sources(&queries, directory))
            .transpose()
            .map_err(core_error)?
            .unwrap_or_default();
        let mut selected: Vec<_> = local.into_iter().map(|id| (id, false)).collect();
        selected.extend(captures.into_iter().map(|s| (s.id, true)));
        for (id, capture) in selected {
            let source = match queries.catalog_source(&id) {
                Ok(source) => source,
                Err(error) => {
                    sources.push(CatalogSource {
                        catalog_id: format!("{}:{id}", if capture { "capture" } else { "local" }),
                        name: "Unavailable folder".into(),
                        kind: if capture { "capture" } else { "local" }.into(),
                        enabled: false,
                        coverage: "unavailable".into(),
                        observed_at_ms: None,
                        failure: Some(core_error(error)),
                    });
                    continue;
                }
            };
            let page = queries.catalog_page(&id, None, None, 1);
            sources.push(CatalogSource {
                catalog_id: format!("{}:{id}", if capture { "capture" } else { "local" }),
                name: source.name.clone(),
                kind: if capture { "capture" } else { "local" }.into(),
                enabled: source.enabled,
                coverage: match &page {
                    Ok(p) if p.snapshot.is_some() && p.indexed_at_ms.is_none() => {
                        "historical_index_coverage_unknown"
                    }
                    Ok(p) if p.snapshot.is_some() => "indexed_observation",
                    Ok(_) => "not_indexed",
                    Err(_) => "unavailable",
                }
                .into(),
                observed_at_ms: page.as_ref().ok().and_then(|p| p.indexed_at_ms),
                failure: page.err().map(core_error),
            });
        }
        drop(queries);
        let providers = observation::providers(&self.owner);
        let provider_failure = match providers {
            Ok(rows) => {
                sources.extend(rows);
                None
            }
            Err(e) => Some(e),
        };
        Ok(CatalogReply::List {
            sources,
            provider_failure,
        })
    }
    fn capture_selected(&self, store: &ReaderStore, id: &str) -> Result<bool> {
        let Some(directory) = &self.captures else {
            return Ok(false);
        };
        Ok(crate::capture::reader::sources(store, directory)
            .map_err(core_error)?
            .iter()
            .any(|s| s.id == id))
    }
    fn page(&self, id: String, cursor: Option<String>, limit: u32) -> Result<CatalogReply> {
        if !(1..=100).contains(&limit) || id.len() > 256 {
            return Err(CatalogError::Invalid);
        }
        let mut cursors = self.cursors.lock().map_err(|_| CatalogError::Persistence)?;
        cursors.retain(|_, c| c.created.elapsed() < Duration::from_secs(1800));
        let cursor_id = cursor
            .as_deref()
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|_| CatalogError::Invalid)?;
        let previous = cursor_id
            .map(|key| cursors.get(&key).cloned().ok_or(CatalogError::Stale))
            .transpose()?;
        if previous
            .as_ref()
            .is_some_and(|c| c.catalog_id != id || c.limit != limit)
        {
            return Err(CatalogError::Invalid);
        }
        if previous.is_none() && cursors.len() >= 8 {
            return Err(CatalogError::Busy);
        }
        drop(cursors);
        let after = previous.as_ref().map(|c| c.after.as_str());
        let (source, documents, next) = if let Some(local) = id
            .strip_prefix("local:")
            .or_else(|| id.strip_prefix("capture:"))
        {
            let capture = id.starts_with("capture:");
            if local.len() != 64 || !local.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(CatalogError::Invalid);
            }
            let expected = match previous.as_ref().map(|p| &p.snapshot) {
                Some(Snapshot::Local(s)) => Some(s),
                Some(_) => return Err(CatalogError::Invalid),
                None => None,
            };
            let queries = self.queries.lock().map_err(|_| CatalogError::Persistence)?;
            if capture && !self.capture_selected(&queries, local)? {
                return Err(CatalogError::Unauthorized);
            }
            if !queries.catalog_source(local).map_err(core_error)?.enabled {
                return Err(CatalogError::Unauthorized);
            }
            let p = queries
                .catalog_page(local, after, expected, limit as usize)
                .map_err(core_error)?;
            if capture && !self.capture_selected(&queries, local)? {
                return Err(CatalogError::Stale);
            }
            let source = CatalogSource {
                catalog_id: id.clone(),
                name: p.source_name,
                kind: if capture { "capture" } else { "local" }.into(),
                enabled: true,
                coverage: if p.snapshot.is_none() {
                    "not_indexed"
                } else if p.indexed_at_ms.is_none() {
                    "historical_index_coverage_unknown"
                } else {
                    "indexed_observation"
                }
                .into(),
                observed_at_ms: p.indexed_at_ms,
                failure: None,
            };
            let documents = p
                .documents
                .into_iter()
                .map(|d| CatalogDocument {
                    name: d.title,
                    display_path: d.path.clone(),
                    modified_at_ms: Some(d.modified_ms),
                    size_bytes: None,
                    observed_at_ms: p.indexed_at_ms,
                    observation: if d.pending {
                        "discovered_local"
                    } else {
                        "indexed_local"
                    }
                    .into(),
                    open: if d.pending {
                        CatalogOpen::LocalPending {
                            source_id: local.into(),
                            path: d.path,
                        }
                    } else {
                        CatalogOpen::Exact {
                            source_id: local.into(),
                            path: d.path,
                            revision: d.revision,
                        }
                    },
                })
                .collect();
            (
                source,
                documents,
                p.next_after
                    .zip(p.snapshot)
                    .map(|(after, s)| (after, Snapshot::Local(s))),
            )
        } else if let Some(remote) = id.strip_prefix("remote:") {
            let remote = SourceId::new(remote).map_err(provider_error)?;
            let expected = match previous.as_ref().map(|p| &p.snapshot) {
                Some(Snapshot::Provider(s)) => Some(s),
                Some(_) => return Err(CatalogError::Invalid),
                None => None,
            };
            let mut owner = self.owner.try_lock().map_err(|error| match error {
                std::sync::TryLockError::WouldBlock => CatalogError::Busy,
                std::sync::TryLockError::Poisoned(_) => CatalogError::Persistence,
            })?;
            owner
                .with_library_connection(|c| {
                    let source = provider::catalog::catalog_sources(c)?
                        .into_iter()
                        .find(|s| s.catalog_id == id)
                        .ok_or(mdreader_provider::ProviderError::Unavailable)?;
                    let page = provider::catalog::catalog_page(
                        c,
                        &remote,
                        after,
                        expected,
                        limit as usize,
                    )?;
                    Ok((
                        source,
                        page.documents,
                        page.next_after
                            .map(|after| (after, Snapshot::Provider(page.snapshot))),
                    ))
                })
                .map_err(provider_error)?
        } else if id == "device:library" {
            #[cfg(feature = "sync-native")]
            {
                let device = self.device.as_ref().ok_or(CatalogError::Unsupported)?;
                let expected = match previous.as_ref().map(|p| &p.snapshot) {
                    Some(Snapshot::Device(s)) => Some(s),
                    Some(_) => return Err(CatalogError::Invalid),
                    None => None,
                };
                let queries = self.queries.try_lock().map_err(|_| CatalogError::Busy)?;
                let page = device
                    .with_catalog(|directory| {
                        mdreader_core::device::catalog::page_in(
                            &queries,
                            directory,
                            after,
                            expected,
                            limit as usize,
                        )
                    })
                    .map_err(self::device::service_error)?
                    .map_err(self::device::error)?;
                let source = device::header(None);
                let documents = page.documents.into_iter().map(device::document).collect();
                (
                    source,
                    documents,
                    page.next_after
                        .zip(page.snapshot)
                        .map(|(after, snapshot)| (after, Snapshot::Device(snapshot))),
                )
            }
            #[cfg(not(feature = "sync-native"))]
            {
                return Err(CatalogError::Unsupported);
            }
        } else {
            return Err(CatalogError::Invalid);
        };
        let mut cursors = self.cursors.lock().map_err(|_| CatalogError::Persistence)?;
        cursors.retain(|_, c| c.created.elapsed() < Duration::from_secs(1800));
        if cursor_id.is_some_and(|id| !cursors.contains_key(&id)) {
            return Err(CatalogError::Stale);
        }
        if cursor_id.is_none() && next.is_some() && cursors.len() >= 8 {
            return Err(CatalogError::Busy);
        }
        let next_cursor = if let Some((after, snapshot)) = next {
            let key = Uuid::new_v4();
            if let Some(previous) = cursor_id {
                cursors.remove(&previous);
            }
            cursors.insert(
                key,
                Cursor {
                    catalog_id: id,
                    after,
                    snapshot,
                    limit,
                    created: Instant::now(),
                },
            );
            Some(key.to_string())
        } else {
            if let Some(key) = cursor_id {
                cursors.remove(&key);
            }
            None
        };
        Ok(CatalogReply::Page {
            source,
            documents,
            next_cursor,
        })
    }
}
