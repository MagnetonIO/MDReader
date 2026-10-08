#[cfg_attr(not(feature = "local-embed"), allow(dead_code))]
#[path = "model.rs"]
mod model;
use super::SearchOutcome;
use super::{execute, protocol_error, Request, SearchMode};
use mdreader_core::{ReaderError, ReaderStore, Result};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex, MutexGuard};

use model::{ModelActor, ModelError, ModelState, Ticket};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;

pub(super) struct Runtime {
    store: Arc<Mutex<ReaderStore>>,
    queries: Arc<Mutex<ReaderStore>>,
    model: Option<ModelActor>,
    wake: mpsc::SyncSender<()>,
    directory: PathBuf,
    catalog: super::catalog::Coordinator,
    #[cfg(feature = "sync-native")]
    accounts: super::accounts::Result<super::accounts::Coordinator>,
    reviews: std::result::Result<super::reviews::Coordinator, super::reviews::ReviewError>,
    providers:
        std::result::Result<Option<super::provider::Coordinator>, mdreader_provider::ProviderError>,
}
impl Runtime {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(
        store: Arc<Mutex<ReaderStore>>,
        cache: PathBuf,
        options: super::ServiceOptions,
    ) -> Result<Self> {
        Self::new_with_provider(store, cache, options, None)
    }
    pub fn new_with_provider(
        store: Arc<Mutex<ReaderStore>>,
        cache: PathBuf,
        options: super::ServiceOptions,
        factory: Option<Arc<dyn super::provider::ProviderFactory>>,
    ) -> Result<Self> {
        Self::new_with_adapters(store, cache, options, super::NativeAdapters { factory, ..super::NativeAdapters::default() })
    }
    pub(super) fn new_with_adapters(
        store: Arc<Mutex<ReaderStore>>,
        cache: PathBuf,
        options: super::ServiceOptions,
        adapters: super::NativeAdapters,
    ) -> Result<Self> {
        // Offline mode creates neither an actor thread nor a model/download.
        #[cfg(feature = "local-embed")]
        let model = (!options.disable_model).then(|| ModelActor::new(cache.clone(), false));
        #[cfg(not(feature = "local-embed"))]
        let model: Option<ModelActor> = {
            let _ = options;
            None
        };
        let owner = store.clone();
        let mut runtime = Self::with_model(store, cache, model)?;
        #[cfg(feature = "sync-native")]
        if let Some((setup, roots)) = adapters.accounts {
            runtime.accounts = setup.and_then(|setup| super::accounts::Coordinator::new(owner.clone(), setup, roots));
        }
        if let Some(factory) = adapters.factory {
            // Provider cache/host failure must not take local reading, capture,
            // or indexing offline. Retain the typed error for provider clients.
            runtime.providers = (|| {
                let library =
                    super::provider::Owner::with_connection(&*owner, super::provider::library_id)?;
                let projection = Arc::new(super::provider::CoreProjection::open(
                    &runtime.directory.join("provider-cache"),
                    library,
                )?);
                super::provider::Coordinator::new(owner, projection, factory).map(Some)
            })();
        }
        Ok(runtime)
    }
    fn with_model(
        store: Arc<Mutex<ReaderStore>>,
        cache: PathBuf,
        model: Option<ModelActor>,
    ) -> Result<Self> {
        {
            let mut owner = store
                .lock()
                .map_err(|_| protocol_error("reader worker failed"))?;
            crate::workload::initialize(&mut owner)?;
            if let Some(model) = &model {
                model.set_background_policy(crate::workload::get(&mut owner)?);
            }
        }
        let (wake, receiver) = mpsc::sync_channel(1);
        let worker_store = Arc::clone(&store);
        let worker_model = model.clone();
        let directory = cache
            .parent()
            .expect("model cache is inside service directory")
            .to_path_buf();
        let queries = Arc::new(Mutex::new(ReaderStore::open_readonly(&directory)?));
        let worker_directory = directory.clone();
        std::thread::spawn(move || {
            if let Some(model) = &worker_model {
                // Persisted vectors need query inference after a service restart.
                let needs_model = worker_store.lock().ok().is_some_and(|store| {
                    store
                        .list_sources()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|source| source.enabled)
                        .any(|source| {
                            store
                                .index_status(&[source.id])
                                .is_ok_and(|status| status.embedded > 0)
                        })
                });
                if needs_model {
                    model.ensure_loaded();
                }
            }
            let mut scheduler = Scheduler::default();
            loop {
                // Admission does not wait for initialization or inference.
                if let Ok(mut store) = worker_store.lock() {
                    let _ = super::capture::drain(&mut store, &worker_directory);
                }
                let activity = scheduler.tick(&worker_store, worker_model.as_ref());
                // Continue fair progress immediately. A zero timeout still
                // consumes wakes and detects shutdown; capture admission runs
                // again before the next bounded turn. Sleep only with no
                // runnable progress, so blocked inference never causes a spin.
                let delay = if activity == Activity::Waiting {
                    worker_model
                        .as_ref()
                        .and_then(ModelActor::background_wait)
                        .unwrap_or_else(|| activity.delay())
                } else {
                    activity.delay()
                };
                if matches!(
                    receiver.recv_timeout(delay),
                    Err(mpsc::RecvTimeoutError::Disconnected)
                ) {
                    break;
                }
            }
        });
        let reviews = super::reviews::Coordinator::new(store.clone(), &directory);
        Ok(Self {
            catalog: super::catalog::Coordinator::new(store.clone(), queries.clone()),
            reviews,
            store,
            queries,
            model,
            wake,
            directory,
            providers: Ok(None),
            #[cfg(feature = "sync-native")]
            accounts: Err(super::accounts::AccountError::Unconfigured),
        })
    }
    fn store(&self) -> Result<MutexGuard<'_, ReaderStore>> {
        self.store
            .lock()
            .map_err(|_| protocol_error("reader worker failed"))
    }
    fn queries(&self) -> Result<MutexGuard<'_, ReaderStore>> {
        self.queries
            .lock()
            .map_err(|_| protocol_error("reader query worker failed"))
    }
    pub fn execute(&self, request: Request) -> Result<Value> {
        #[cfg(feature = "sync-native")]
        if let Request::Account { request } = request {
            let result = self.accounts.as_ref().map_err(|error| *error).and_then(|owner| owner.execute(request));
            return Ok(match result {
                Ok(reply) => serde_json::json!({"result":reply,"error":null}),
                Err(error) => serde_json::json!({"result":null,"error":error}),
            });
        }
        if let Request::Catalog { request } = request {
            let result = self.catalog.execute(request);
            return Ok(match result {
                Ok(reply) => serde_json::json!({"result":reply,"error":null}),
                Err(error) => serde_json::json!({"result":null,"error":error}),
            });
        }
        if let Request::Reviews { request } = request {
            let result = self
                .reviews
                .as_ref()
                .map_err(|error| *error)
                .and_then(|reviews| reviews.execute(request));
            return Ok(match result {
                Ok(reply) => serde_json::json!({"result":reply,"error":null}),
                Err(error) => serde_json::json!({"result":null,"error":error}),
            });
        }
        if let Request::Provider { request } = request {
            let result = self
                .providers
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|coordinator| {
                    coordinator
                        .as_ref()
                        .ok_or(mdreader_provider::ProviderError::Unsupported)
                })
                .and_then(|coordinator| coordinator.execute(request));
            let value = match result {
                Ok(value) => serde_json::json!({"result":value,"error":null}),
                Err(error) => serde_json::json!({"result":null,"error":error}),
            };
            let _ = self.wake.try_send(());
            return Ok(value);
        }
        if let Request::SetEmbeddingPolicy { policy } = request {
            crate::workload::set(&mut *self.store()?, policy)?;
            if let Some(model) = &self.model {
                model.set_background_policy(policy);
            }
            let _ = self.wake.try_send(());
            return serde_json::to_value(policy)
                .map_err(|_| protocol_error("invalid embedding policy"));
        }
        if let Request::ConfigureCapture { policy } = request {
            let path = super::capture::policy_path(&self.directory, &policy.project_id);
            super::capture::configure(&mut *self.store()?, &self.directory, policy)?;
            let _ = self.wake.try_send(());
            return Ok(serde_json::json!({"configured":true,"policyPath":path}));
        }
        if let Request::CaptureBackfill { project_id, request } = request {
            let mutation = request.is_mutation();
            let reply = super::capture::backfill::execute(
                &mut *self.store()?,
                &self.directory,
                &project_id,
                request,
            )?;
            if mutation {
                let _ = self.wake.try_send(());
            }
            return serde_json::to_value(reply)
                .map_err(|_| protocol_error("invalid backfill reply"));
        }
        if matches!(request, Request::CaptureStatus) {
            return serde_json::to_value(super::capture::status(
                &*self.queries()?,
                &self.directory,
            )?)
            .map_err(|_| protocol_error("invalid capture status"));
        }
        if let Request::CapturePolicy { project_id } = request {
            return serde_json::to_value(super::integrations::capture_policy(
                &*self.queries()?,
                &project_id,
            )?)
            .map_err(|_| protocol_error("invalid capture policy"));
        }
        if let Request::ListIntegrations { project_id } = request {
            return super::integrations::list(&mut *self.queries()?, &project_id);
        }
        if matches!(
            request,
            Request::PrepareIntegration { .. }
                | Request::ApplyIntegration { .. }
                | Request::UninstallIntegration { .. }
        ) {
            return super::integrations::execute(&mut *self.store()?, &self.directory, request);
        }
        if let Request::SearchRanked {
            query,
            sources,
            limit,
            mode,
        } = request
        {
            let mut fallback = execute(
                &mut *self.queries()?,
                Request::SearchRanked {
                    query: query.clone(),
                    sources: sources.clone(),
                    limit,
                    mode,
                },
            )?;
            if !matches!(mode, SearchMode::Hybrid | SearchMode::Semantic) {
                return Ok(fallback);
            }
            let text = ReaderStore::semantic_query_text(&query)?;
            if text.is_empty() {
                return Ok(fallback);
            }
            if let Some(model) = &self.model {
                if let Ok(vector) = model.query(text, model::QUERY_DEADLINE) {
                    let store = self.queries()?;
                    let hits = if matches!(mode, SearchMode::Semantic) {
                        store.semantic_search_query(&query, &vector, &sources, limit)?
                    } else {
                        store.hybrid_search(&query, &vector, &sources, limit)?
                    };
                    return serde_json::to_value(SearchOutcome {
                        hits,
                        semantic_unavailable: None,
                    })
                    .map_err(|_| protocol_error("invalid search result"));
                }
                fallback["semantic_unavailable"] =
                    Value::String(model_diagnostic(model.ensure_loaded()).into());
            } else {
                fallback["semantic_unavailable"] = Value::String("Local embedding model is disabled or unavailable; lexical search remains available".into());
            }
            Ok(fallback)
        } else {
            let queue = matches!(request, Request::StartIndex { .. });
            let read_only = matches!(
                request,
                Request::Ping
                    | Request::EmbeddingPolicy
                    | Request::FindSource { .. }
                    | Request::ListSources
                    | Request::Status { .. }
                    | Request::Search { .. }
                    | Request::Read { .. }
                    | Request::Resolve { .. }
                    | Request::Related { .. }
                    | Request::RelatedRanked { .. }
                    | Request::Bookmark { .. }
                    | Request::JobStatus { .. }
                    | Request::ListDocuments { .. }
            );
            let result = if read_only {
                execute(&mut *self.queries()?, request)?
            } else {
                execute(&mut *self.store()?, request)?
            };
            if queue {
                let _ = self.wake.try_send(());
            }
            Ok(result)
        }
    }
}
fn phase(store: &Mutex<ReaderStore>, source: &str, generation: u64, status: &str) -> Result<()> {
    if store
        .lock()
        .map_err(|_| protocol_error("reader worker failed"))?
        .record_index_job_if_current(source, generation, status, None)?
    {
        Ok(())
    } else {
        Err(ReaderError::Cancelled)
    }
}
fn model_diagnostic(state: ModelState) -> &'static str {
    match state {
        ModelState::RestartRequired => "Local embedding model exceeded its deadline; restart the reader service to enable semantic search",
        ModelState::Loading => "Local embedding model is loading; lexical search remains available",
        ModelState::Busy => "Local embedding model is busy; lexical search remains available",
        _ => "Local embedding model is unavailable; lexical search remains available",
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Activity {
    Idle,
    Waiting,
    Progress,
}
impl Activity {
    fn delay(self) -> Duration {
        match self {
            Self::Idle => Duration::from_secs(1),
            Self::Waiting => Duration::from_millis(20),
            Self::Progress => Duration::ZERO,
        }
    }
}
enum Step {
    Waiting,
    Progress,
    Admitted,
}
#[derive(Default)]
struct Scheduler {
    jobs: HashMap<String, Work>,
    last_admitted: Option<String>,
}
struct Work {
    generation: u64,
    documents: Option<VecDeque<String>>,
    document: Option<EmbeddingDocument>,
    recovered: bool,
}
struct EmbeddingDocument {
    path: String,
    revision: String,
    texts: VecDeque<String>,
    vectors: Vec<Vec<f32>>,
    ticket: Option<Ticket>,
    batch_size: usize,
}
impl Scheduler {
    fn tick(&mut self, store: &Mutex<ReaderStore>, model: Option<&ModelActor>) -> Activity {
        let pending = store
            .lock()
            .ok()
            .and_then(|store| store.pending_index_jobs().ok())
            .unwrap_or_default();
        self.jobs.retain(|id, _| pending.contains(id));
        let mut pending = pending;
        if let Some(last) = &self.last_admitted {
            if let Some(position) = pending.iter().position(|id| id == last) {
                let length = pending.len();
                pending.rotate_left((position + 1) % length);
            }
        }
        let mut progressed = false;
        for source in &pending {
            let record = store
                .lock()
                .ok()
                .and_then(|store| store.index_job_record(source).ok().flatten());
            let Some(record) = record else {
                continue;
            };
            let work = self.jobs.entry(source.clone()).or_insert_with(|| Work {
                generation: record.generation,
                documents: None,
                document: None,
                recovered: matches!(record.phase.as_str(), "loading" | "embedding"),
            });
            if work.generation != record.generation {
                *work = Work {
                    generation: record.generation,
                    documents: None,
                    document: None,
                    recovered: false,
                };
            }
            match step(store, model, source, work) {
                Ok(Step::Admitted) => {
                    self.last_admitted = Some(source.clone());
                    progressed = true;
                }
                Ok(Step::Progress) => progressed = true,
                Ok(Step::Waiting) => {}
                Err(error) => {
                    if !matches!(error, ReaderError::Cancelled) {
                        let (phase, diagnostic) = if matches!(error, ReaderError::ModelUnavailable)
                        {
                            ("model-unavailable", model.map_or("Local embedding model is disabled or unavailable; lexical search remains available", |model| model_diagnostic(model.ensure_loaded())))
                        } else {
                            ("error", "Indexing failed; check source access and retry")
                        };
                        if let Ok(store) = store.lock() {
                            let _ = store.record_index_job_if_current(
                                source,
                                work.generation,
                                phase,
                                Some(diagnostic),
                            );
                        }
                    }
                    self.jobs.remove(source);
                    progressed = true;
                }
            }
        }
        if pending.is_empty() {
            Activity::Idle
        } else if progressed {
            Activity::Progress
        } else {
            Activity::Waiting
        }
    }
}
fn current(store: &ReaderStore, source: &str, generation: u64) -> Result<()> {
    if store
        .index_job(source)?
        .is_some_and(|job| job.generation == generation)
    {
        Ok(())
    } else {
        Err(ReaderError::Cancelled)
    }
}
/// A scheduler turn performs at most one lexical pass, inference admission,
/// completion, or document install. Inference itself runs only on the actor.
fn step(
    store: &Mutex<ReaderStore>,
    model: Option<&ModelActor>,
    source: &str,
    work: &mut Work,
) -> Result<Step> {
    {
        let store = store.lock().map_err(|_| ReaderError::Unavailable)?;
        current(&store, source, work.generation)?;
    }
    if work.documents.is_none() {
        let mut store = store.lock().map_err(|_| ReaderError::Unavailable)?;
        if !work.recovered {
            store.record_index_job_if_current(source, work.generation, "indexing", None)?;
            store.index_source(source)?;
        }
        current(&store, source, work.generation)?;
        work.documents = Some(store.pending_embedding_documents(source)?.into());
        // Persist that the atomic lexical transaction completed before any model
        // wait. Crash recovery resumes missing vectors without rescanning source.
        store.record_index_job_if_current(source, work.generation, "loading", None)?;
        return Ok(Step::Progress);
    }
    if work.document.is_none() {
        let Some(path) = work.documents.as_mut().and_then(VecDeque::pop_front) else {
            phase(store, source, work.generation, "done")?;
            return Ok(Step::Progress);
        };
        let store = store.lock().map_err(|_| ReaderError::Unavailable)?;
        let (revision, texts) = store.embedding_inputs(source, &path)?;
        work.document = Some(EmbeddingDocument {
            path,
            revision,
            texts: texts.into(),
            vectors: Vec::new(),
            ticket: None,
            batch_size: 0,
        });
    }
    let document = work.document.as_mut().expect("prepared above");
    if let Some(ticket) = &mut document.ticket {
        let Some(result) = model.ok_or(ReaderError::ModelUnavailable)?.poll(ticket) else {
            return Ok(Step::Waiting);
        };
        let vectors = result.map_err(|_| ReaderError::ModelUnavailable)?;
        if vectors.len() != document.batch_size {
            return Err(ReaderError::ModelUnavailable);
        }
        document.vectors.extend(vectors);
        document.ticket = None;
        return Ok(Step::Progress);
    }
    if document.texts.is_empty() {
        let mut store = store.lock().map_err(|_| ReaderError::Unavailable)?;
        current(&store, source, work.generation)?;
        // Indexed revision alone is insufficient if the source changed while
        // inference was running but its next indexing generation has not begun.
        store.read_document(source, &document.path, Some(&document.revision))?;
        store.install_embeddings(
            source,
            &document.path,
            &document.revision,
            &document.vectors,
        )?;
        work.document = None;
        return Ok(Step::Progress);
    }
    let model = model.ok_or(ReaderError::ModelUnavailable)?;
    if !model.background_ready() {
        return Ok(Step::Waiting);
    }
    match model.ensure_loaded() {
        ModelState::Ready => {}
        ModelState::Loading | ModelState::Busy => return Ok(Step::Waiting),
        _ => return Err(ReaderError::ModelUnavailable),
    }
    let mut texts = Vec::new();
    let mut bytes = 0;
    for text in &document.texts {
        if texts.len() == model.background_batch_limit()
            || bytes + text.len() > model::MAX_BATCH_BYTES
        {
            break;
        }
        bytes += text.len();
        texts.push(text.clone());
    }
    if texts.is_empty() {
        return Err(ReaderError::ModelUnavailable);
    }
    document.batch_size = texts.len();
    phase(store, source, work.generation, "embedding")?;
    match model.start_background(texts, model::BATCH_DEADLINE) {
        Ok(ticket) => {
            document.texts.drain(..document.batch_size);
            document.ticket = Some(ticket);
            Ok(Step::Admitted)
        }
        // A query may have won the actor admission race. Keep every input and
        // retry this source on a later turn instead of failing its durable job.
        Err(ModelError::Busy) => Ok(Step::Waiting),
        Err(_) => Err(ReaderError::ModelUnavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct SyntheticEngine {
        calls: Arc<Mutex<Vec<Vec<String>>>>,
        first: Option<mpsc::Receiver<()>>,
        entered: mpsc::Sender<()>,
    }
    impl model::Engine for SyntheticEngine {
        fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            self.calls.lock().unwrap().push(texts.to_vec());
            if let Some(release) = self.first.take() {
                self.entered.send(()).unwrap();
                release.recv().unwrap();
            }
            Ok(texts.iter().map(|_| vec![1.0; 384]).collect())
        }
    }
    type RecordedBatches = Arc<Mutex<Vec<Vec<String>>>>;
    type FakeActor = (
        ModelActor,
        mpsc::Sender<()>,
        mpsc::Receiver<()>,
        RecordedBatches,
    );
    fn fake_model() -> FakeActor {
        let (release, blocked) = mpsc::channel();
        let (entered, observed) = mpsc::channel();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recording = calls.clone();
        let model = ModelActor::with_factory(Duration::from_secs(1), move || {
            Ok(Box::new(SyntheticEngine {
                calls: recording,
                first: Some(blocked),
                entered,
            }))
        });
        (model, release, observed, calls)
    }
    fn add_source(store: &mut ReaderStore, directory: &std::path::Path, name: &str) -> String {
        store
            .add_local_source(
                name,
                mdreader_core::SourceRoot::capture(directory.canonicalize().unwrap()).unwrap(),
            )
            .unwrap()
            .id
    }
    fn drive_until(mut condition: impl FnMut() -> bool) {
        let until = std::time::Instant::now() + Duration::from_secs(3);
        while !condition() {
            assert!(
                std::time::Instant::now() < until,
                "synthetic worker did not make bounded progress"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    #[cfg(unix)]
    fn hung_large_source_cannot_starve_fresh_capture_or_other_source_lexical_work() {
        use mdreader_capture::{Adapter, CaptureOrigin, Limits, ProjectPolicy, Spool};
        let data = tempfile::tempdir().unwrap();
        let notes = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let huge = (0..100)
            .map(|index| format!("## Section {index}\n\nHugework {index}.\n\n"))
            .collect::<String>();
        std::fs::write(notes.path().join("large.md"), format!("# Huge\n\n{huge}")).unwrap();
        let mut store = ReaderStore::open(data.path()).unwrap();
        let source = add_source(&mut store, notes.path(), "Huge");
        store.queue_index(&source).unwrap();
        let root = project.path().canonicalize().unwrap();
        let policy = ProjectPolicy {
            enabled: true,
            project_id: "scheduler-test".into(),
            project_root: root.clone(),
            repository_root: root.clone(),
            worktree_roots: vec![root.clone()],
            capture_subagents: false,
            excluded_paths: vec![],
            excluded_sessions: vec![],
            selected_backfill_sessions: vec![],
        };
        super::super::capture::configure(&mut store, data.path(), policy.clone()).unwrap();
        let store = Mutex::new(store);
        let (model, release, entered, calls) = fake_model();
        let mut scheduler = Scheduler::default();
        drive_until(|| {
            scheduler.tick(&store, Some(&model));
            entered.try_recv().is_ok()
        });
        assert_eq!(scheduler.tick(&store, Some(&model)), Activity::Waiting);
        assert!(
            Activity::Waiting.delay() >= Duration::from_millis(20),
            "all-waiting jobs must back off instead of spinning"
        );
        // A file arriving after this generation's lexical pass must not be
        // silently indexed again once per embedding batch.
        std::fs::write(notes.path().join("late.md"), "# Late\nNeverrescanned").unwrap();
        let spool = Spool::open(data.path().join("captures"), Limits::default()).unwrap();
        let event=serde_json::to_vec(&serde_json::json!({"hook_event_name":"Stop","session_id":"scheduler-session","cwd":root,"last_assistant_message":"# Plan\nFreshcapturevisible"})).unwrap();
        spool
            .enqueue(Adapter::ClaudeCode, &event, &policy, CaptureOrigin::Live)
            .unwrap();
        let begin = std::time::Instant::now();
        let admitted =
            super::super::capture::drain(&mut store.lock().unwrap(), data.path()).unwrap();
        assert_eq!(admitted.admitted, 1);
        for _ in 0..5 {
            scheduler.tick(&store, Some(&model));
        }
        let capture = store
            .lock()
            .unwrap()
            .list_sources()
            .unwrap()
            .into_iter()
            .find(|source| source.name == "Agent captures")
            .unwrap()
            .id;
        assert!(!store
            .lock()
            .unwrap()
            .search("Freshcapturevisible", std::slice::from_ref(&capture), 10)
            .unwrap()
            .is_empty());
        assert!(spool.pending(10).unwrap().is_empty());
        assert!(
            begin.elapsed() < Duration::from_millis(500),
            "capture/lexical work waited for blocked inference"
        );
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "busy admission must never queue another inference"
        );
        release.send(()).unwrap();
        drive_until(|| {
            scheduler.tick(&store, Some(&model));
            store
                .lock()
                .unwrap()
                .pending_index_jobs()
                .unwrap()
                .is_empty()
        });
        let batches = calls.lock().unwrap();
        assert!(batches.len() >= 5);
        assert!(batches
            .iter()
            .all(|batch| batch.len() <= model::MAX_BATCH_TEXTS));
        let capture_position = batches
            .iter()
            .position(|batch| {
                batch
                    .iter()
                    .any(|text| text.contains("Freshcapturevisible"))
            })
            .unwrap();
        assert!(
            capture_position <= 2,
            "fresh capture must get an embedding turn before the huge source drains"
        );
        assert!(store
            .lock()
            .unwrap()
            .search("Neverrescanned", &[source], 10)
            .unwrap()
            .is_empty());
    }
    #[test]
    fn nonblocking_progress_does_not_pay_a_poll_sleep_per_document() {
        let data = tempfile::tempdir().unwrap();
        let notes = tempfile::tempdir().unwrap();
        for index in 0..20 {
            std::fs::write(
                notes.path().join(format!("note-{index}.md")),
                format!("# Note {index}\nSynthetic passage {index}"),
            )
            .unwrap();
        }
        let mut owner = ReaderStore::open(data.path()).unwrap();
        let source = add_source(&mut owner, notes.path(), "Scheduling");
        owner.queue_index(&source).unwrap();
        let writer = Arc::new(Mutex::new(owner));
        let (model, release, _entered, _calls) = fake_model();
        release.send(()).unwrap();
        let begin = std::time::Instant::now();
        let runtime =
            Runtime::with_model(writer.clone(), data.path().join("models"), Some(model)).unwrap();
        drive_until(|| {
            writer
                .lock()
                .unwrap()
                .index_job(&source)
                .unwrap()
                .unwrap()
                .phase
                == "done"
        });
        let elapsed = begin.elapsed();
        assert_eq!(
            writer
                .lock()
                .unwrap()
                .index_status(&[source])
                .unwrap()
                .embedded,
            20
        );
        drop(runtime);
        assert!(elapsed < Duration::from_millis(900), "20 immediately available synthetic operations took {elapsed:?}; progress turns must not sleep");
    }
    #[test]
    fn pending_vectors_never_install_after_edit_requeue_or_revoke() {
        for action in ["edit", "requeue", "revoke"] {
            let data = tempfile::tempdir().unwrap();
            let notes = tempfile::tempdir().unwrap();
            std::fs::write(notes.path().join("note.md"), "# Note\nOriginalrevision").unwrap();
            let mut owner = ReaderStore::open(data.path()).unwrap();
            let source = add_source(&mut owner, notes.path(), "Notes");
            owner.queue_index(&source).unwrap();
            let generation = owner.index_job(&source).unwrap().unwrap().generation;
            let store = Mutex::new(owner);
            let (model, release, entered, _) = fake_model();
            let mut work = Work {
                generation,
                documents: None,
                document: None,
                recovered: false,
            };
            drive_until(|| {
                step(&store, Some(&model), &source, &mut work).unwrap();
                entered.try_recv().is_ok()
            });
            match action {
                "edit" => std::fs::write(notes.path().join("note.md"), "# Changed\nFreshrevision")
                    .unwrap(),
                "requeue" => store.lock().unwrap().queue_index(&source).unwrap(),
                "revoke" => store
                    .lock()
                    .unwrap()
                    .set_source_enabled(&source, false)
                    .unwrap(),
                _ => unreachable!(),
            }
            release.send(()).unwrap();
            let mut rejected = false;
            drive_until(|| match step(&store, Some(&model), &source, &mut work) {
                Err(_) => {
                    rejected = true;
                    true
                }
                Ok(_) => false,
            });
            assert!(rejected);
            if action == "revoke" {
                store
                    .lock()
                    .unwrap()
                    .set_source_enabled(&source, true)
                    .unwrap();
            }
            assert_eq!(
                store
                    .lock()
                    .unwrap()
                    .index_status(&[source])
                    .unwrap()
                    .embedded,
                0,
                "{action} installed stale vectors"
            );
        }
    }
    #[test]
    fn crash_recovery_resumes_vectors_without_repeating_committed_lexical_index() {
        let data = tempfile::tempdir().unwrap();
        let notes = tempfile::tempdir().unwrap();
        std::fs::write(notes.path().join("first.md"), "# First\nAlreadyembedded").unwrap();
        std::fs::write(notes.path().join("second.md"), "# Second\nPendingvectors").unwrap();
        let source;
        {
            let mut owner = ReaderStore::open(data.path()).unwrap();
            source = add_source(&mut owner, notes.path(), "Notes");
            owner.queue_index(&source).unwrap();
            owner.index_source(&source).unwrap();
            let (revision, texts) = owner.embedding_inputs(&source, "first.md").unwrap();
            owner
                .install_embeddings(
                    &source,
                    "first.md",
                    &revision,
                    &vec![vec![1.0; 384]; texts.len()],
                )
                .unwrap();
            owner.record_index_job(&source, "embedding", None).unwrap();
        }
        std::fs::write(notes.path().join("late.md"), "# Late\nNotthisgeneration").unwrap();
        let store = Mutex::new(ReaderStore::open(data.path()).unwrap());
        let (model, release, entered, calls) = fake_model();
        let mut scheduler = Scheduler::default();
        drive_until(|| {
            scheduler.tick(&store, Some(&model));
            entered.try_recv().is_ok()
        });
        release.send(()).unwrap();
        drive_until(|| {
            scheduler.tick(&store, Some(&model));
            store
                .lock()
                .unwrap()
                .pending_index_jobs()
                .unwrap()
                .is_empty()
        });
        let batches = calls.lock().unwrap();
        assert_eq!(batches.len(), 1);
        assert!(batches[0][0].contains("Pendingvectors"));
        let owner = store.lock().unwrap();
        assert_eq!(owner.index_job(&source).unwrap().unwrap().phase, "done");
        assert!(owner
            .search("Notthisgeneration", &[source], 10)
            .unwrap()
            .is_empty());
    }
    #[test]
    fn committed_search_remains_available_while_writer_is_busy() {
        let directory = tempfile::tempdir().unwrap();
        let notes = tempfile::tempdir().unwrap();
        std::fs::write(
            notes.path().join("note.md"),
            "# Note\nSnapshotresponsiveness",
        )
        .unwrap();
        let mut store = ReaderStore::open(directory.path()).unwrap();
        let root =
            mdreader_core::SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap();
        let source = store.add_local_source("Notes", root).unwrap();
        store.index_source(&source.id).unwrap();
        let writer = Arc::new(Mutex::new(store));
        let runtime = Arc::new(
            Runtime::new(
                writer.clone(),
                directory.path().join("models"),
                super::super::ServiceOptions {
                    disable_model: true,
                },
            )
            .unwrap(),
        );
        assert!(
            runtime.model.is_none(),
            "offline mode must not construct a model actor"
        );
        let guard = writer.lock().unwrap();
        let (sender, receiver) = mpsc::channel();
        let query_runtime = runtime.clone();
        let thread = std::thread::spawn(move || {
            let result = query_runtime.execute(Request::Search {
                query: "Snapshotresponsiveness".into(),
                sources: vec![source.id],
                limit: 10,
            });
            let _ = sender.send(result);
        });
        let result = receiver.recv_timeout(std::time::Duration::from_millis(300));
        drop(guard);
        thread.join().unwrap();
        assert!(
            result.is_ok(),
            "queries must not wait on indexing's writer mutex"
        );
        assert_eq!(result.unwrap().unwrap()[0]["path"], "note.md");
    }
}

#[cfg(all(test, feature = "sync-native"))]
#[path = "accounts_runtime_tests.rs"]
mod accounts_tests;
