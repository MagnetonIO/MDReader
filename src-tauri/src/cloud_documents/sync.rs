//! Bounded background synchronization for provider-owned Markdown metadata.
//!
//! A sync owner never holds the SQLite store mutex while awaiting a provider.
//! Its epoch and cancellation token fence every durable publication so a
//! paused, retired, or superseded owner cannot publish late results.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant, SystemTime},
};

#[cfg(test)]
use std::future::Future;

use catdb_connectors::{CancellationToken, SourceId};
use tokio::sync::Notify;

use super::{
    store::{
        CloudDocumentStore, CloudDocumentStoreError, CloudDocumentVersionPins,
        PersistedCloudWriteRepair, RepairContinuation,
    },
    CloudDocumentError, CloudIoContext, KnownMarkdownMetadata, MarkdownChangeEvent,
    MarkdownChangePage, MarkdownDocumentSource, MarkdownMetadata, MarkdownMetadataPage,
    ProviderCursor,
};

const SYNC_DEADLINE: Duration = Duration::from_secs(10 * 60);
const SYNC_PAGE_ROWS: usize = 256;
const SYNC_PAGE_BYTES: usize = 1024 * 1024;
const SYNC_MAX_PAGES: usize = 256;
const SYNC_MAX_ROWS: usize = SYNC_PAGE_ROWS * SYNC_MAX_PAGES;
const SYNC_MAX_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_RETRY_AFTER_MS: u64 = 1_000;
const STARTUP_REPAIR_PAGE_ROWS: usize = 64;
const STARTUP_REPAIR_MAX_PAGES: usize = 16;
const STARTUP_REPAIR_EXECUTOR_DEADLINE: Duration = Duration::from_secs(5);

/// Closed state of one cloud-source metadata synchronizer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CloudSyncPhase {
    /// No snapshot has been published for this owner yet.
    NeverSynced,
    /// A complete replacement snapshot is being staged.
    Initializing { generation: uuid::Uuid },
    /// The latest durable snapshot is available to foreground readers.
    Published { observed_at_ms: i64 },
    /// A durable cursor is being advanced through provider changes.
    Refreshing { observed_at_ms: i64 },
    /// Provider or local failure retained verbatim until explicit refresh.
    Backoff {
        error: CloudDocumentError,
        retry_at_ms: i64,
    },
    /// Provider calls are cancelled while the last publication remains intact.
    Paused,
    /// This exact source owner is permanently retired.
    Retired,
}

/// Typed in-process status for one source's synchronization owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CloudSyncStatus {
    phase: CloudSyncPhase,
    startup_repair_error: Option<CloudDocumentError>,
}

impl CloudSyncStatus {
    /// Returns the owner's current closed phase.
    pub(crate) fn phase(&self) -> &CloudSyncPhase {
        &self.phase
    }

    /// Returns an unresolved startup local-publication outcome, if any.
    pub(crate) fn startup_repair_error(&self) -> Option<&CloudDocumentError> {
        self.startup_repair_error.as_ref()
    }
}

/// Errors from coordinator ownership or durable metadata operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CloudSyncError {
    /// SQLite could not safely complete the requested source-scoped effect.
    Store(CloudDocumentStoreError),
    /// The requested source has no retained coordinator owner.
    UnknownSource,
    /// A retired owner cannot be reactivated under the same source identity.
    Retired,
    /// Coordinator state was poisoned by an unexpected task failure.
    StateUnavailable,
    /// The runtime could not complete the blocking SQLite hand-off.
    RuntimeUnavailable,
    /// This coordinator accepts only source-owned semantic pins.
    MissingVersionPins,
}

/// Application-owned local-publication executor for a provider-accepted
/// receipt. It receives no provider handle, so restart repair cannot replay a
/// remote mutation.
#[async_trait::async_trait]
pub(crate) trait CloudWriteRepairExecutor: Send + Sync {
    /// Publishes the derived local state represented by one durable receipt.
    async fn resume_local_publication(
        &self,
        repair: &PersistedCloudWriteRepair,
    ) -> Result<(), CloudDocumentError>;
}

impl From<CloudDocumentStoreError> for CloudSyncError {
    fn from(error: CloudDocumentStoreError) -> Self {
        Self::Store(error)
    }
}

/// Coordinates one bounded metadata-sync owner per source identifier.
#[derive(Clone)]
pub(crate) struct CloudSyncCoordinator {
    store: Arc<CloudDocumentStore>,
    default_pins: Option<CloudDocumentVersionPins>,
    owners: Arc<Mutex<HashMap<String, SyncOwner>>>,
    durable_effects: Arc<tokio::sync::Mutex<()>>,
    startup_repair_errors: Arc<Mutex<HashMap<String, CloudDocumentError>>>,
    startup_repair_owners: Arc<Mutex<HashMap<String, StartupRepairOwner>>>,
    next_startup_repair_epoch: Arc<AtomicU64>,
    #[cfg(test)]
    effect_gate: Arc<Mutex<Option<DurableEffectGate>>>,
}

struct SyncOwner {
    source: Arc<dyn MarkdownDocumentSource>,
    pins: CloudDocumentVersionPins,
    epoch: u64,
    cancellation: CancellationToken,
    phase: CloudSyncPhase,
    running: bool,
    idle: Arc<Notify>,
    published: Arc<Notify>,
}

#[derive(Clone)]
struct StartupRepairOwner {
    epoch: u64,
    cancellation: CancellationToken,
}

struct SyncOperation<'a> {
    source: &'a Arc<dyn MarkdownDocumentSource>,
    source_id: &'a SourceId,
    epoch: u64,
    cancellation: &'a CancellationToken,
    pins: &'a CloudDocumentVersionPins,
    query_id: uuid::Uuid,
    deadline: Instant,
}

