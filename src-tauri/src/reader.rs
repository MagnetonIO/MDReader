//! Platform boundary for the MDReader-owned SQLite service. Desktop windows
//! never open the SQLite files; mobile embeds the same dispatcher.
use mdreader_core::SourceRoot;
use mdreader_service::Request;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[path = "reader/admission.rs"]
mod admission;
#[cfg(any(mobile, test))]
#[path = "reader/embedded.rs"]
mod embedded;

#[derive(Clone)]
pub(crate) struct ReaderBackend(Arc<Transport>, Arc<admission::Gate>);
enum Transport {
    #[cfg(not(mobile))]
    Desktop {
        directory: PathBuf,
        executable: PathBuf,
    },
    #[cfg(any(mobile, test))]
    Embedded(embedded::Owner),
}
impl ReaderBackend {
    pub(crate) fn bind_profile_admission(
        &self,
        profile: &Arc<crate::profile_transition::NativeProfileTransition>,
    ) -> Result<(), String> {
        self.1.bind(profile)
    }
    /// Preserve typed service failures at native owner boundaries. Renderer
    /// adapters may format these later; provider revisions must remain conflicts.
    #[cfg(not(mobile))]
    pub(crate) fn call_native(&self, request: Request) -> mdreader_core::Result<Value> {
        let _lease = self
            .1
            .enter()
            .map_err(|_| mdreader_core::ReaderError::Unavailable)?;
        match self.0.as_ref() {
            Transport::Desktop {
                directory,
                executable,
            } => {
                mdreader_service::ensure_running(
                    directory,
                    executable,
                    &["--mdreader-service".into()],
                )?;
                mdreader_service::call(directory, &request)
            }
            #[cfg(test)]
            Transport::Embedded(store) => {
                let mut store = store
                    .store
                    .lock()
                    .map_err(|_| mdreader_core::ReaderError::Unavailable)?;
                mdreader_service::execute(&mut store, request)
            }
        }
    }
    /// Mobile operations share the embedded owner's mutex and durable connection.
    /// A desktop renderer cannot open a second writer through this adapter.
    pub(crate) fn with_mobile_store<T>(
        &self,
        operation: impl FnOnce(&mut mdreader_core::ReaderStore) -> Result<T, String>,
    ) -> Result<T, String> {
        match self.0.as_ref() {
            #[cfg(any(mobile, test))]
            Transport::Embedded(store) => {
                let mut owner = store.store.lock().map_err(|_| "Reader storage failed")?;
                operation(&mut owner)
            }
            #[cfg(not(mobile))]
            Transport::Desktop { .. } => {
                let _ = operation;
                Err("Device library commands require the mobile owner".into())
            }
        }
    }
    pub(crate) fn open(directory: PathBuf) -> Result<Self, String> {
        #[cfg(not(mobile))]
        {
            Ok(Self(
                Arc::new(Transport::Desktop {
                    directory,
                    executable: std::env::current_exe().map_err(|e| e.to_string())?,
                }),
                Arc::default(),
            ))
        }
        #[cfg(mobile)]
        {
            Self::embedded(&directory)
        }
    }
    #[cfg(any(mobile, test))]
    pub(crate) fn embedded(directory: &Path) -> Result<Self, String> {
        Ok(Self(
            Arc::new(Transport::Embedded(embedded::Owner::open(directory)?)),
            Arc::default(),
        ))
    }
    pub(crate) fn call(&self, request: Request) -> Result<Value, String> {
        let _lease = self.1.enter()?;
        match self.0.as_ref() {
            #[cfg(not(mobile))]
            Transport::Desktop {
                directory,
                executable,
            } => {
                mdreader_service::ensure_running(
                    directory,
                    executable,
                    &["--mdreader-service".into()],
                )
                .map_err(|e| e.to_string())?;
                mdreader_service::call(directory, &request).map_err(|e| e.to_string())
            }
            #[cfg(any(mobile, test))]
            Transport::Embedded(store) => {
                #[cfg(mobile)]
                if let Request::Account { request } = &request {
                    return Ok(store.account(request.clone()));
                }
                let mut store = store.store.lock().map_err(|_| "Reader storage failed")?;
                if let Request::StartIndex { source_id } = &request {
                    store.queue_index(source_id).map_err(|e| e.to_string())?;
                    store.index_source(source_id).map_err(|e| e.to_string())?;
                    store.record_index_job(source_id,"model-unavailable",Some("Local mobile embedding runtime is not installed; lexical search remains available")).map_err(|e|e.to_string())?;
                    return serde_json::to_value(
                        store.index_job(source_id).map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string());
                }
                mdreader_service::execute(&mut store, request).map_err(|e| e.to_string())
            }
        }
    }
    #[cfg(mobile)]
    pub(crate) fn mobile_owner(&self) -> Result<Arc<std::sync::Mutex<mdreader_core::ReaderStore>>, String> {
        match self.0.as_ref() {
            Transport::Embedded(owner) => Ok(owner.store.clone()),
        }
    }
    #[cfg(mobile)]
    pub(crate) fn bind_mobile_accounts(
        &self,
        accounts: Result<Arc<mdreader_service::accounts::Coordinator>, mdreader_service::accounts::AccountError>,
    ) -> Result<(), String> {
        match self.0.as_ref() {
            Transport::Embedded(owner) => owner.bind_accounts(accounts),
        }
    }
    pub(crate) fn typed<T: DeserializeOwned>(&self, request: Request) -> Result<T, String> {
        serde_json::from_value(self.call(request)?)
            .map_err(|_| "Invalid reader service response".into())
    }
    pub(crate) async fn invoke<T: DeserializeOwned + Send + 'static>(
        &self,
        request: Request,
    ) -> Result<T, String> {
        let backend = self.clone();
        tauri::async_runtime::spawn_blocking(move || backend.typed(request))
            .await
            .map_err(|_| "Reader worker failed".to_string())?
    }
    pub(crate) fn admit(&self, root: &SourceRoot) -> Result<String, String> {
        root.verify().map_err(|e| e.to_string())?;
        let name = root
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Local source")
            .to_string();
        let source: mdreader_core::Source = self.typed(Request::Admit {
            name,
            path: root.path.clone(),
            identity: root.identity.clone(),
        })?;
        root.verify().map_err(|e| e.to_string())?;
        if !source.enabled {
            return Err("Source permission was revoked; activate the folder again".into());
        }
        Ok(source.id)
    }
    /// Called only by the explicit native Library activation path. Ordinary
    /// reads/import listeners use admit(), which cannot revive a revoked grant.
    pub(crate) fn grant(&self, root: &SourceRoot) -> Result<(), String> {
        root.verify().map_err(|e| e.to_string())?;
        let source: mdreader_core::Source = self.typed(Request::Admit {
            name: root
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Local source")
                .into(),
            path: root.path.clone(),
            identity: root.identity.clone(),
        })?;
        let _: Value = self.typed(Request::SetSourceEnabled {
            source_id: source.id,
            enabled: true,
        })?;
        root.verify().map_err(|e| e.to_string())
    }
    pub(crate) fn revoke_binding(
        &self,
        path: PathBuf,
        identity: mdreader_core::SourceRootIdentity,
    ) -> Result<(), String> {
        let source: Option<mdreader_core::Source> =
            self.typed(Request::FindSource { path, identity })?;
        if let Some(source) = source {
            let _: Value = self.typed(Request::ForgetSourceIndex {
                source_id: source.id,
            })?;
        }
        Ok(())
    }
    #[cfg(not(mobile))]
    pub(crate) fn pause_capture(&self, root: &Path) -> Result<(), String> {
        let policy: Option<mdreader_capture::ProjectPolicy> =
            self.typed(Request::CapturePolicy {
                project_id: worktree_project_id(root),
            })?;
        if let Some(mut policy) = policy {
            policy.enabled = false;
            let _: Value = self.typed(Request::ConfigureCapture { policy })?;
        }
        Ok(())
    }
    pub(crate) async fn admit_async(&self, root: SourceRoot) -> Result<String, String> {
        let backend = self.clone();
        tauri::async_runtime::spawn_blocking(move || backend.admit(&root))
            .await
            .map_err(|_| "Reader worker failed".to_string())?
    }
}

