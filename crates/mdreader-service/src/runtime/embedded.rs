//! In-process search owner for mobile. Reuses desktop indexing/model deadlines,
//! fairness, revision checks and background budgets without opening an IPC port,
//! initializing agent capture, or creating a second SQLite writer.
use super::{Runtime, model::ModelActor};
use crate::{Request, ServiceOptions, protocol_error};
use mdreader_core::{ReaderStore, Result};
use serde_json::Value;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

pub struct EmbeddedRuntime(Runtime);
impl EmbeddedRuntime {
    /// The app supplies its existing writer after account namespaces have been
    /// initialized. Construction is lazy: no model download occurs until queued
    /// indexing/search needs inference, or persisted vectors need query support.
    pub fn new(
        store: Arc<Mutex<ReaderStore>>,
        directory: &Path,
        options: ServiceOptions,
    ) -> Result<Self> {
        #[cfg(unix)]
        let identity = {
            let writer = store
                .lock()
                .map_err(|_| mdreader_core::ReaderError::Unavailable)?;
            let identity = writer.library_identity()?;
            identity.verify_at(directory)?;
            identity
        };
        #[cfg(feature = "local-embed")]
        let model =
            (!options.disable_model).then(|| ModelActor::new(directory.join("models"), false));
        #[cfg(not(feature = "local-embed"))]
        let model: Option<ModelActor> = {
            let _ = options;
            None
        };
        let runtime = Self(Runtime::with_host(
            store,
            directory.join("models"),
            model,
            None,
            super::Host::Embedded,
        )?);
        #[cfg(unix)]
        identity.verify_at(directory)?;
        Ok(runtime)
    }
    /// Mobile composition handles account/device operations separately over the
    /// same writer. This adapter exposes no desktop capture/provider operations.
    pub fn execute(&self, request: Request) -> Result<Value> {
        if !matches!(
            &request,
            Request::Ping
                | Request::EmbeddingPolicy
                | Request::SetEmbeddingPolicy { .. }
                | Request::Admit { .. }
                | Request::Index { .. }
                | Request::StartIndex { .. }
                | Request::ListSources
                | Request::FindSource { .. }
                | Request::Status { .. }
                | Request::Search { .. }
                | Request::SearchRanked { .. }
                | Request::Read { .. }
                | Request::Resolve { .. }
                | Request::Related { .. }
                | Request::RelatedRanked { .. }
                | Request::Bookmark { .. }
                | Request::SetBookmark { .. }
                | Request::JobStatus { .. }
                | Request::SetSourceEnabled { .. }
                | Request::ForgetSourceIndex { .. }
                | Request::ListDocuments { .. }
        ) {
            return Err(protocol_error(
                "Operation requires a composed native adapter",
            ));
        }
        self.0.execute(request)
    }
    /// Nonblocking lifecycle callback. Stops NEW scheduler/model admission;
    /// an already issued non-cancellable inference may finish its bounded turn.
    pub fn set_active(&self, active: bool) {
        self.0.daemon_admission.set_suspended(!active);
        let _ = self.0.wake.try_send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn shared_writer_indexes_after_resume_and_queries_do_not_wait_on_writer() {
        let library = tempfile::tempdir().unwrap();
        let notes = tempfile::tempdir().unwrap();
        std::fs::write(
            notes.path().join("note.md"),
            "# Public fixture\nMobileindexmarker",
        )
        .unwrap();
        let mut store = ReaderStore::open(library.path()).unwrap();
        let source = store
            .add_local_source(
                "Notes",
                mdreader_core::SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
            )
            .unwrap()
            .id;
        let store = Arc::new(Mutex::new(store));
        #[cfg(unix)]
        {
            let other = tempfile::tempdir().unwrap();
            assert!(
                EmbeddedRuntime::new(
                    store.clone(),
                    other.path(),
                    ServiceOptions {
                        disable_model: true,
                    }
                )
                .is_err(),
                "a different directory must not replace the supplied writer's library"
            );
        }
        let runtime = EmbeddedRuntime::new(
            store.clone(),
            library.path(),
            ServiceOptions {
                disable_model: true,
            },
        )
        .unwrap();
        runtime.set_active(false);
        assert!(
            runtime
                .execute(Request::StartIndex {
                    source_id: source.clone()
                })
                .is_err()
        );
        // Queue through the actual supplied owner while OS admission is closed.
        store.lock().unwrap().queue_index(&source).unwrap();
        runtime.set_active(true);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let result = runtime
                .execute(Request::Search {
                    query: "Mobileindexmarker".into(),
                    sources: vec![source.clone()],
                    limit: 10,
                })
                .unwrap();
            if !result.as_array().unwrap().is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "embedded scheduler did not resume"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut writer = store.lock().unwrap();
        let result = runtime
            .execute(Request::SearchRanked {
                query: "Mobileindexmarker".into(),
                sources: vec![source],
                limit: 10,
                mode: crate::SearchMode::Hybrid,
            })
            .unwrap();
        assert_eq!(result["hits"].as_array().unwrap().len(), 1);
        assert!(result["semantic_unavailable"].is_string());
        assert!(runtime.execute(Request::CaptureStatus).is_err());
        let desktop_tables: u32 = writer
            .with_library_connection(|db| {
                db.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name LIKE 'reader_reviews%'",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(desktop_tables, 0);
        drop(writer);
    }
}