#[cfg(test)]
pub(crate) struct DurableEffectBarrier {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

#[cfg(test)]
impl DurableEffectBarrier {
    pub(crate) fn entered(&self) -> impl Future<Output = ()> + '_ {
        self.entered.notified()
    }

    pub(crate) fn release(&self) {
        self.release.notify_waiters();
    }
}

#[cfg(test)]
struct DurableEffectGate {
    ordinal: usize,
    barrier: DurableEffectBarrier,
}

impl CloudSyncCoordinator {
    /// Returns the local-only store read by foreground materialized sources.
    pub(crate) fn store(&self) -> Arc<CloudDocumentStore> {
        Arc::clone(&self.store)
    }

    /// Pins the currently admitted provider adapter and semantic source
    /// version for one foreground point read.
    pub(crate) fn point_read_owner(
        &self,
        source_id: &SourceId,
    ) -> Result<(Arc<dyn MarkdownDocumentSource>, CloudDocumentVersionPins), CloudDocumentError>
    {
        let owners = self
            .owners()
            .map_err(|_| CloudDocumentError::ProtocolFailed)?;
        let owner = owners
            .get(source_id.as_str())
            .ok_or(CloudDocumentError::SourceDisconnected)?;
        match owner.phase {
            CloudSyncPhase::Paused => return Err(CloudDocumentError::SourcePaused),
            CloudSyncPhase::Retired => return Err(CloudDocumentError::SourceDisconnected),
            _ if owner.cancellation.is_cancelled() => {
                return Err(CloudDocumentError::Cancelled);
            }
            _ => {}
        }
        Ok((Arc::clone(&owner.source), owner.pins.clone()))
    }

    /// Creates a coordinator whose durable rows carry the supplied immutable
    /// semantic pins.
    #[cfg(test)]
    pub(crate) fn new(store: Arc<CloudDocumentStore>, pins: CloudDocumentVersionPins) -> Self {
        Self {
            store,
            default_pins: Some(pins),
            owners: Arc::new(Mutex::new(HashMap::new())),
            durable_effects: Arc::new(tokio::sync::Mutex::new(())),
            startup_repair_errors: Arc::new(Mutex::new(HashMap::new())),
            startup_repair_owners: Arc::new(Mutex::new(HashMap::new())),
            next_startup_repair_epoch: Arc::new(AtomicU64::new(1)),
            #[cfg(test)]
            effect_gate: Arc::new(Mutex::new(None)),
        }
    }