pub(crate) fn worktree_project_id(root: &Path) -> String {
    format!(
        "worktree:{}",
        uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            root.to_string_lossy().as_bytes()
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forgetting_revokes_reader_access_and_late_admission_until_explicit_activation() {
        let data = tempfile::tempdir().unwrap();
        let notes = tempfile::tempdir().unwrap();
        std::fs::write(notes.path().join("plan.md"), "# Plan\nOffline reading\n").unwrap();
        let root = SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap();
        let backend = ReaderBackend::embedded(data.path()).unwrap();
        let source = backend.admit(&root).unwrap();
        backend
            .call(Request::Index {
                source_id: source.clone(),
            })
            .unwrap();
        backend
            .revoke_binding(root.path.clone(), root.identity.clone())
            .unwrap();
        assert!(backend
            .call(Request::Search {
                query: "offline".into(),
                sources: vec![source.clone()],
                limit: 10
            })
            .is_err());
        assert!(backend.admit(&root).is_err());
        backend.grant(&root).unwrap();
        assert_eq!(backend.admit(&root).unwrap(), source);
        let missing: Vec<mdreader_core::Evidence> = backend
            .typed(Request::Search {
                query: "offline".into(),
                sources: vec![source.clone()],
                limit: 10,
            })
            .unwrap();
        assert!(
            missing.is_empty(),
            "Forget must delete the derived index, preserving original Markdown"
        );
        backend
            .call(Request::Index {
                source_id: source.clone(),
            })
            .unwrap();
        let hits: Vec<mdreader_core::Evidence> = backend
            .typed(Request::Search {
                query: "offline".into(),
                sources: vec![source],
                limit: 10,
            })
            .unwrap();
        assert_eq!(hits[0].path, "plan.md");
    }
    #[test]
    fn admitted_epoch_uses_sqlite_without_installing_a_legacy_engine() {
        let data = tempfile::tempdir().unwrap();
        let notes = tempfile::tempdir().unwrap();
        std::fs::write(notes.path().join("plan.md"), "# Plan\nOffline reading\n").unwrap();
        let workspace = crate::commands::WorkspaceState::default();
        let epoch = workspace
            .switch(notes.path().canonicalize().unwrap())
            .unwrap();
        let backend = ReaderBackend::embedded(data.path()).unwrap();
        let source = backend.admit(&epoch.source_root()).unwrap();
        backend
            .call(mdreader_service::Request::Index {
                source_id: source.clone(),
            })
            .unwrap();
        let hits: Vec<mdreader_core::Evidence> = backend
            .typed(mdreader_service::Request::Search {
                query: "offline".into(),
                sources: vec![source],
                limit: 10,
            })
            .unwrap();
        assert_eq!(hits[0].path, "plan.md");
        assert!(data.path().join("library.sqlite").exists());
        assert!(data.path().join("index.sqlite").exists());
        assert!(!data.path().join("catdb.db").exists());
    }
    #[tokio::test]
    async fn gui_search_uses_sqlite_evidence_and_discards_superseded_epochs() {
        let data = tempfile::tempdir().unwrap();
        let notes = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::write(notes.path().join("plan.md"), "# Plan\nOffline reading\n").unwrap();
        let workspace = crate::commands::WorkspaceState::default();
        let epoch = workspace
            .switch(notes.path().canonicalize().unwrap())
            .unwrap();
        let backend = ReaderBackend::embedded(data.path()).unwrap();
        let source = backend.admit(&epoch.source_root()).unwrap();
        backend.call(Request::Index { source_id: source }).unwrap();
        let options = crate::search::SearchQuery {
            limit: 10,
            lexical_only: false,
            explain: true,
            intent: Some("best".into()),
            provider_sources: None,
        };
        let result =
            crate::search::search_reader(&backend, &workspace, "offline", &options, |_| {})
                .await
                .unwrap();
        assert_eq!(result.hits[0].file_path, "plan.md");
        assert!(result.hits[0].revision.is_some());
        assert!(result.semantic_unavailable.is_some());
        let stale =
            crate::search::search_reader(&backend, &workspace, "offline", &options, |batch| {
                if batch.phase == "lexical" {
                    workspace
                        .switch(other.path().canonicalize().unwrap())
                        .unwrap();
                }
            })
            .await
            .unwrap();
        assert!(stale.hits.is_empty());
    }
    #[tokio::test]
    async fn related_reader_preserves_tag_evidence_revisions_and_missing_vector_coverage() {
        let data = tempfile::tempdir().unwrap();
        let notes = tempfile::tempdir().unwrap();
        std::fs::write(
            notes.path().join("anchor.md"),
            "---\ntags: [reader]\n---\n# Anchor\n[next](linked.md)\n",
        )
        .unwrap();
        std::fs::write(notes.path().join("linked.md"), "# Linked\n").unwrap();
        std::fs::write(
            notes.path().join("tagged.md"),
            "---\ntags: [reader]\n---\n# Tagged\n",
        )
        .unwrap();
        let workspace = crate::commands::WorkspaceState::default();
        let epoch = workspace
            .switch(notes.path().canonicalize().unwrap())
            .unwrap();
        let backend = ReaderBackend::embedded(data.path()).unwrap();
        let source = backend.admit(&epoch.source_root()).unwrap();
        backend
            .call(Request::Index {
                source_id: source.clone(),
            })
            .unwrap();
        let outcome = crate::search::related_reader(
            &backend,
            &workspace,
            epoch.root.join("anchor.md").to_str().unwrap(),
            10,
        )
        .await
        .unwrap();
        assert!(matches!(
            outcome.availability.vector,
            crate::search::RelatedVectorStateDto::NoAnchorVectors
        ));
        assert_eq!(outcome.hits.len(), 2);
        let tagged = outcome
            .hits
            .iter()
            .find(|h| h.file_path == "tagged.md")
            .unwrap();
        assert_eq!(tagged.source_id.as_deref(), Some(source.as_str()));
        assert_eq!(tagged.revision.as_ref().unwrap().len(), 64);
        assert_eq!(tagged.components.shared_tags, Some(0.5));
        assert_eq!(tagged.components.concept, 0.0);
        assert_eq!(tagged.components.vector, None);
        assert_eq!(
            tagged.abs_path,
            epoch.root.join("tagged.md").to_string_lossy()
        );
        let missing = crate::search::related_reader(
            &backend,
            &workspace,
            epoch.root.join("missing.md").to_str().unwrap(),
            10,
        )
        .await
        .unwrap();
        assert!(missing.hits.is_empty());
        assert!(matches!(
            missing.availability.vector,
            crate::search::RelatedVectorStateDto::AnchorMissing
        ));
    }
    #[tokio::test]
    async fn source_changes_requeue_even_when_an_import_progress_listener_is_active() {
        let data = tempfile::tempdir().unwrap();
        let notes = tempfile::tempdir().unwrap();
        std::fs::write(notes.path().join("note.md"), "# First\nFirstrevision").unwrap();
        let workspace = crate::commands::WorkspaceState::default();
        let epoch = workspace
            .switch(notes.path().canonicalize().unwrap())
            .unwrap();
        let backend = ReaderBackend::embedded(data.path()).unwrap();
        let (owner, source) = crate::engine::queue_reader_index(&backend, &epoch)
            .await
            .unwrap();
        assert!(owner);
        std::fs::write(notes.path().join("note.md"), "# Second\nSecondrevision").unwrap();
        let (owner_again, second_source) = crate::engine::queue_reader_index(&backend, &epoch)
            .await
            .unwrap();
        assert!(!owner_again);
        assert_eq!(source, second_source);
        let job: Option<mdreader_core::IndexJob> = backend
            .typed(Request::JobStatus {
                source_id: source.clone(),
            })
            .unwrap();
        assert_eq!(job.unwrap().generation, 2);
        let hits: Vec<mdreader_core::Evidence> = backend
            .typed(Request::Search {
                query: "Secondrevision".into(),
                sources: vec![source],
                limit: 10,
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
    }
}
