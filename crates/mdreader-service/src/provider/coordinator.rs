//! Bounded service transport over the owner registry. Opaque handles never carry
//! raw provider IDs, account IDs, revision validators, cursors or credentials.
use super::*;
use mdreader_credentials::CredentialReference;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
#[path = "editing.rs"]
mod editing;

const DEADLINE: Duration = Duration::from_secs(30);
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderRequest {
    List,
    RecoveredDrafts {
        offset: u32,
        limit: u32,
    },
    ReadRecoveredDraft {
        draft_id: Uuid,
        revision: String,
    },
    /// Native-only input from completed OAuth custody, never renderer-supplied.
    Admit {
        credential_reference: String,
        name: DisplayText,
    },
    Reconnect {
        source_id: SourceId,
        credential_reference: String,
        name: DisplayText,
    },
    BrowseFolders {
        source_id: SourceId,
        folder_id: RecordKey,
        cursor: Option<ProviderCursor>,
    },
    Documents {
        source_id: SourceId,
        offset: usize,
        limit: usize,
    },
    ResolveLink {
        reader_source_id: SourceId,
        path: RecordKey,
        revision: OpaqueValue,
        kind: ProviderLinkKind,
        target: String,
    },
    Refresh {
        source_id: SourceId,
    },
    Open {
        source_id: SourceId,
        document_id: Uuid,
    },
    CachedOpen {
        source_id: SourceId,
        document_id: Uuid,
    },
    Disable {
        source_id: SourceId,
    },
    Forget {
        source_id: SourceId,
    },
    BeginEditExact {
        reader_source_id: SourceId,
        path: RecordKey,
        revision: OpaqueValue,
    },
    CreateDocument {
        source_id: SourceId,
        operation_id: Uuid,
        filename: String,
        markdown: String,
    },
    InspectCreate {
        source_id: SourceId,
        operation_id: Uuid,
    },
    InspectSave {
        source_id: SourceId,
        operation_id: Uuid,
    },
    BeginEdit {
        source_id: SourceId,
        document_id: Uuid,
    },
    PrepareSave {
        source_id: SourceId,
        edit_id: Uuid,
        markdown: String,
    },
    DispatchSave {
        source_id: SourceId,
        operation_id: Uuid,
    },
    Repair {
        source_id: SourceId,
    },
    Mutations {
        source_id: SourceId,
    },
}
impl ProviderRequest {
    fn scope(&self) -> String {
        match self {
            Self::List
            | Self::RecoveredDrafts { .. }
            | Self::ReadRecoveredDraft { .. }
            | Self::Admit { .. } => "registry".into(),
            Self::BeginEditExact {
                reader_source_id, ..
            }
            | Self::ResolveLink {
                reader_source_id, ..
            } => reader_source_id.as_str().into(),
            Self::Reconnect { source_id, .. }
            | Self::BrowseFolders { source_id, .. }
            | Self::Documents { source_id, .. }
            | Self::Refresh { source_id }
            | Self::Open { source_id, .. }
            | Self::CachedOpen { source_id, .. }
            | Self::Disable { source_id }
            | Self::Forget { source_id }
            | Self::BeginEdit { source_id, .. }
            | Self::PrepareSave { source_id, .. }
            | Self::DispatchSave { source_id, .. }
            | Self::Repair { source_id }
            | Self::CreateDocument { source_id, .. }
            | Self::InspectCreate { source_id, .. }
            | Self::InspectSave { source_id, .. }
            | Self::Mutations { source_id } => source_id.as_str().into(),
        }
    }
}
struct Command {
    request: ProviderRequest,
    reply: mpsc::SyncSender<Result<Value>>,
    deadline: Instant,
    cancellation: Cancellation,
    effect: Option<crate::daemon::EffectLease>,
    agent_fence:Option<crate::agents::WriteFence>,
}
#[derive(Clone)]
pub struct Coordinator {
    sender: tokio::sync::mpsc::Sender<Command>,
    local: Arc<dyn Fn(ProviderRequest) -> Result<Value> + Send + Sync>,
}
impl Coordinator {
    pub fn new<O: Owner + 'static>(
        owner: Arc<O>,
        projection: Arc<dyn ProjectionAdmission>,
        factory: Arc<dyn ProviderFactory>,
    ) -> Result<Self> {
        owner.with_connection(|c| {
            initialize_client(c)?;
            let library = library_id(c)?;
            for source in list(c, &library)?.into_iter().filter(|s| s.enabled) {
                let configuration = configuration(c, &source.source_id)?;
                journal::recover_interrupted(c, configuration.descriptor.binding())?;
                mdreader_provider::creation::recover_interrupted(
                    c,
                    configuration.descriptor.binding(),
                )?;
            }
            Ok(())
        })?;
        let local_owner = owner.clone();
        let local_projection = projection.clone();
        let local = Arc::new(move |request: ProviderRequest| {
            local_owner.with_connection(|c| {
                if matches!(request, ProviderRequest::List) {
                    let library = library_id(c)?;
                    return serde_json::to_value(list(c, &library)?)
                        .map_err(|_| ProviderError::Protocol);
                }
                if let ProviderRequest::ResolveLink {
                    reader_source_id,
                    path,
                    revision,
                    kind,
                    target,
                } = request
                {
                    return serde_json::to_value(links::resolve_link(
                        c,
                        &reader_source_id,
                        &path,
                        &revision,
                        kind,
                        &target,
                    )?)
                    .map_err(|_| ProviderError::Protocol);
                }
                if let ProviderRequest::BeginEditExact {
                    reader_source_id,
                    path,
                    revision,
                } = request
                {
                    return editing::begin_edit_exact(c, &reader_source_id, &path, &revision);
                }
                match &request {
                    ProviderRequest::RecoveredDrafts { offset, limit } => {
                        return serde_json::to_value(draft_recovery::page(c, *offset, *limit)?)
                            .map_err(|_| ProviderError::Protocol);
                    }
                    ProviderRequest::ReadRecoveredDraft { draft_id, revision } => {
                        return serde_json::to_value(draft_recovery::read(c, *draft_id, revision)?)
                            .map_err(|_| ProviderError::Protocol);
                    }
                    _ => {}
                }
                let source = SourceId::new(request.scope())?;
                let configuration = configuration(c, &source)?;
                let binding = configuration.descriptor.binding();
                match request {
                    ProviderRequest::InspectSave { operation_id, .. } => {
                        editing::inspect_save(c, binding, operation_id)
                    }
                    ProviderRequest::CachedOpen { document_id, .. } => {
                        cached_open(c, binding, document_id)
                    }
                    ProviderRequest::Documents { offset, limit, .. } => {
                        documents(c, binding, offset, limit)
                    }
                    ProviderRequest::Disable { .. } => {
                        disable(c, binding, &*local_projection)?;
                        Ok(json!({"disabled":true}))
                    }
                    ProviderRequest::Forget { .. } => {
                        forget_materialization(c, binding, &*local_projection)?;
                        Ok(json!({"forgotten":true,"disabled":true}))
                    }
                    _ => Err(ProviderError::Invalid),
                }
            })
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| ProviderError::Unavailable)?;
        let (sender, receiver) = tokio::sync::mpsc::channel::<Command>(8);
        std::thread::Builder::new()
            .name("provider-coordinator".into())
            .spawn(move || runtime.block_on(run(owner, projection, factory, receiver)))
            .map_err(|_| ProviderError::Unavailable)?;
        Ok(Self { sender, local })
    }
    pub fn execute(&self, request: ProviderRequest) -> Result<Value> {
        self.execute_with_effect(request, None)
    }
    pub(crate) fn execute_agent(&self,request:ProviderRequest,fence:crate::agents::WriteFence,effect:crate::daemon::EffectLease)->Result<Value>{
        self.execute_scoped(request,Some(effect),Some(fence))
    }
    pub(crate) fn execute_owned(
        &self,
        request: ProviderRequest,
        effect: crate::daemon::EffectLease,
    ) -> Result<Value> {
        self.execute_with_effect(request, Some(effect))
    }
    fn execute_with_effect(
        &self,
        request: ProviderRequest,
        effect: Option<crate::daemon::EffectLease>,
    ) -> Result<Value> {
        self.execute_scoped(request,effect,None)
    }
    fn execute_scoped(&self,request:ProviderRequest,effect:Option<crate::daemon::EffectLease>,agent_fence:Option<crate::agents::WriteFence>)->Result<Value>{
        // Revocation never waits for a network slot or a credential worker.
        if agent_fence.is_none() && matches!(
            request,
            ProviderRequest::List
                | ProviderRequest::RecoveredDrafts { .. }
                | ProviderRequest::ReadRecoveredDraft { .. }
                | ProviderRequest::BeginEditExact { .. }
                | ProviderRequest::InspectSave { .. }
                | ProviderRequest::ResolveLink { .. }
                | ProviderRequest::CachedOpen { .. }
                | ProviderRequest::Documents { .. }
                | ProviderRequest::Disable { .. }
                | ProviderRequest::Forget { .. }
        ) {
            return (self.local)(request);
        }
        let (reply, receiver) = mpsc::sync_channel(1);
        let cancellation = Cancellation::default();
        self.sender
            .try_send(Command {
                request,
                reply,
                deadline: Instant::now() + DEADLINE,
                cancellation: cancellation.clone(),
                effect,
                agent_fence,
            })
            .map_err(|_| ProviderError::Unavailable)?;
        let result = receiver
            .recv_timeout(DEADLINE + Duration::from_secs(1))
            .map_err(|_| ProviderError::Timeout);
        if result.is_err() {
            cancellation.cancel()
        }
        result?
    }
}
struct OperationGuard {
    active: Arc<Mutex<BTreeSet<String>>>,
    scope: String,
}
impl Drop for OperationGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(&self.scope);
        }
    }
}
async fn run<O: Owner + 'static>(
    owner: Arc<O>,
    projection: Arc<dyn ProjectionAdmission>,
    factory: Arc<dyn ProviderFactory>,
    mut receiver: tokio::sync::mpsc::Receiver<Command>,
) {
    let active = Arc::new(Mutex::new(BTreeSet::new()));
    while let Some(command) = receiver.recv().await {
        let scope = command.request.scope();
        let allowed = active.lock().ok().is_some_and(|mut active| {
            if active.len() >= 2 || active.contains(&scope) {
                false
            } else {
                active.insert(scope.clone());
                true
            }
        });
        if !allowed {
            let _ = command.reply.send(Err(ProviderError::Unavailable));
            continue;
        }
        let owner=Arc::new(super::agent_fence::ScopedOwner{owner:owner.clone(),fence:command.agent_fence.clone()});
        let projection = projection.clone();
        let factory = factory.clone();
        let active = active.clone();
        tokio::spawn(async move {
            // Keep the actual issued effect through timeout/cancel Unknown
            // journal cleanup even when execute's caller has abandoned reply.
            let _effect = command.effect;
            let _guard = OperationGuard { active, scope };
            let interrupted = match &command.request {
                ProviderRequest::DispatchSave {
                    source_id,
                    operation_id,
                } => Some((source_id.clone(), *operation_id, false)),
                ProviderRequest::CreateDocument {
                    source_id,
                    operation_id,
                    ..
                } => Some((source_id.clone(), *operation_id, true)),
                _ => None,
            };
            let context = IoContext::new(
                Uuid::new_v4(),
                command.deadline,
                256,
                8 * 1024 * 1024,
                command.cancellation.clone(),
            );
            let result = match context {
                Ok(context) => tokio::select! {biased;
                    _=context.cancellation().cancelled()=>Err(ProviderError::Cancelled),
                    result=tokio::time::timeout_at(tokio::time::Instant::from_std(context.deadline()),perform(owner.clone(),&*projection,&*factory,command.request,&context))=>result.unwrap_or(Err(ProviderError::Timeout))
                },
                Err(error) => Err(error),
            };
            // The coordinator deadline can win the race against inner cleanup.
            // Preserve dispatch uncertainty immediately rather than awaiting restart.
            let result = if matches!(
                result,
                Err(ProviderError::Timeout | ProviderError::Cancelled)
            ) {
                if let Some((source, id, is_create)) = interrupted {
                    owner.with_connection(|c| {
                        let configuration = configuration(c, &source)?;
                        let binding = configuration.descriptor.binding();
                        if is_create {
                            mdreader_provider::creation::initialize(c)?;
                            if let Some(pending) =
                                mdreader_provider::creation::load(c, binding, id)?
                            {
                                if matches!(
                                    pending.state,
                                    journal::MutationState::Dispatched
                                        | journal::MutationState::Unknown
                                ) {
                                    mdreader_provider::creation::uncertain(c, binding, id)?;
                                    return Err(ProviderError::UnknownWrite { operation_id: id });
                                }
                            }
                            return result;
                        }
                        let pending = journal::load(c, binding, id)?;
                        if matches!(
                            pending.state,
                            journal::MutationState::Dispatched | journal::MutationState::Unknown
                        ) {
                            journal::mark_unknown(c, binding, id)?;
                            Err(ProviderError::UnknownWrite { operation_id: id })
                        } else {
                            result
                        }
                    })
                } else {
                    result
                }
            } else {
                result
            };
            let _ = command.reply.send(result);
        });
    }
}
fn now() -> Result<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ProviderError::Protocol)?
        .as_millis()
        .try_into()
        .map_err(|_| ProviderError::Protocol)
}
pub(super) fn initialize_client(c: &mut Connection) -> Result<()> {
    initialize(c)?;
    mdreader_provider::creation::initialize(c)?;
    let tx = registry::db(c.transaction())?;
    let count:u64=registry::db(tx.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('mdreader_provider_client_meta','mdreader_provider_client_handles','mdreader_provider_client_edits')",[],|r|r.get(0)))?;
    if count == 0 {
        registry::db(tx.execute_batch("CREATE TABLE mdreader_provider_client_meta(id INTEGER PRIMARY KEY CHECK(id=1),version INTEGER NOT NULL);INSERT INTO mdreader_provider_client_meta VALUES(1,1);CREATE TABLE mdreader_provider_client_handles(id TEXT PRIMARY KEY,binding BLOB NOT NULL,key TEXT NOT NULL,UNIQUE(binding,key));CREATE TABLE mdreader_provider_client_edits(id TEXT PRIMARY KEY,binding BLOB NOT NULL,key TEXT NOT NULL,expected TEXT NOT NULL,expires INTEGER NOT NULL,consumed INTEGER NOT NULL);"))?;
    } else if count != 3 {
        return Err(ProviderError::Persistence);
    }
    let version: u64 = registry::db(tx.query_row(
        "SELECT version FROM mdreader_provider_client_meta WHERE id=1",
        [],
        |r| r.get(0),
    ))?;
    if version != 1 {
        return Err(ProviderError::Persistence);
    }
    registry::db(tx.commit())
}
/// Private bounded metadata factory input, never raw provider metadata to a UI.
pub fn published_metadata(
    c: &mut Connection,
    b: &SourceBinding,
    offset: usize,
    limit: usize,
) -> Result<Vec<Metadata>> {
    if limit == 0 || limit > 16384 || offset > 16384 {
        return Err(ProviderError::Invalid);
    }
    revalidate(c, b)?;
    let mut statement=registry::db(c.prepare("SELECT metadata,digest FROM mdreader_provider_registry_rows WHERE snapshot=(SELECT published FROM mdreader_provider_registry_state WHERE source=?1) ORDER BY key LIMIT ?2 OFFSET ?3"))?;
    let rows = registry::db(
        statement.query_map(params![registry::identity(b), limit, offset], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        }),
    )?;
    rows.map(|row| {
        let (text, digest) = registry::db(row)?;
        if body_hash(text.as_bytes()).as_slice() != digest {
            return Err(ProviderError::Persistence);
        }
        registry::decode(&text, 32 * 1024)
    })
    .collect()
}
fn key_for(c: &mut Connection, b: &SourceBinding, id: Uuid) -> Result<RecordKey> {
    revalidate(c, b)?;
    let key: Option<String> = registry::db(
        c.query_row(
            "SELECT key FROM mdreader_provider_client_handles WHERE id=?1 AND binding=?2",
            params![id.to_string(), b.digest().as_slice()],
            |r| r.get(0),
        )
        .optional(),
    )?;
    RecordKey::new(key.ok_or(ProviderError::Unauthorized)?)
}
fn cached_open(c: &mut Connection, b: &SourceBinding, id: Uuid) -> Result<Value> {
    let key = key_for(c, b, id)?;
    let projection = cached(c, b, &key)?
        .and_then(|d| d.projection)
        .ok_or(ProviderError::Unavailable)?;
    serde_json::to_value(projection).map_err(|_| ProviderError::Protocol)
}
pub(super) fn handle_for(c: &mut Connection, b: &SourceBinding, key: &RecordKey) -> Result<Uuid> {
    let existing: Option<String> = registry::db(
        c.query_row(
            "SELECT id FROM mdreader_provider_client_handles WHERE binding=?1 AND key=?2",
            params![b.digest().as_slice(), key.as_str()],
            |r| r.get(0),
        )
        .optional(),
    )?;
    let id = match existing {
        Some(id) => id,
        None => {
            let count: u64 = registry::db(c.query_row(
                "SELECT count(*) FROM mdreader_provider_client_handles",
                [],
                |r| r.get(0),
            ))?;
            if count >= 16384 {
                return Err(ProviderError::TooLarge);
            }
            let id = Uuid::new_v4().to_string();
            registry::db(c.execute(
                "INSERT INTO mdreader_provider_client_handles VALUES(?1,?2,?3)",
                params![id, b.digest().as_slice(), key.as_str()],
            ))?;
            id
        }
    };
    Uuid::parse_str(&id).map_err(|_| ProviderError::Persistence)
}
pub(super) fn documents(
    c: &mut Connection,
    b: &SourceBinding,
    offset: usize,
    limit: usize,
) -> Result<Value> {
    if limit == 0 || limit > 256 {
        return Err(ProviderError::Invalid);
    }
    let page = document_page::read(c, b, offset, limit)?;
    let mut result = Vec::new();
    // Handles belonging to disabled/replaced bindings no longer authorize access.
    registry::db(c.execute("DELETE FROM mdreader_provider_client_handles WHERE binding NOT IN (SELECT binding FROM mdreader_provider_registry_sources WHERE enabled=1)",[]))?;
    for m in page.rows {
        let id = handle_for(c, b, &m.key)?;
        let cache = cached(c, b, &m.key)?;
        result.push(json!({"document_id":id,"name":m.name,"display_path":m.display_path,"modified_at_ms":m.modified_at_ms,"size_bytes":m.size_bytes,"cached":cache.is_some(),"cached_observed_at_ms":cache.as_ref().map(|d|d.document.observed_at_ms),"admitted":cache.as_ref().is_some_and(|d|d.projection.is_some()),"projection":cache.as_ref().and_then(|d|d.projection.as_ref())}));
    }
    Ok(json!({"documents":result,"offset":offset,"next_offset":page.next_offset}))
}
async fn perform<O: Owner + 'static>(
    owner: Arc<O>,
    projection: &dyn ProjectionAdmission,
    factory: &dyn ProviderFactory,
    request: ProviderRequest,
    context: &IoContext,
) -> Result<Value> {
    context.check()?;
    if matches!(request, ProviderRequest::List) {
        return owner.with_connection(|c| {
            let library = library_id(c)?;
            serde_json::to_value(list(c, &library)?).map_err(|_| ProviderError::Protocol)
        });
    }
    if let ProviderRequest::Admit {
        credential_reference,
        name,
    } = request
    {
        let library = owner.with_connection(library_id)?;
        let reference = CredentialReference::new(credential_reference)?;
        let configuration = factory
            .describe(
                library,
                SourceId::new(format!("remote-{}", Uuid::new_v4()))?,
                name,
                reference,
                context,
            )
            .await?;
        context.check()?;
        return owner.with_connection(|c|{
   if let Some(replay)=admission_replay(c,&configuration,None)?{return Ok(replay)}
   admit(c,&configuration.descriptor,&configuration.credential_reference,configuration.configuration_digest)?;
   Ok(json!({"source_id":configuration.descriptor.binding().source_id(),"enabled":true,"replayed":false}))
  });
    }
    let source = SourceId::new(request.scope())?;
    let configuration = owner.with_connection(|c| configuration(c, &source))?;
    let binding = configuration.descriptor.binding().clone();
    if let ProviderRequest::Reconnect {
        credential_reference,
        name,
        ..
    } = request
    {
        let reference = CredentialReference::new(credential_reference)?;
        let next = factory
            .describe(
                binding.library_id().clone(),
                source,
                name,
                reference,
                context,
            )
            .await?;
        context.check()?;
        return owner.with_connection(|c|{
   if let Some(replay)=admission_replay(c,&next,Some(next.descriptor.binding().source_id()))?{return Ok(replay)}
   reconnect(c,&next.descriptor,&next.credential_reference,next.configuration_digest)?;
   Ok(json!({"source_id":next.descriptor.binding().source_id(),"enabled":true,"replayed":false}))
  });
    }
    match request {
        ProviderRequest::Disable { .. } => {
            owner.with_connection(|c| disable(c, &binding, projection))?;
            return Ok(json!({"disabled":true}));
        }
        ProviderRequest::Forget { .. } => {
            owner.with_connection(|c| forget_materialization(c, &binding, projection))?;
            return Ok(json!({"forgotten":true,"disabled":true}));
        }
        ProviderRequest::Documents { offset, limit, .. } => {
            return owner.with_connection(|c| documents(c, &binding, offset, limit));
        }
        ProviderRequest::Mutations { .. } => {
            return owner.with_connection(|c| {
                revalidate(c, &binding)?;
                let ids = journal::pending_ids(c, &binding)?;
                let rows = ids
                    .into_iter()
                    .map(|id| {
                        journal::load(c, &binding, id)
                            .map(|m| json!({"operation_id":id,"state":m.state}))
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(json!(rows))
            });
        }
        ProviderRequest::BeginEdit { document_id, .. } => {
            return owner.with_connection(|c| {
                editing::begin_edit(c, &configuration.descriptor, document_id)
            });
        }
        ProviderRequest::PrepareSave {
            edit_id, markdown, ..
        } => {
            return owner.with_connection(|c| {
                editing::prepare_save(c, &configuration.descriptor, edit_id, markdown)
            });
        }
        ProviderRequest::InspectSave { operation_id,.. } => {
            return owner.with_connection(|c|editing::inspect_save(c,&binding,operation_id));
        }
        _ => {}
    }
    owner.with_connection(|c| revalidate(c, &binding))?;
    let known = owner.with_connection(|c| {
        if let ProviderRequest::Open { document_id, .. } = &request {
            let key = key_for(c, &binding, *document_id)?;
            Ok(vec![super::metadata_observation::metadata(
                c, &binding, &key,
            )?])
        } else if matches!(request, ProviderRequest::Refresh { .. }) {
            discovery_metadata::read(c, &binding)
        } else if matches!(
            request,
            ProviderRequest::DispatchSave { .. }
                | ProviderRequest::Repair { .. }
                | ProviderRequest::InspectCreate { .. }
        ) {
            if metadata_observation::snapshot(c, &binding)?.partial {
                discovery_metadata::read(c, &binding)
            } else {
                published_metadata(c, &binding, 0, 16384)
            }
        } else {
            published_metadata(c, &binding, 0, 16384)
        }
    })?;
    let authority = Arc::new(RegistryAuthority(owner.clone()));
    let provider = factory
        .open(configuration, authority, known, context)
        .await?;
    if provider.descriptor().binding() != &binding {
        return Err(ProviderError::IdentityMismatch);
    }
    owner.with_connection(|c| capabilities::retain_verified_changes(c, provider.descriptor()))?;
    match request {
        ProviderRequest::BrowseFolders {
            folder_id, cursor, ..
        } => {
            let page = provider
                .folders(&folder_id, cursor.as_ref(), context)
                .await?;
            context.check()?;
            owner.with_connection(|c| revalidate(c, &binding))?;
            if page.folders.len() > context.page_rows() {
                return Err(ProviderError::TooLarge);
            }
            serde_json::to_value(page).map_err(|_| ProviderError::Protocol)
        }
        ProviderRequest::Refresh { .. } => {
            let result = synchronize(&*owner, &*provider, projection, context).await?;
            Ok(json!({"pages":result.pages,"metadata_complete":result.metadata_complete}))
        }
        ProviderRequest::Open { document_id, .. } => {
            let key = owner.with_connection(|c| key_for(c, &binding, document_id))?;
            serde_json::to_value(
                fetch_and_admit(&*owner, &*provider, &key, projection, context).await?,
            )
            .map_err(|_| ProviderError::Protocol)
        }
        ProviderRequest::InspectCreate { operation_id, .. } => {
            creation::inspect_remote(&*owner, &*provider, operation_id, projection, context).await
        }
        ProviderRequest::CreateDocument {
            operation_id,
            filename,
            markdown,
            ..
        } => {
            creation::create(
                &*owner,
                &*provider,
                operation_id,
                filename,
                markdown,
                projection,
                context,
            )
            .await
        }
        ProviderRequest::DispatchSave { operation_id, .. } => serde_json::to_value(
            dispatch_write(&*owner, &*provider, operation_id, projection, context).await?,
        )
        .map_err(|_| ProviderError::Protocol),
        ProviderRequest::Repair { .. } => {
            let result = repair_mutations(&*owner, &*provider, projection, context).await?;
            Ok(
                json!({"recovered_unknown":result.recovered_unknown,"published":result.published,"conflicts":result.conflicts,"pending":result.pending}),
            )
        }
        _ => Err(ProviderError::Invalid),
    }
}

fn admission_replay(
    c: &mut Connection,
    next: &SourceConfiguration,
    source: Option<&SourceId>,
) -> Result<Option<Value>> {
    let Some(receipt) = registry::admission_receipt(c, &next.credential_reference)? else {
        return Ok(None);
    };
    if receipt.configuration_digest != next.configuration_digest
        || source.is_some_and(|source| source != &receipt.source_id)
    {
        return Err(ProviderError::Conflict);
    }
    let current = configuration(c, &receipt.source_id)?;
    if current.descriptor.binding().digest() != receipt.binding
        || current.credential_reference != next.credential_reference
        || current.configuration_digest != receipt.configuration_digest
    {
        // Retired references remain consumed after reconnect or forget. They
        // cannot revive an old generation or manufacture a new source identity.
        return Err(ProviderError::Conflict);
    }
    let enabled: bool = registry::db(c.query_row(
        "SELECT enabled FROM mdreader_provider_registry_sources WHERE identity=?1",
        [registry::identity(current.descriptor.binding())],
        |r| r.get(0),
    ))?;
    Ok(Some(
        json!({"source_id":receipt.source_id,"enabled":enabled,"replayed":true}),
    ))
}

#[cfg(all(test, unix))]
#[path = "../daemon/provider_tests.rs"]
mod daemon_tests;