    /// Creates a production coordinator that requires every activation to
    /// supply pins derived from the admitted source's catalog authority.
    pub(crate) fn with_source_owned_pins(store: Arc<CloudDocumentStore>) -> Self {
        Self {
            store,
            default_pins: None,
            owners: Arc::new(Mutex::new(HashMap::new())),
            durable_effects: Arc::new(tokio::sync::Mutex::new(())),
            startup_repair_errors: Arc::new(Mutex::new(HashMap::new())),
            startup_repair_owners: Arc::new(Mutex::new(HashMap::new())),
            next_startup_repair_epoch: Arc::new(AtomicU64::new(1)),
            #[cfg(test)]
            effect_gate: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(test)]
    pub(crate) fn block_durable_effect(&self, ordinal: usize) -> DurableEffectBarrier {
        assert!(ordinal > 0, "durable effects are one-indexed");
        let barrier = DurableEffectBarrier {
            entered: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        };
        *self.effect_gate.lock().expect("test effect gate lock") = Some(DurableEffectGate {
            ordinal,
            barrier: DurableEffectBarrier {
                entered: Arc::clone(&barrier.entered),
                release: Arc::clone(&barrier.release),
            },
        });
        barrier
    }

    /// Activates a source. A newer activation cancels the previous owner
    /// before installing a new epoch, so only the newest owner may publish.
    pub(crate) async fn activate(
        &self,
        source: Arc<dyn MarkdownDocumentSource>,
    ) -> Result<(), CloudSyncError> {
        let pins = self
            .default_pins
            .clone()
            .ok_or(CloudSyncError::MissingVersionPins)?;
        self.activate_with_pins(source, pins).await
    }

    /// Activates a source using its exact immutable semantic pins. Pins live
    /// on the source owner rather than the coordinator because one Library may
    /// contain distinct Drive and Dropbox source/mapping revisions.
    pub(crate) async fn activate_with_pins(
        &self,
        source: Arc<dyn MarkdownDocumentSource>,
        pins: CloudDocumentVersionPins,
    ) -> Result<(), CloudSyncError> {
        let _effects = self.durable_effects.lock().await;
        let source_id = source.source_id().clone();
        let (epoch, cancellation) = {
            let mut owners = self.owners()?;
            let key = source_id.as_str().to_owned();
            match owners.get_mut(&key) {
                Some(owner) => {
                    if matches!(owner.phase, CloudSyncPhase::Retired) {
                        return Err(CloudSyncError::Retired);
                    }
                    owner.cancellation.cancel();
                    owner.epoch = owner.epoch.wrapping_add(1);
                    owner.source = Arc::clone(&source);
                    owner.pins = pins;
                    owner.cancellation = CancellationToken::new();
                    owner.phase = CloudSyncPhase::NeverSynced;
                    owner.running = true;
                    (owner.epoch, owner.cancellation.clone())
                }
                None => {
                    let cancellation = CancellationToken::new();
                    owners.insert(
                        key,
                        SyncOwner {
                            source: Arc::clone(&source),
                            pins,
                            epoch: 1,
                            cancellation: cancellation.clone(),
                            phase: CloudSyncPhase::NeverSynced,
                            running: true,
                            idle: Arc::new(Notify::new()),
                            published: Arc::new(Notify::new()),
                        },
                    );
                    (1, cancellation)
                }
            }
        };
        self.spawn_owner(source, source_id, epoch, cancellation);
        Ok(())
    }

    /// Schedules one incremental pass for an idle owner. Repeated refresh
    /// requests while that source already has provider work are idempotent so
    /// no second owner can replay the same committed cursor.
    pub(crate) async fn refresh(&self, source_id: &SourceId) -> Result<(), CloudSyncError> {
        let _effects = self.durable_effects.lock().await;
        let scheduled = {
            let mut owners = self.owners()?;
            let owner = owners
                .get_mut(source_id.as_str())
                .ok_or(CloudSyncError::UnknownSource)?;
            if matches!(owner.phase, CloudSyncPhase::Retired) {
                return Err(CloudSyncError::Retired);
            }
            if matches!(owner.phase, CloudSyncPhase::Paused) || owner.running {
                None
            } else {
                owner.running = true;
                Some((
                    Arc::clone(&owner.source),
                    owner.epoch,
                    owner.cancellation.clone(),
                ))
            }
        };
        if let Some((source, epoch, cancellation)) = scheduled {
            self.spawn_owner(source, source_id.clone(), epoch, cancellation);
        }
        Ok(())
    }

    /// Cancels in-flight and future source calls without removing the last
    /// durable publication or its body cache.
    pub(crate) async fn pause(&self, source_id: &SourceId) -> Result<(), CloudSyncError> {
        let _effects = self.durable_effects.lock().await;
        self.cancel_startup_repair(source_id)?;
        let idle = {
            let mut owners = self.owners()?;
            let owner = owners
                .get_mut(source_id.as_str())
                .ok_or(CloudSyncError::UnknownSource)?;
            if matches!(owner.phase, CloudSyncPhase::Retired) {
                return Err(CloudSyncError::Retired);
            }
            owner.cancellation.cancel();
            owner.epoch = owner.epoch.wrapping_add(1);
            owner.phase = CloudSyncPhase::Paused;
            owner.running = false;
            Arc::clone(&owner.idle)
        };
        idle.notify_waiters();
        Ok(())
    }

    /// Cancels this owner before removing only its durable source state. The
    /// caller invokes it after the credential lifecycle commit has succeeded.
    pub(crate) async fn retire(&self, source_id: &SourceId) -> Result<(), CloudSyncError> {
        let _effects = self.durable_effects.lock().await;
        self.cancel_startup_repair(source_id)?;
        let idle = {
            let mut owners = self.owners()?;
            owners.get_mut(source_id.as_str()).map(|owner| {
                owner.cancellation.cancel();
                owner.epoch = owner.epoch.wrapping_add(1);
                owner.phase = CloudSyncPhase::Retired;
                owner.running = false;
                Arc::clone(&owner.idle)
            })
        };
        if let Some(idle) = idle {
            idle.notify_waiters();
        }
        let source_id = source_id.clone();
        let store = Arc::clone(&self.store);
        blocking_store(store, move |store| store.retire_source(&source_id)).await
    }

    /// Returns the current typed status, if this source has an owner.
    pub(crate) fn status(
        &self,
        source_id: &SourceId,
    ) -> Result<Option<CloudSyncStatus>, CloudSyncError> {
        let owner_phase = self
            .owners()?
            .get(source_id.as_str())
            .map(|owner| owner.phase.clone());
        let startup_repair_error = self
            .startup_repair_errors
            .lock()
            .map_err(|_| CloudSyncError::StateUnavailable)?
            .get(source_id.as_str())
            .cloned();
        Ok(match (owner_phase, startup_repair_error) {
            (Some(phase), startup_repair_error) => Some(CloudSyncStatus {
                phase,
                startup_repair_error,
            }),
            (None, Some(startup_repair_error)) => Some(CloudSyncStatus {
                phase: CloudSyncPhase::NeverSynced,
                startup_repair_error: Some(startup_repair_error),
            }),
            (None, None) => None,
        })
    }

    /// Waits until this owner has published a local snapshot that foreground
    /// Library queries can read. Provider enumeration may continue after the
    /// first bounded publication.
    pub(crate) async fn wait_until_published(
        &self,
        source_id: &SourceId,
    ) -> Result<(), CloudSyncError> {
        loop {
            let (published, idle, running) = {
                let owners = self.owners()?;
                let owner = owners
                    .get(source_id.as_str())
                    .ok_or(CloudSyncError::UnknownSource)?;
                (
                    Arc::clone(&owner.published),
                    Arc::clone(&owner.idle),
                    owner.running,
                )
            };
            let source_id_for_store = source_id.clone();
            let store = Arc::clone(&self.store);
            if blocking_store(store, move |store| {
                store
                    .published_snapshot(&source_id_for_store)
                    .map(|snapshot| snapshot.is_some())
            })
            .await?
            {
                return Ok(());
            }
            if !running {
                return Err(CloudSyncError::StateUnavailable);
            }
            tokio::select! {
                _ = published.notified() => {}
                _ = idle.notified() => {}
            }
        }
    }

    /// Waits until the current owner has no scheduled provider work. A
    /// backoff, pause, or retirement is idle; callers inspect [`Self::status`]
    /// to distinguish those typed outcomes.
    #[cfg(test)]
    pub(crate) async fn wait_until_idle(&self, source_id: &SourceId) -> Result<(), CloudSyncError> {
        loop {
            let idle = {
                let owners = self.owners()?;
                let owner = owners
                    .get(source_id.as_str())
                    .ok_or(CloudSyncError::UnknownSource)?;
                if !owner.running {
                    return Ok(());
                }
                Arc::clone(&owner.idle)
            };
            idle.notified().await;
        }
    }

    /// Performs bounded process-start cleanup and source-scoped local repair
    /// without provider I/O. Source activation remains a separate lifecycle
    /// effect so a pending repair never delays window creation or replays a
    /// remote write.
    pub(crate) async fn startup_repair(
        &self,
        source_ids: impl IntoIterator<Item = SourceId>,
        executor: &dyn CloudWriteRepairExecutor,
    ) -> Result<(), CloudSyncError> {
        {
            let _effects = self.durable_effects.lock().await;
            let store = Arc::clone(&self.store);
            blocking_store(store, |store| store.discard_unreferenced_staging()).await?;
        }
        for source_id in source_ids {
            let repair_owner = self.begin_startup_repair(&source_id)?;
            let result = self
                .resume_startup_repairs(
                    &source_id,
                    repair_owner.epoch,
                    &repair_owner.cancellation,
                    executor,
                )
                .await;
            self.finish_startup_repair(&source_id, repair_owner.epoch)?;
            result?;
        }
        Ok(())
    }

    async fn resume_startup_repairs(
        &self,
        source_id: &SourceId,
        epoch: u64,
        cancellation: &CancellationToken,
        executor: &dyn CloudWriteRepairExecutor,
    ) -> Result<(), CloudSyncError> {
        let mut continuation: Option<RepairContinuation> = None;
        for _ in 0..STARTUP_REPAIR_MAX_PAGES {
            let source_id_for_store = source_id.clone();
            let after = continuation.clone();
            let store = Arc::clone(&self.store);
            let page = blocking_store(store, move |store| {
                store.pending_write_repairs_page(
                    &source_id_for_store,
                    after.as_ref(),
                    STARTUP_REPAIR_PAGE_ROWS,
                )
            })
            .await?;
            for repair in page.repairs() {
                match self
                    .run_startup_repair_executor(cancellation, executor, repair)
                    .await
                {
                    Ok(()) => {
                        if cancellation.is_cancelled() {
                            self.record_startup_repair_error(
                                source_id,
                                epoch,
                                CloudDocumentError::SourcePaused,
                            )?;
                            return Ok(());
                        }
                        let source_id_for_store = source_id.clone();
                        let operation_id = repair.operation_id().to_vec();
                        let store = Arc::clone(&self.store);
                        blocking_store(store, move |store| {
                            store.mark_write_published(&source_id_for_store, &operation_id)
                        })
                        .await?;
                    }
                    Err(error) => self.record_startup_repair_error(source_id, epoch, error)?,
                }
            }
            let Some(next) = page.next_continuation().cloned() else {
                return Ok(());
            };
            continuation = Some(next);
        }
        self.record_startup_repair_error(
            source_id,
            epoch,
            CloudDocumentError::LocalPublicationPending,
        )?;
        Ok(())
    }

    async fn run_startup_repair_executor(
        &self,
        cancellation: &CancellationToken,
        executor: &dyn CloudWriteRepairExecutor,
        repair: &PersistedCloudWriteRepair,
    ) -> Result<(), CloudDocumentError> {
        let deadline = tokio::time::sleep(STARTUP_REPAIR_EXECUTOR_DEADLINE);
        tokio::pin!(deadline);
        tokio::select! {
            _ = cancellation.cancelled() => Err(CloudDocumentError::SourcePaused),
            _ = &mut deadline => Err(CloudDocumentError::Timeout),
            result = executor.resume_local_publication(repair) => result,
        }
    }

    fn begin_startup_repair(
        &self,
        source_id: &SourceId,
    ) -> Result<StartupRepairOwner, CloudSyncError> {
        let repair_owner = StartupRepairOwner {
            epoch: self
                .next_startup_repair_epoch
                .fetch_add(1, Ordering::Relaxed),
            cancellation: CancellationToken::new(),
        };
        let previous = self
            .startup_repair_owners
            .lock()
            .map_err(|_| CloudSyncError::StateUnavailable)?
            .insert(source_id.as_str().to_owned(), repair_owner.clone());
        if let Some(previous) = previous {
            previous.cancellation.cancel();
        }
        self.startup_repair_errors
            .lock()
            .map_err(|_| CloudSyncError::StateUnavailable)?
            .remove(source_id.as_str());
        Ok(repair_owner)
    }

    fn finish_startup_repair(
        &self,
        source_id: &SourceId,
        completed_epoch: u64,
    ) -> Result<(), CloudSyncError> {
        let mut repair_owners = self
            .startup_repair_owners
            .lock()
            .map_err(|_| CloudSyncError::StateUnavailable)?;
        if repair_owners
            .get(source_id.as_str())
            .is_some_and(|owner| owner.epoch == completed_epoch)
        {
            repair_owners.remove(source_id.as_str());
        }
        Ok(())
    }

    fn cancel_startup_repair(&self, source_id: &SourceId) -> Result<(), CloudSyncError> {
        if let Some(cancellation) = self
            .startup_repair_owners
            .lock()
            .map_err(|_| CloudSyncError::StateUnavailable)?
            .get(source_id.as_str())
        {
            cancellation.cancellation.cancel();
        }
        Ok(())
    }

    fn record_startup_repair_error(
        &self,
        source_id: &SourceId,
        epoch: u64,
        error: CloudDocumentError,
    ) -> Result<(), CloudSyncError> {
        let repair_owners = self
            .startup_repair_owners
            .lock()
            .map_err(|_| CloudSyncError::StateUnavailable)?;
        if repair_owners
            .get(source_id.as_str())
            .is_none_or(|owner| owner.epoch != epoch)
        {
            return Ok(());
        }
        self.startup_repair_errors
            .lock()
            .map_err(|_| CloudSyncError::StateUnavailable)?
            .insert(source_id.as_str().to_owned(), error);
        Ok(())
    }

    fn spawn_owner(
        &self,
        source: Arc<dyn MarkdownDocumentSource>,
        source_id: SourceId,
        epoch: u64,
        cancellation: CancellationToken,
    ) {
        let coordinator = self.clone();
        tokio::spawn(async move {
            coordinator
                .run_owner(source, source_id, epoch, cancellation)
                .await;
        });
    }

    async fn run_owner(
        &self,
        source: Arc<dyn MarkdownDocumentSource>,
        source_id: SourceId,
        epoch: u64,
        cancellation: CancellationToken,
    ) {
        let result = self
            .run_owner_inner(&source, &source_id, epoch, &cancellation)
            .await;
        self.finish_owner(&source_id, epoch, result);
    }

    async fn run_owner_inner(
        &self,
        source: &Arc<dyn MarkdownDocumentSource>,
        source_id: &SourceId,
        epoch: u64,
        cancellation: &CancellationToken,
    ) -> Result<i64, CloudDocumentError> {
        self.check_current(source_id, epoch, cancellation)?;
        let pins = self.pins_for(source_id, epoch)?;
        let source_id_for_store = source_id.clone();
        let store = Arc::clone(&self.store);
        let stored_cursor = blocking_store(store, move |store| {
            store.load_change_cursor(&source_id_for_store)
        })
        .await
        .map_err(store_error)?;
        match stored_cursor {
            Some(cursor) => {
                let deadline = Instant::now() + SYNC_DEADLINE;
                self.set_phase_if_current(
                    source_id,
                    epoch,
                    CloudSyncPhase::Refreshing {
                        observed_at_ms: now_ms(),
                    },
                )?;
                match self
                    .run_incremental(
                        &SyncOperation {
                            source,
                            source_id,
                            epoch,
                            cancellation,
                            pins: &pins,
                            query_id: uuid::Uuid::new_v4(),
                            deadline,
                        },
                        cursor,
                    )
                    .await
                {
                    Err(CloudDocumentError::ResetCursor) => {
                        self.discard_staging(source_id, epoch, cancellation).await?;
                        self.run_initial(source, source_id, epoch, cancellation, &pins)
                            .await
                    }
                    result => result,
                }
            }
            None => match self
                .run_initial(source, source_id, epoch, cancellation, &pins)
                .await
            {
                Err(CloudDocumentError::ResetCursor) => {
                    self.discard_staging(source_id, epoch, cancellation).await?;
                    self.run_initial(source, source_id, epoch, cancellation, &pins)
                        .await
                }
                result => result,
            },
        }
    }

    async fn run_initial(
        &self,
        source: &Arc<dyn MarkdownDocumentSource>,
        source_id: &SourceId,
        epoch: u64,
        cancellation: &CancellationToken,
        pins: &CloudDocumentVersionPins,
    ) -> Result<i64, CloudDocumentError> {
        let deadline = Instant::now() + SYNC_DEADLINE;
        let query_id = uuid::Uuid::new_v4();
        let source_id_for_repair = source_id.clone();
        let pins_for_repair = pins.clone();
        let store = Arc::clone(&self.store);
        let repaired_publication = blocking_store(store, move |store| {
            store.repair_published_schema_pin(&source_id_for_repair, &pins_for_repair)
        })
        .await
        .map_err(store_error)?;
        if repaired_publication {
            self.notify_published_if_current(source_id, epoch)?;
        }
        let start_cursor =
            provider_start(source.as_ref(), cancellation, query_id, deadline).await?;
        self.check_current(source_id, epoch, cancellation)?;
        let source_id_for_publication = source_id.clone();
        let has_current_publication = {
            let store = Arc::clone(&self.store);
            let expected_pins = pins.clone();
            blocking_store(store, move |store| {
                store
                    .published_snapshot(&source_id_for_publication)
                    .map(|snapshot| {
                        snapshot.is_some_and(|snapshot| snapshot.pins == expected_pins)
                    })
            })
            .await
            .map_err(store_error)?
        };
        if !has_current_publication {
            if let Some(bootstrap) =
                provider_bootstrap_recent(source.as_ref(), cancellation, query_id, deadline).await?
            {
                let mut budget = PageBudget::default();
                budget.start_page()?;
                budget.charge_metadata(&bootstrap.rows)?;
                if !bootstrap.rows.is_empty() {
                    let observed_at_ms = bootstrap.observed_at_ms;
                    let bootstrap_pins = pins.clone();
                    let source_id_for_bootstrap = source_id.clone();
                    let cursor_for_bootstrap = start_cursor.clone();
                    let generation = self
                        .source_effect(source_id.clone(), epoch, cancellation, move |store| {
                            store.begin_initial_sync(
                                &source_id_for_bootstrap,
                                &cursor_for_bootstrap,
                                &bootstrap_pins,
                                observed_at_ms,
                            )
                        })
                        .await?;
                    let bootstrap_pins = pins.clone();
                    let generation_for_stage = generation.clone();
                    self.source_effect(source_id.clone(), epoch, cancellation, move |store| {
                        store.stage_page(&generation_for_stage, &bootstrap_pins, &bootstrap)
                    })
                    .await?;
                    let generation_for_publish = generation.clone();
                    self.source_effect(source_id.clone(), epoch, cancellation, move |store| {
                        store.publish_provisional_sync(&generation_for_publish, observed_at_ms)
                    })
                    .await?;
                    self.notify_published_if_current(source_id, epoch)?;
                }
            }
        }
        let pins = pins.clone();
        let pins_for_begin = pins.clone();
        let source_id_for_store = source_id.clone();
        let start_cursor_for_staging = start_cursor.clone();
        let generation = self
            .source_effect(source_id.clone(), epoch, cancellation, move |store| {
                store.begin_initial_sync(
                    &source_id_for_store,
                    &start_cursor_for_staging,
                    &pins_for_begin,
                    now_ms(),
                )
            })
            .await?;
        self.set_phase_if_current(
            source_id,
            epoch,
            CloudSyncPhase::Initializing {
                generation: generation.id(),
            },
        )?;
        let operation = SyncOperation {
            source,
            source_id,
            epoch,
            cancellation,
            pins: &pins,
            query_id,
            deadline,
        };

        let observed_at_ms = self
            .stage_initial_pages(&operation, generation.clone())
            .await?;
        self.check_current(source_id, epoch, cancellation)?;
        let start_cursor_for_publish = start_cursor.clone();
        let generation_for_publish = generation.clone();
        self.source_effect(source_id.clone(), epoch, cancellation, move |store| {
            store.publish_initial_sync(
                &generation_for_publish,
                &start_cursor_for_publish,
                observed_at_ms,
            )
        })
        .await?;
        self.notify_published_if_current(source_id, epoch)?;
        self.check_current(source_id, epoch, cancellation)?;
        self.run_incremental(&operation, start_cursor).await
    }

    async fn stage_initial_pages(
        &self,
        operation: &SyncOperation<'_>,
        generation: super::store::CloudSnapshotGeneration,
    ) -> Result<i64, CloudDocumentError> {
        let mut cursor = None;
        let mut seen = HashSet::new();
        let mut budget = PageBudget::default();
        loop {
            budget.start_page()?;
            let page = provider_list(
                operation.source.as_ref(),
                cursor.as_ref(),
                operation.cancellation,
                operation.query_id,
                operation.deadline,
            )
            .await?;
            budget.charge_metadata(&page.rows)?;
            let observed_at_ms = page.observed_at_ms;
            let pins = operation.pins.clone();
            let generation_for_store = generation.clone();
            let next_cursor = page.next_cursor.clone();
            self.source_effect(
                operation.source_id.clone(),
                operation.epoch,
                operation.cancellation,
                move |store| store.stage_page(&generation_for_store, &pins, &page),
            )
            .await?;
            let Some(next) = next_cursor else {
                return Ok(observed_at_ms);
            };
            if !seen.insert(next.0.clone()) {
                return Err(CloudDocumentError::ProtocolFailed);
            }
            cursor = Some(next);
        }
    }

    async fn run_incremental(
        &self,
        operation: &SyncOperation<'_>,
        mut cursor: ProviderCursor,
    ) -> Result<i64, CloudDocumentError> {
        let mut budget = PageBudget::default();
        let mut seen = HashSet::new();
        loop {
            budget.start_page()?;
            self.check_current(operation.source_id, operation.epoch, operation.cancellation)?;
            let page = provider_changes(
                operation.source.as_ref(),
                &cursor,
                operation.cancellation,
                operation.query_id,
                operation.deadline,
            )
            .await?;
            budget.charge_change_events(&page.changes)?;
            let keys = page
                .changes
                .iter()
                .map(|event| event.key().clone())
                .collect::<Vec<_>>();
            let source_id_for_store = operation.source_id.clone();
            let store = Arc::clone(&self.store);
            let known = blocking_store(store, move |store| {
                store.published_for_keys(&source_id_for_store, &keys)
            })
            .await
            .map_err(store_error)?
            .into_iter()
            .map(|document| KnownMarkdownMetadata {
                key: document.key,
                name: document.name,
                display_path: document.display_path,
                media_type: document.media_type,
                modified_at_ms: document.modified_at_ms,
                size_bytes: document.size_bytes,
                revision: document.revision,
                deleted: document.deleted,
            })
            .collect::<Vec<_>>();
            let changes = provider_materialize_changes(
                operation.source.as_ref(),
                &page.changes,
                &known,
                operation.cancellation,
                operation.query_id,
                operation.deadline,
            )
            .await?;
            if changes.len() > page.changes.len() {
                return Err(CloudDocumentError::ProtocolFailed);
            }
            budget.charge_metadata(&changes)?;
            if page.has_more
                && (!seen.insert(page.next_cursor.0.clone()) || page.next_cursor == cursor)
            {
                return Err(CloudDocumentError::ProtocolFailed);
            }
            let observed_at_ms = page.observed_at_ms;
            let has_more = page.has_more;
            let metadata = MarkdownMetadataPage {
                rows: changes,
                next_cursor: None,
                observed_at_ms,
            };
            let pins = operation.pins.clone();
            let next_cursor = page.next_cursor;
            let next_cursor_for_store = next_cursor.clone();
            let source_id_for_store = operation.source_id.clone();
            self.source_effect(
                operation.source_id.clone(),
                operation.epoch,
                operation.cancellation,
                move |store| {
                    store.apply_change_page(
                        &source_id_for_store,
                        &pins,
                        &metadata,
                        &next_cursor_for_store,
                        observed_at_ms,
                    )
                },
            )
            .await?;
            if !has_more {
                return Ok(observed_at_ms);
            }
            cursor = next_cursor;
        }
    }

    async fn discard_staging(
        &self,
        source_id: &SourceId,
        epoch: u64,
        cancellation: &CancellationToken,
    ) -> Result<(), CloudDocumentError> {
        let source_id = source_id.clone();
        self.source_effect(source_id.clone(), epoch, cancellation, move |store| {
            store.discard_staging(&source_id)
        })
        .await
    }

    fn check_current(
        &self,
        source_id: &SourceId,
        epoch: u64,
        cancellation: &CancellationToken,
    ) -> Result<(), CloudDocumentError> {
        if cancellation.is_cancelled() {
            return Err(CloudDocumentError::Cancelled);
        }
        let owners = self
            .owners()
            .map_err(|_| CloudDocumentError::ProtocolFailed)?;
        let Some(owner) = owners.get(source_id.as_str()) else {
            return Err(CloudDocumentError::SourceDisconnected);
        };
        if owner.epoch != epoch || owner.cancellation.is_cancelled() {
            return Err(match owner.phase {
                CloudSyncPhase::Paused => CloudDocumentError::SourcePaused,
                CloudSyncPhase::Retired => CloudDocumentError::SourceDisconnected,
                _ => CloudDocumentError::Cancelled,
            });
        }
        Ok(())
    }

    fn set_phase_if_current(
        &self,
        source_id: &SourceId,
        epoch: u64,
        phase: CloudSyncPhase,
    ) -> Result<(), CloudDocumentError> {
        let mut owners = self
            .owners()
            .map_err(|_| CloudDocumentError::ProtocolFailed)?;
        let Some(owner) = owners.get_mut(source_id.as_str()) else {
            return Err(CloudDocumentError::SourceDisconnected);
        };
        if owner.epoch != epoch || owner.cancellation.is_cancelled() {
            return Err(CloudDocumentError::Cancelled);
        }
        owner.phase = phase;
        Ok(())
    }

    fn pins_for(
        &self,
        source_id: &SourceId,
        epoch: u64,
    ) -> Result<CloudDocumentVersionPins, CloudDocumentError> {
        let owners = self
            .owners()
            .map_err(|_| CloudDocumentError::ProtocolFailed)?;
        let owner = owners
            .get(source_id.as_str())
            .ok_or(CloudDocumentError::SourceDisconnected)?;
        if owner.epoch != epoch || owner.cancellation.is_cancelled() {
            return Err(CloudDocumentError::Cancelled);
        }
        Ok(owner.pins.clone())
    }

    async fn source_effect<T: Send + 'static>(
        &self,
        source_id: SourceId,
        epoch: u64,
        cancellation: &CancellationToken,
        operation: impl FnOnce(&CloudDocumentStore) -> Result<T, CloudDocumentStoreError>
            + Send
            + 'static,
    ) -> Result<T, CloudDocumentError> {
        let _effects = self.durable_effects.lock().await;
        self.check_current(&source_id, epoch, cancellation)?;
        #[cfg(test)]
        self.wait_for_test_effect_gate().await;
        blocking_store(Arc::clone(&self.store), operation)
            .await
            .map_err(store_error)
    }

