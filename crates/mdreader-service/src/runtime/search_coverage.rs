//! Short read-only coverage observations over the same retained query scope.
use super::{ModelState, Runtime};
use crate::protocol_error;
use mdreader_core::{Result, SearchCoverage, SearchDocumentReceipt, SearchModelState};

impl Runtime {
    fn coverage_devices(&self) -> Result<Vec<crate::catalog::DeviceSearchReceipt>> {
        #[cfg(feature = "sync-native")]
        if let Some(device) = &self.device {
            return Ok(device
                .search_sources()
                .map_err(|_| protocol_error("Device search coverage unavailable"))?
                .into_iter()
                .map(Into::into)
                .collect());
        }
        // Embedded composition has the same canonical namespace but no desktop
        // coordinator. Its read-only catalog still supplies exact current rows.
        #[cfg(feature = "sync-native")]
        {
            let store = self.queries()?;
            let mut result = Vec::new();
            let mut after = None;
            let mut snapshot = None;
            loop {
                let page = mdreader_core::device::catalog::page(
                    &store,
                    after.as_deref(),
                    snapshot.as_ref(),
                    100,
                )
                .map_err(|_| protocol_error("Device search coverage unavailable"))?;
                result.extend(
                    page.documents
                        .iter()
                        .map(|d| crate::catalog::DeviceSearchReceipt {
                            document_id: d.id.clone(),
                            source_id: d.source_id.clone(),
                            revision: d.revision.clone(),
                        }),
                );
                if result.len() > mdreader_core::MAX_SEARCH_SOURCES {
                    return Err(protocol_error("Device coverage budget exceeded"));
                }
                if page.next_after.is_none() {
                    return Ok(result);
                }
                after = page.next_after;
                snapshot = page.snapshot;
            }
        }
        #[cfg(not(feature = "sync-native"))]
        Ok(Vec::new())
    }

    pub(super) fn search_coverage(&self, sources: &[String]) -> Result<SearchCoverage> {
        let devices = self.coverage_devices()?;
        let exact: Vec<_> = devices
            .iter()
            .filter(|d| sources.contains(&d.source_id))
            .map(|d| SearchDocumentReceipt {
                source_id: d.source_id.clone(),
                path: "document.md".into(),
                revision: d.revision.clone(),
            })
            .collect();
        let (mut coverage, metadata) = {
            let mut store = self.queries()?;
            let before = store.search_coverage(sources, &exact)?;
            let metadata = store
                .with_library_connection(|db| {
                    crate::provider::search_coverage::observe(db, sources)
                })
                .map_err(|_| protocol_error("Provider search coverage unavailable"))?;
            if store.search_coverage(sources, &exact)? != before {
                return Err(mdreader_core::ReaderError::Conflict);
            }
            let after = store
                .with_library_connection(|db| {
                    crate::provider::search_coverage::observe(db, sources)
                })
                .map_err(|_| protocol_error("Provider search coverage unavailable"))?;
            if after != metadata {
                return Err(mdreader_core::ReaderError::Conflict);
            }
            (before, metadata)
        };
        if self.coverage_devices()? != devices {
            return Err(mdreader_core::ReaderError::Conflict);
        }
        for source in &mut coverage.sources {
            source.metadata = metadata.get(&source.source_id).cloned();
        }
        let token = serde_json::to_vec(&(&coverage.scope_token, &coverage.sources))
            .map_err(|_| protocol_error("Invalid search coverage"))?;
        coverage.scope_token = mdreader_provider::hex(&mdreader_provider::body_hash(&token));
        coverage.model = match self.model.as_ref().map(|m| m.phase()) {
            None => SearchModelState::Disabled,
            Some(ModelState::Dormant) => SearchModelState::NotDownloaded,
            Some(ModelState::Loading) => SearchModelState::Loading,
            Some(ModelState::Ready) => SearchModelState::Ready,
            Some(ModelState::Busy) => SearchModelState::Busy,
            Some(ModelState::RestartRequired) => SearchModelState::RestartRequired,
            Some(ModelState::Unavailable) => SearchModelState::Unavailable,
        };
        Ok(coverage)
    }
}

#[cfg(all(test, feature = "sync-native"))]
mod tests {
    use crate::{EmbeddedRuntime, Request, ServiceOptions};
    use mdreader_core::{ReaderStore, SearchCoverage, SearchModelState};
    use std::sync::{Arc, Mutex};

    #[test]
    fn embedded_coverage_uses_current_device_catalog_receipt_without_loading_model() {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("profile");
        let roots = t.path().join("documents");
        let mut store = ReaderStore::open(&profile).unwrap();
        let document = mdreader_core::device::import(
            &mut store,
            &roots,
            "selected.md".into(),
            "# Selected\n\nquartz".into(),
        )
        .unwrap();
        #[cfg(unix)]
        std::fs::write(
            roots.join(&document.id).join("other.md"),
            "# Not selected\n\nother text",
        )
        .unwrap();
        store.index_source(&document.source_id).unwrap();
        store
            .record_index_job(&document.source_id, "done", None)
            .unwrap();
        let runtime = EmbeddedRuntime::new(
            Arc::new(Mutex::new(store)),
            &profile,
            ServiceOptions {
                disable_model: true,
            },
        )
        .unwrap();
        let observed: SearchCoverage = serde_json::from_value(
            runtime
                .execute(Request::SearchCoverage {
                    sources: vec![document.source_id.clone()],
                })
                .unwrap(),
        )
        .unwrap();
        assert_eq!(observed.model, SearchModelState::Disabled);
        assert_eq!(observed.sources.len(), 1);
        assert_eq!(observed.sources[0].indexed_documents, 1);
        assert_eq!(observed.sources[0].eligible_documents, Some(1));
        assert_eq!(observed.sources[0].embedded_chunks, 0);
        assert_eq!(observed.sources[0].admitted_bodies, None);
        assert_eq!(observed.sources[0].metadata, None);
        assert!(!profile.join("models").exists());
    }
}