    #[cfg(test)]
    async fn wait_for_test_effect_gate(&self) {
        let barrier = {
            let mut configured_gate = self.effect_gate.lock().expect("test effect gate lock");
            let Some(gate) = configured_gate.as_mut() else {
                return;
            };
            if gate.ordinal > 1 {
                gate.ordinal -= 1;
                return;
            }
            configured_gate.take().map(|gate| gate.barrier)
        };
        if let Some(barrier) = barrier {
            barrier.entered.notify_waiters();
            barrier.release.notified().await;
        }
    }

    fn finish_owner(
        &self,
        source_id: &SourceId,
        epoch: u64,
        result: Result<i64, CloudDocumentError>,
    ) {
        let Ok(mut owners) = self.owners() else {
            return;
        };
        let Some(owner) = owners.get_mut(source_id.as_str()) else {
            return;
        };
        if owner.epoch != epoch {
            return;
        }
        owner.running = false;
        match result {
            Ok(observed_at_ms) => {
                owner.phase = CloudSyncPhase::Published { observed_at_ms };
            }
            Err(CloudDocumentError::Cancelled | CloudDocumentError::SourcePaused)
                if matches!(
                    owner.phase,
                    CloudSyncPhase::Paused | CloudSyncPhase::Retired
                ) => {}
            Err(CloudDocumentError::SourceDisconnected)
                if matches!(owner.phase, CloudSyncPhase::Retired) => {}
            Err(error) => {
                let kind = serde_json::to_value(&error)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("kind")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| "unknown".to_owned());
                eprintln!("MDReader cloud sync failed: {kind}");
                owner.phase = CloudSyncPhase::Backoff {
                    retry_at_ms: retry_at_ms(&error),
                    error,
                };
            }
        }
        owner.idle.notify_waiters();
    }

    fn notify_published_if_current(
        &self,
        source_id: &SourceId,
        epoch: u64,
    ) -> Result<(), CloudDocumentError> {
        let owners = self
            .owners()
            .map_err(|_| CloudDocumentError::ProtocolFailed)?;
        let owner = owners
            .get(source_id.as_str())
            .ok_or(CloudDocumentError::SourceDisconnected)?;
        if owner.epoch != epoch || owner.cancellation.is_cancelled() {
            return Err(CloudDocumentError::Cancelled);
        }
        owner.published.notify_one();
        Ok(())
    }

    fn owners(&self) -> Result<MutexGuard<'_, HashMap<String, SyncOwner>>, CloudSyncError> {
        self.owners
            .lock()
            .map_err(|_| CloudSyncError::StateUnavailable)
    }
}

#[derive(Default)]
struct PageBudget {
    pages: usize,
    rows: usize,
    bytes: usize,
}

impl PageBudget {
    fn start_page(&mut self) -> Result<(), CloudDocumentError> {
        self.pages = self
            .pages
            .checked_add(1)
            .ok_or(CloudDocumentError::TooLarge)?;
        if self.pages > SYNC_MAX_PAGES {
            return Err(CloudDocumentError::TooLarge);
        }
        Ok(())
    }

    fn charge_metadata(&mut self, rows: &[MarkdownMetadata]) -> Result<(), CloudDocumentError> {
        if rows.len() > SYNC_PAGE_ROWS {
            return Err(CloudDocumentError::TooLarge);
        }
        let page_bytes = rows.iter().try_fold(0_usize, |total, row| {
            total
                .checked_add(metadata_bytes(row))
                .ok_or(CloudDocumentError::TooLarge)
        })?;
        if page_bytes > SYNC_PAGE_BYTES {
            return Err(CloudDocumentError::TooLarge);
        }
        self.rows = self
            .rows
            .checked_add(rows.len())
            .ok_or(CloudDocumentError::TooLarge)?;
        self.bytes = self
            .bytes
            .checked_add(page_bytes)
            .ok_or(CloudDocumentError::TooLarge)?;
        if self.rows > SYNC_MAX_ROWS || self.bytes > SYNC_MAX_BYTES {
            return Err(CloudDocumentError::TooLarge);
        }
        Ok(())
    }

    fn charge_change_events(
        &mut self,
        events: &[MarkdownChangeEvent],
    ) -> Result<(), CloudDocumentError> {
        if events.len() > SYNC_PAGE_ROWS {
            return Err(CloudDocumentError::TooLarge);
        }
        let page_bytes = events.iter().try_fold(0_usize, |total, event| {
            total
                .checked_add(event.key().as_str().len())
                .ok_or(CloudDocumentError::TooLarge)
        })?;
        if page_bytes > SYNC_PAGE_BYTES {
            return Err(CloudDocumentError::TooLarge);
        }
        Ok(())
    }
}

fn metadata_bytes(row: &MarkdownMetadata) -> usize {
    row.key
        .as_str()
        .len()
        .saturating_add(row.name.len())
        .saturating_add(row.display_path.len())
        .saturating_add(row.media_type.as_ref().map_or(0, String::len))
        .saturating_add(row.revision.opaque.len())
        .saturating_add(std::mem::size_of::<i64>() * 2)
}

fn store_error(error: CloudSyncError) -> CloudDocumentError {
    match error {
        CloudSyncError::Store(_)
        | CloudSyncError::RuntimeUnavailable
        | CloudSyncError::StateUnavailable
        | CloudSyncError::MissingVersionPins => CloudDocumentError::ProtocolFailed,
        CloudSyncError::UnknownSource | CloudSyncError::Retired => {
            CloudDocumentError::SourceDisconnected
        }
    }
}

async fn blocking_store<T: Send + 'static>(
    store: Arc<CloudDocumentStore>,
    operation: impl FnOnce(&CloudDocumentStore) -> Result<T, CloudDocumentStoreError> + Send + 'static,
) -> Result<T, CloudSyncError> {
    tauri::async_runtime::spawn_blocking(move || operation(store.as_ref()))
        .await
        .map_err(|_| CloudSyncError::RuntimeUnavailable)?
        .map_err(CloudSyncError::Store)
}

fn context(
    cancellation: &CancellationToken,
    query_id: uuid::Uuid,
    deadline: Instant,
) -> CloudIoContext {
    CloudIoContext {
        query_id,
        deadline,
        page_rows: SYNC_PAGE_ROWS,
        byte_limit: SYNC_PAGE_BYTES,
        cancellation: cancellation.clone(),
    }
}

async fn provider_start(
    source: &dyn MarkdownDocumentSource,
    cancellation: &CancellationToken,
    query_id: uuid::Uuid,
    deadline: Instant,
) -> Result<ProviderCursor, CloudDocumentError> {
    let context = context(cancellation, query_id, deadline);
    provider_call(
        cancellation,
        context.deadline,
        source.start_change_tracking(&context),
    )
    .await
}

async fn provider_bootstrap_recent(
    source: &dyn MarkdownDocumentSource,
    cancellation: &CancellationToken,
    query_id: uuid::Uuid,
    deadline: Instant,
) -> Result<Option<MarkdownMetadataPage>, CloudDocumentError> {
    let context = context(cancellation, query_id, deadline);
    provider_call(
        cancellation,
        context.deadline,
        source.bootstrap_recent(&context),
    )
    .await
}

async fn provider_list(
    source: &dyn MarkdownDocumentSource,
    cursor: Option<&ProviderCursor>,
    cancellation: &CancellationToken,
    query_id: uuid::Uuid,
    deadline: Instant,
) -> Result<MarkdownMetadataPage, CloudDocumentError> {
    let context = context(cancellation, query_id, deadline);
    provider_call(
        cancellation,
        context.deadline,
        source.list_page(cursor, &context),
    )
    .await
}

async fn provider_changes(
    source: &dyn MarkdownDocumentSource,
    cursor: &ProviderCursor,
    cancellation: &CancellationToken,
    query_id: uuid::Uuid,
    deadline: Instant,
) -> Result<MarkdownChangePage, CloudDocumentError> {
    let context = context(cancellation, query_id, deadline);
    provider_call(
        cancellation,
        context.deadline,
        source.read_changes(cursor, &context),
    )
    .await
}

async fn provider_materialize_changes(
    source: &dyn MarkdownDocumentSource,
    changes: &[MarkdownChangeEvent],
    known: &[KnownMarkdownMetadata],
    cancellation: &CancellationToken,
    query_id: uuid::Uuid,
    deadline: Instant,
) -> Result<Vec<MarkdownMetadata>, CloudDocumentError> {
    let context = context(cancellation, query_id, deadline);
    provider_call(
        cancellation,
        context.deadline,
        source.materialize_changes(changes, known, &context),
    )
    .await
}

pub(super) async fn provider_call<T>(
    cancellation: &CancellationToken,
    deadline: Instant,
    operation: impl std::future::Future<Output = Result<T, CloudDocumentError>>,
) -> Result<T, CloudDocumentError> {
    if cancellation.is_cancelled() {
        return Err(CloudDocumentError::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(CloudDocumentError::Timeout);
    }
    let deadline_timer = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
    tokio::pin!(deadline_timer);
    let result = tokio::select! {
        _ = cancellation.cancelled() => return Err(CloudDocumentError::Cancelled),
        _ = &mut deadline_timer => return Err(CloudDocumentError::Timeout),
        result = operation => result,
    }?;
    if cancellation.is_cancelled() {
        return Err(CloudDocumentError::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(CloudDocumentError::Timeout);
    }
    Ok(result)
}

fn retry_at_ms(error: &CloudDocumentError) -> i64 {
    let delay = match error {
        CloudDocumentError::RateLimited {
            retry_after_ms: Some(delay),
        } => *delay,
        _ => DEFAULT_RETRY_AFTER_MS,
    };
    now_ms().saturating_add(i64::try_from(delay).unwrap_or(i64::MAX))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}
