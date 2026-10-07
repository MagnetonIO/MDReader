use mdreader_provider::*;
use mdreader_service::provider;

fn descriptor_with_library(library: LibraryId) -> SourceDescriptor {
    let scope = SelectedScope::exact(vec![RecordKey::new("selected").unwrap()]).unwrap();
    SourceDescriptor::new(
        SourceBinding::new(
            library,
            SourceId::new("source").unwrap(),
            AccountBinding::new("account").unwrap(),
            uuid::Uuid::new_v4(),
            scope.digest(),
        )
        .unwrap(),
        ProviderKind::GoogleDrive,
        DisplayText::new("Selected plans").unwrap(),
        scope,
        ProviderCapabilities {
            read: true,
            list: true,
            changes: true,
            guarded_write: true,
            assets: false,
        },
        SourceHealth::Ready,
        Coverage::CompleteSelected { observed_at_ms: 99 },
    )
    .unwrap()
}
#[test]
fn explicit_registry_admission_uses_owner_connection() {
    let mut connection = rusqlite::Connection::open_in_memory().unwrap();
    connection.pragma_update(None, "user_version", 73).unwrap();
    let descriptor = descriptor_with_library(provider::library_id(&mut connection).unwrap());
    provider::admit(
        &mut connection,
        &descriptor,
        &OpaqueValue::new("private-keychain-reference").unwrap(),
        [7; 32],
    )
    .unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, u64>(0))
            .unwrap(),
        73
    );
    assert!(matches!(
        provider::list(&mut connection, descriptor.binding().library_id()).unwrap()[0].coverage,
        Coverage::NeverIndexed
    ));
    assert_eq!(
        provider::admit(
            &mut connection,
            &descriptor,
            &OpaqueValue::new("other-ref").unwrap(),
            [8; 32]
        ),
        Err(ProviderError::Conflict)
    );
    let foreign = descriptor_with_library(LibraryId::new("foreign-library").unwrap());
    assert_eq!(
        provider::admit(
            &mut connection,
            &foreign,
            &OpaqueValue::new("foreign-ref").unwrap(),
            [8; 32]
        ),
        Err(ProviderError::Unauthorized)
    );
    // Exact prototype v1 schema had no retired-reference ledger. Migration can
    // preserve the current reference, but cannot invent prior grant history.
    connection
        .execute_batch(
            "DROP TABLE mdreader_provider_registry_admissions;
             ALTER TABLE mdreader_provider_registry_meta DROP COLUMN legacy_reference_fence;
             UPDATE mdreader_provider_registry_meta SET version=1;",
        )
        .unwrap();
    provider::initialize(&mut connection).unwrap();
    let migrated: (u64, bool, u64) = connection
        .query_row(
            "SELECT version,legacy_reference_fence,
             (SELECT count(*) FROM mdreader_provider_registry_admissions)
             FROM mdreader_provider_registry_meta",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(migrated, (2, true, 1));
    let unknown = SourceDescriptor::new(
        SourceBinding::new(
            descriptor.binding().library_id().clone(),
            SourceId::new("new-source").unwrap(),
            descriptor.binding().account().clone(),
            uuid::Uuid::new_v4(),
            descriptor.scope().digest(),
        )
        .unwrap(),
        descriptor.kind(),
        descriptor.name().clone(),
        descriptor.scope().clone(),
        descriptor.capabilities(),
        descriptor.health(),
        descriptor.coverage().clone(),
    )
    .unwrap();
    assert_eq!(
        provider::admit(
            &mut connection,
            &unknown,
            &OpaqueValue::new("unknown-pre-migration-reference").unwrap(),
            [9; 32],
        ),
        Err(ProviderError::Unsupported)
    );
    assert_eq!(
        provider::list(&mut connection, descriptor.binding().library_id())
            .unwrap()
            .len(),
        1
    );
    connection
        .execute("UPDATE mdreader_provider_registry_meta SET version=99", [])
        .unwrap();
    assert_eq!(
        provider::initialize(&mut connection),
        Err(ProviderError::Persistence)
    );
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, u64>(0))
            .unwrap(),
        73
    );
}

fn key() -> RecordKey {
    RecordKey::new("selected").unwrap()
}
fn cursor(value: &str) -> ProviderCursor {
    ProviderCursor::new(value.as_bytes().to_vec()).unwrap()
}
fn version(label: &str, body: &[u8]) -> DocumentVersion {
    DocumentVersion {
        provider_revision: OpaqueValue::new(label).unwrap(),
        content_hash: Some(
            ContentHash::new(HashAlgorithm::Sha256, body_hash(body).to_vec()).unwrap(),
        ),
        conditional_write_token: Some(OpaqueValue::new(format!("http-{label}")).unwrap()),
    }
}
fn row(label: &str, path: &str, body: &[u8]) -> Metadata {
    Metadata {
        key: key(),
        name: DisplayText::new("Plan.md").unwrap(),
        display_path: DisplayText::new(path).unwrap(),
        media_type: None,
        modified_at_ms: Some(1),
        size_bytes: Some(body.len() as u64),
        version: version(label, body),
    }
}
fn setup() -> (rusqlite::Connection, SourceDescriptor) {
    let mut c = rusqlite::Connection::open_in_memory().unwrap();
    let d = descriptor_with_library(provider::library_id(&mut c).unwrap());
    provider::admit(
        &mut c,
        &d,
        &OpaqueValue::new("private-credential-reference").unwrap(),
        [7; 32],
    )
    .unwrap();
    (c, d)
}
#[test]
fn partial_enumeration_cannot_be_promoted_by_change_checkpoint() {
    let (mut c, d) = setup();
    let start = cursor("start");
    let snapshot = provider::begin_snapshot(&mut c, d.binding(), Some(&start)).unwrap();
    let page = MetadataPage::new(
        vec![row("v1", "/Plan.md", b"plan")],
        None,
        1,
        Coverage::Partial {
            reason: DisplayText::new("provider listing incomplete").unwrap(),
        },
    )
    .unwrap();
    provider::stage_page(&mut c, &snapshot, None, &page).unwrap();
    provider::stage_changes(
        &mut c,
        &snapshot,
        &start,
        &[],
        None,
        Some(&cursor("final")),
        2,
    )
    .unwrap();
    provider::publish_snapshot(&mut c, &snapshot, &provider::UnsupportedProjection).unwrap();
    assert!(matches!(
        provider::list(&mut c, d.binding().library_id()).unwrap()[0].coverage,
        Coverage::Partial { .. }
    ));
}

use async_trait::async_trait;
use provider::{Owner, ProjectionAdmission, ProjectionReceipt};
use rusqlite::{params, Transaction};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Default)]
struct Projection {
    fail: AtomicBool,
}
impl Projection {
    fn install(c: &rusqlite::Connection) {
        c.execute_batch("CREATE TABLE fake_jobs(binding BLOB,key TEXT,version TEXT,job TEXT,active INTEGER,PRIMARY KEY(binding,key,version));CREATE TABLE fake_gate(binding BLOB PRIMARY KEY,enabled INTEGER);").unwrap();
    }
}
impl ProjectionAdmission for Projection {
    fn admit(
        &self,
        tx: &Transaction<'_>,
        binding: &SourceBinding,
        _metadata: &Metadata,
        doc: &DocumentBody,
    ) -> Result<ProjectionReceipt> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(ProviderError::Unsupported);
        }
        let version = doc.version.provider_revision.as_str();
        let job: Option<String> = tx
            .query_row(
                "SELECT job FROM fake_jobs WHERE binding=?1 AND key=?2 AND version=?3",
                params![binding.digest().as_slice(), doc.key.as_str(), version],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        let job = job.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        tx.execute(
            "INSERT OR REPLACE INTO fake_gate VALUES(?1,1)",
            [binding.digest().as_slice()],
        )
        .unwrap();
        tx.execute(
            "INSERT OR REPLACE INTO fake_jobs VALUES(?1,?2,?3,?4,1)",
            params![binding.digest().as_slice(), doc.key.as_str(), version, job],
        )
        .unwrap();
        Ok(ProjectionReceipt {
            job_id: uuid::Uuid::parse_str(&job).unwrap(),
            reader_source_id: SourceId::new(format!("provider:{}", hex(&binding.digest())))
                .unwrap(),
            path: RecordKey::new(format!(
                "{}.md",
                hex(&body_hash(doc.key.as_str().as_bytes()))
            ))
            .unwrap(),
            revision: OpaqueValue::new(hex(&body_hash(doc.body.as_bytes()))).unwrap(),
        })
    }
    fn remove(&self, tx: &Transaction<'_>, binding: &SourceBinding, key: &RecordKey) -> Result<()> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(ProviderError::Unsupported);
        }
        tx.execute(
            "UPDATE fake_jobs SET active=0 WHERE binding=?1 AND key=?2",
            params![binding.digest().as_slice(), key.as_str()],
        )
        .unwrap();
        Ok(())
    }
    fn revoke(&self, tx: &Transaction<'_>, binding: &SourceBinding) -> Result<()> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(ProviderError::Unsupported);
        }
        tx.execute(
            "UPDATE fake_gate SET enabled=0 WHERE binding=?1",
            [binding.digest().as_slice()],
        )
        .unwrap();
        tx.execute(
            "UPDATE fake_jobs SET active=0 WHERE binding=?1",
            [binding.digest().as_slice()],
        )
        .unwrap();
        Ok(())
    }
}
use rusqlite::OptionalExtension;
fn publish(c: &mut rusqlite::Connection, d: &SourceDescriptor, row: Metadata, checkpoint: &str) {
    let start = cursor("initial");
    let s = provider::begin_snapshot(c, d.binding(), Some(&start)).unwrap();
    provider::stage_page(
        c,
        &s,
        None,
        &MetadataPage::new(
            vec![row],
            None,
            1,
            Coverage::MetadataCompleteSelected { observed_at_ms: 1 },
        )
        .unwrap(),
    )
    .unwrap();
    provider::stage_changes(c, &s, &start, &[], None, Some(&cursor(checkpoint)), 2).unwrap();
    provider::publish_snapshot(c, &s, &provider::UnsupportedProjection).unwrap();
}
fn document(label: &str, bytes: &[u8]) -> DocumentBody {
    DocumentBody {
        key: key(),
        version: version(label, bytes),
        body: MarkdownBytes::new(bytes.to_vec()).unwrap(),
        observed_at_ms: 3,
    }
}
fn context() -> IoContext {
    IoContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_secs(5),
        256,
        8 * 1024 * 1024,
        Cancellation::default(),
    )
    .unwrap()
}
struct Synthetic {
    descriptor: SourceDescriptor,
    owner: Arc<Mutex<rusqlite::Connection>>,
    pages: Mutex<VecDeque<MetadataPage>>,
    changes: Mutex<VecDeque<ChangeOutcome>>,
    document: Mutex<DocumentBody>,
    writes: AtomicUsize,
    reads: AtomicUsize,
    starts: AtomicUsize,
    lose_receipt: AtomicBool,
    hang: AtomicBool,
}
impl Synthetic {
    fn new(
        descriptor: SourceDescriptor,
        owner: Arc<Mutex<rusqlite::Connection>>,
        doc: DocumentBody,
    ) -> Self {
        Self {
            descriptor,
            owner,
            pages: Mutex::default(),
            changes: Mutex::default(),
            document: Mutex::new(doc),
            writes: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
            starts: AtomicUsize::new(0),
            lose_receipt: AtomicBool::new(false),
            hang: AtomicBool::new(false),
        }
    }
    async fn outside_writer(&self, ctx: &IoContext) -> Result<()> {
        assert!(self.owner.try_lock().is_ok(), "I/O held owner writer lock");
        ctx.check()?;
        if self.hang.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        Ok(())
    }
}
#[async_trait]
impl DocumentProvider for Synthetic {
    fn descriptor(&self) -> &SourceDescriptor {
        &self.descriptor
    }
    async fn list(
        &self,
        _cursor: Option<&ProviderCursor>,
        ctx: &IoContext,
    ) -> Result<MetadataPage> {
        self.outside_writer(ctx).await?;
        self.pages
            .lock()
            .unwrap()
            .pop_front()
            .ok_or(ProviderError::Protocol)
    }
    async fn metadata(&self, key: &RecordKey, ctx: &IoContext) -> Result<Metadata> {
        self.outside_writer(ctx).await?;
        assert_eq!(key, &self.document.lock().unwrap().key);
        let doc = self.document.lock().unwrap();
        Ok(row(
            doc.version.provider_revision.as_str(),
            "/Plan.md",
            doc.body.as_bytes(),
        ))
    }
    async fn read(&self, _key: &RecordKey, ctx: &IoContext) -> Result<DocumentBody> {
        self.outside_writer(ctx).await?;
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.document.lock().unwrap().clone())
    }
    async fn start_changes(&self, ctx: &IoContext) -> Result<ProviderCursor> {
        self.outside_writer(ctx).await?;
        self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(cursor("initial"))
    }
    async fn changes(&self, _cursor: &ProviderCursor, ctx: &IoContext) -> Result<ChangeOutcome> {
        self.outside_writer(ctx).await?;
        self.changes
            .lock()
            .unwrap()
            .pop_front()
            .ok_or(ProviderError::Protocol)
    }
    async fn guarded_write(
        &self,
        permit: journal::DispatchPermit,
        body: &MarkdownBytes,
        ctx: &IoContext,
    ) -> Result<WriteReceipt> {
        self.outside_writer(ctx).await?;
        permit.validate(self.descriptor.binding(), body)?;
        self.writes.fetch_add(1, Ordering::SeqCst);
        let doc = document("written", body.as_bytes());
        *self.document.lock().unwrap() = doc.clone();
        if self.lose_receipt.load(Ordering::SeqCst) {
            return Err(ProviderError::Timeout);
        }
        Ok(WriteReceipt {
            operation_id: permit.operation_id(),
            key: doc.key,
            version: doc.version,
            body_hash: body_hash(body.as_bytes()),
            size_bytes: body.as_bytes().len() as u64,
        })
    }
}

#[test]
fn snapshot_resume_checkpoint_and_path_fence_publish_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("owner.sqlite");
    let mut c = rusqlite::Connection::open(&file).unwrap();
    let d = descriptor_with_library(provider::library_id(&mut c).unwrap());
    provider::admit(
        &mut c,
        &d,
        &OpaqueValue::new("credential-ref").unwrap(),
        [7; 32],
    )
    .unwrap();
    Projection::install(&c);
    let projection = Projection::default();
    publish(&mut c, &d, row("v1", "/old.md", b"old"), "first");
    let doc = document("v1", b"old");
    provider::admit_body(&mut c, d.binding(), &doc, &projection).unwrap();
    let s = provider::begin_delta(&mut c, d.binding()).unwrap();
    provider::stage_changes(
        &mut c,
        &s,
        &cursor("first"),
        &[provider::ResolvedChange::Upsert(row(
            "v2", "/new.md", b"new",
        ))],
        Some(&cursor("continue")),
        None,
        4,
    )
    .unwrap();
    drop(c);
    let mut c = rusqlite::Connection::open(&file).unwrap();
    let resumed = provider::resume(&mut c, d.binding()).unwrap().unwrap();
    assert_eq!(resumed.snapshot.id(), s.id());
    assert_eq!(resumed.change_cursor, Some(cursor("continue")));
    assert_eq!(
        provider::checkpoint(&mut c, d.binding()).unwrap(),
        Some(cursor("first"))
    );
    assert_eq!(
        provider::metadata(&mut c, d.binding(), &key())
            .unwrap()
            .display_path
            .as_str(),
        "/old.md"
    );
    provider::stage_changes(
        &mut c,
        &s,
        &cursor("continue"),
        &[provider::ResolvedChange::Deleted {
            key: key(),
            expected_path: Some(RecordKey::new("/old.md").unwrap()),
        }],
        None,
        Some(&cursor("second")),
        5,
    )
    .unwrap();
    projection.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        provider::publish_snapshot(&mut c, &s, &projection),
        Err(ProviderError::Unsupported)
    );
    assert_eq!(
        provider::checkpoint(&mut c, d.binding()).unwrap(),
        Some(cursor("first"))
    );
    assert!(provider::cached(&mut c, d.binding(), &key())
        .unwrap()
        .unwrap()
        .projection
        .is_some());
    projection.fail.store(false, Ordering::SeqCst);
    provider::publish_snapshot(&mut c, &s, &projection).unwrap();
    assert_eq!(
        provider::metadata(&mut c, d.binding(), &key())
            .unwrap()
            .display_path
            .as_str(),
        "/new.md"
    );
    assert!(provider::cached(&mut c, d.binding(), &key())
        .unwrap()
        .is_none());
    assert_eq!(
        c.query_row("SELECT active FROM fake_jobs", [], |r| r.get::<_, u8>(0))
            .unwrap(),
        0
    );
    // An empty incremental page may legitimately retain its final checkpoint.
    let s = provider::begin_delta(&mut c, d.binding()).unwrap();
    provider::stage_changes(
        &mut c,
        &s,
        &cursor("second"),
        &[],
        None,
        Some(&cursor("second")),
        6,
    )
    .unwrap();
    provider::publish_snapshot(&mut c, &s, &projection).unwrap();
}

#[tokio::test]
async fn resumable_sync_keeps_io_outside_writer_and_has_bounded_deadline() {
    let (mut c, d) = setup();
    let s = provider::begin_snapshot(&mut c, d.binding(), Some(&cursor("initial"))).unwrap();
    provider::stage_page(
        &mut c,
        &s,
        None,
        &MetadataPage::new(
            vec![],
            Some(cursor("list-next")),
            1,
            Coverage::Partial {
                reason: DisplayText::new("continuation").unwrap(),
            },
        )
        .unwrap(),
    )
    .unwrap();
    let owner = Arc::new(Mutex::new(c));
    let p = Synthetic::new(d.clone(), owner.clone(), document("v1", b"plan"));
    p.pages.lock().unwrap().push_back(
        MetadataPage::new(
            vec![row("v1", "/Plan.md", b"plan")],
            None,
            2,
            Coverage::MetadataCompleteSelected { observed_at_ms: 2 },
        )
        .unwrap(),
    );
    p.changes
        .lock()
        .unwrap()
        .push_back(ChangeOutcome::page(vec![], None, Some(cursor("checkpoint"))).unwrap());
    assert!(
        provider::synchronize(
            owner.as_ref(),
            &p,
            &provider::UnsupportedProjection,
            &context()
        )
        .await
        .unwrap()
        .metadata_complete
    );
    assert_eq!(p.starts.load(Ordering::SeqCst), 0);
    assert_eq!(
        owner
            .with_connection(|c| provider::checkpoint(c, d.binding()))
            .unwrap(),
        Some(cursor("checkpoint"))
    );
    p.hang.store(true, Ordering::SeqCst);
    let ctx = IoContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_millis(20),
        256,
        1024,
        Cancellation::default(),
    )
    .unwrap();
    assert!(matches!(
        provider::synchronize(owner.as_ref(), &p, &provider::UnsupportedProjection, &ctx).await,
        Err(ProviderError::Timeout)
    ));
    assert_eq!(
        owner
            .with_connection(|c| provider::checkpoint(c, d.binding()))
            .unwrap(),
        Some(cursor("checkpoint"))
    );
    let cancellation = Cancellation::default();
    cancellation.cancel();
    let ctx = IoContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_secs(5),
        256,
        1024,
        cancellation,
    )
    .unwrap();
    assert!(matches!(
        provider::synchronize(owner.as_ref(), &p, &provider::UnsupportedProjection, &ctx).await,
        Err(ProviderError::Cancelled)
    ));
    p.hang.store(false, Ordering::SeqCst);
    p.changes
        .lock()
        .unwrap()
        .push_back(ChangeOutcome::RebuildRequired);
    assert!(matches!(
        provider::synchronize(
            owner.as_ref(),
            &p,
            &provider::UnsupportedProjection,
            &context()
        )
        .await,
        Err(ProviderError::ResetCursor)
    ));
    assert_eq!(
        owner
            .with_connection(|c| provider::checkpoint(c, d.binding()))
            .unwrap(),
        None
    );
    p.pages.lock().unwrap().push_back(
        MetadataPage::new(
            vec![row("v1", "/Plan.md", b"plan")],
            None,
            3,
            Coverage::MetadataCompleteSelected { observed_at_ms: 3 },
        )
        .unwrap(),
    );
    p.changes
        .lock()
        .unwrap()
        .push_back(ChangeOutcome::page(vec![], None, Some(cursor("rebuilt"))).unwrap());
    assert!(
        provider::synchronize(
            owner.as_ref(),
            &p,
            &provider::UnsupportedProjection,
            &context()
        )
        .await
        .unwrap()
        .metadata_complete
    );
    assert_eq!(p.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cache_only_is_not_admission_and_revocation_fences_account_switch() {
    let (mut c, d) = setup();
    Projection::install(&c);
    publish(&mut c, &d, row("v1", "/Plan.md", b"plan"), "checkpoint");
    let owner = Arc::new(Mutex::new(c));
    let p = Synthetic::new(d.clone(), owner.clone(), document("v1", b"plan"));
    let projection = Projection::default();
    assert_eq!(
        provider::fetch_and_admit(
            owner.as_ref(),
            &p,
            &key(),
            &provider::UnsupportedProjection,
            &context()
        )
        .await,
        Err(ProviderError::Unsupported)
    );
    let cached = owner
        .with_connection(|c| provider::cached(c, d.binding(), &key()))
        .unwrap()
        .unwrap();
    assert!(cached.projection.is_none());
    let receipt = provider::fetch_and_admit(owner.as_ref(), &p, &key(), &projection, &context())
        .await
        .unwrap();
    assert_eq!(
        receipt,
        provider::fetch_and_admit(owner.as_ref(), &p, &key(), &projection, &context())
            .await
            .unwrap()
    );
    owner
        .with_connection(|c| provider::cache_body(c, d.binding(), &document("v1", b"plan")))
        .unwrap();
    assert!(owner
        .with_connection(|c| provider::cached(c, d.binding(), &key()))
        .unwrap()
        .unwrap()
        .projection
        .is_some());
    assert_eq!(
        owner.with_connection(|c| provider::disable(
            c,
            d.binding(),
            &provider::UnsupportedProjection
        )),
        Err(ProviderError::Unsupported)
    );
    owner
        .with_connection(|c| provider::revalidate(c, d.binding()))
        .unwrap();
    owner
        .with_connection(|c| provider::disable(c, d.binding(), &projection))
        .unwrap();
    assert_eq!(
        owner.with_connection(|c| provider::revalidate(c, d.binding())),
        Err(ProviderError::Unauthorized)
    );
    assert_eq!(
        owner
            .lock()
            .unwrap()
            .query_row("SELECT enabled FROM fake_gate", [], |r| r.get::<_, u8>(0))
            .unwrap(),
        0
    );
    let scope = d.scope().clone();
    let next = SourceDescriptor::new(
        SourceBinding::new(
            d.binding().library_id().clone(),
            d.binding().source_id().clone(),
            AccountBinding::new("other-account").unwrap(),
            uuid::Uuid::new_v4(),
            scope.digest(),
        )
        .unwrap(),
        d.kind(),
        d.name().clone(),
        scope,
        d.capabilities(),
        SourceHealth::Ready,
        Coverage::NeverIndexed,
    )
    .unwrap();
    owner
        .with_connection(|c| {
            provider::reconnect(
                c,
                &next,
                &OpaqueValue::new("other-credential-reference")?,
                [8; 32],
            )
        })
        .unwrap();
    assert_eq!(
        owner.with_connection(|c| provider::admit_body(
            c,
            d.binding(),
            &document("v1", b"plan"),
            &projection
        )),
        Err(ProviderError::Unauthorized)
    );
    assert!(matches!(
        owner.with_connection(|c| provider::cached(c, next.binding(), &key())),
        Err(ProviderError::Unavailable)
    ));
}

#[tokio::test]
async fn unknown_write_is_send_once_and_startup_reconciles_without_upload() {
    let (mut c, d) = setup();
    Projection::install(&c);
    publish(&mut c, &d, row("v1", "/Plan.md", b"old"), "checkpoint");
    let body = MarkdownBytes::new(b"desired".to_vec()).unwrap();
    let id = provider::prepare_write(&mut c, d.binding(), &key(), &version("v1", b"old"), &body)
        .unwrap();
    let owner = Arc::new(Mutex::new(c));
    let p = Synthetic::new(d.clone(), owner.clone(), document("v1", b"old"));
    let projection = Projection::default();
    p.lose_receipt.store(true, Ordering::SeqCst);
    assert_eq!(
        provider::dispatch_write(owner.as_ref(), &p, id, &projection, &context()).await,
        Err(ProviderError::UnknownWrite { operation_id: id })
    );
    assert!(matches!(
        provider::dispatch_write(owner.as_ref(), &p, id, &projection, &context()).await,
        Err(ProviderError::UnknownWrite { .. })
    ));
    assert_eq!(p.writes.load(Ordering::SeqCst), 1);
    let result = provider::repair_mutations(owner.as_ref(), &p, &projection, &context())
        .await
        .unwrap();
    assert_eq!(result.published, 1);
    assert_eq!(p.writes.load(Ordering::SeqCst), 1);
    assert_eq!(
        owner
            .with_connection(|c| Ok(journal::load(c, d.binding(), id)?.state))
            .unwrap(),
        journal::MutationState::Published
    );
    let cached = owner
        .with_connection(|c| provider::cached(c, d.binding(), &key()))
        .unwrap()
        .unwrap();
    assert_eq!(cached.document.body, body);
    assert!(cached.projection.is_some());
    let other = owner
        .with_connection(|c| {
            provider::prepare_write(
                c,
                d.binding(),
                &key(),
                &cached.document.version,
                &MarkdownBytes::new(b"second".to_vec())?,
            )
        })
        .unwrap();
    owner
        .with_connection(|c| journal::begin_dispatch(c, d.binding(), other))
        .unwrap();
    *p.document.lock().unwrap() = document("third-party", b"conflict");
    let repair = provider::repair_mutations(owner.as_ref(), &p, &projection, &context())
        .await
        .unwrap();
    assert_eq!(repair.recovered_unknown, 1);
    assert_eq!(repair.conflicts, 1);
    assert_eq!(p.writes.load(Ordering::SeqCst), 1);
    assert!(owner
        .with_connection(|c| journal::conflict_body(c, d.binding(), other))
        .unwrap()
        .is_some());
    owner
        .with_connection(|c| provider::forget_materialization(c, d.binding(), &projection))
        .unwrap();
    assert_eq!(
        owner.with_connection(|c| provider::revalidate(c, d.binding())),
        Err(ProviderError::Unauthorized)
    );
    assert!(owner
        .with_connection(|c| journal::conflict_body(c, d.binding(), other))
        .unwrap()
        .is_some());
}

#[test]
fn provider_child_checkpoint() {
    let Ok(file) = std::env::var("MDREADER_PROVIDER_SYNTHETIC_DB") else {
        return;
    };
    let descriptor: SourceDescriptor =
        serde_json::from_str(&std::env::var("MDREADER_PROVIDER_SYNTHETIC_DESCRIPTOR").unwrap())
            .unwrap();
    let operation =
        uuid::Uuid::parse_str(&std::env::var("MDREADER_PROVIDER_SYNTHETIC_OPERATION").unwrap())
            .unwrap();
    let mut connection = rusqlite::Connection::open(file).unwrap();
    let snapshot = provider::begin_delta(&mut connection, descriptor.binding()).unwrap();
    provider::stage_changes(
        &mut connection,
        &snapshot,
        &cursor("checkpoint"),
        &[],
        Some(&cursor("continued")),
        None,
        3,
    )
    .unwrap();
    journal::begin_dispatch(&mut connection, descriptor.binding(), operation).unwrap();
    use std::io::Write;
    println!("COMMITTED");
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}
#[tokio::test]
async fn killed_process_preserves_staging_and_uncertain_mutation_for_read_only_repair() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("owner.sqlite");
    let mut connection = rusqlite::Connection::open(&file).unwrap();
    let d = descriptor_with_library(provider::library_id(&mut connection).unwrap());
    provider::admit(
        &mut connection,
        &d,
        &OpaqueValue::new("synthetic-credential-reference").unwrap(),
        [7; 32],
    )
    .unwrap();
    Projection::install(&connection);
    publish(
        &mut connection,
        &d,
        row("v1", "/Plan.md", b"old"),
        "checkpoint",
    );
    let operation = provider::prepare_write(
        &mut connection,
        d.binding(),
        &key(),
        &version("v1", b"old"),
        &MarkdownBytes::new(b"desired".to_vec()).unwrap(),
    )
    .unwrap();
    drop(connection);
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "provider_child_checkpoint", "--nocapture"])
        .env("MDREADER_PROVIDER_SYNTHETIC_DB", &file)
        .env(
            "MDREADER_PROVIDER_SYNTHETIC_DESCRIPTOR",
            serde_json::to_string(&d).unwrap(),
        )
        .env(
            "MDREADER_PROVIDER_SYNTHETIC_OPERATION",
            operation.to_string(),
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stdout).lines() {
            if line.unwrap().contains("COMMITTED") {
                send.send(()).unwrap();
                break;
            }
        }
    });
    let committed = receive.recv_timeout(Duration::from_secs(10));
    child.kill().unwrap();
    child.wait().unwrap();
    reader.join().unwrap();
    committed.unwrap();
    let mut connection = rusqlite::Connection::open(&file).unwrap();
    let progress = provider::resume(&mut connection, d.binding())
        .unwrap()
        .unwrap();
    assert_eq!(progress.change_cursor, Some(cursor("continued")));
    assert_eq!(
        provider::checkpoint(&mut connection, d.binding()).unwrap(),
        Some(cursor("checkpoint"))
    );
    assert_eq!(
        journal::load(&mut connection, d.binding(), operation)
            .unwrap()
            .state,
        journal::MutationState::Dispatched
    );
    let owner = Arc::new(Mutex::new(connection));
    let p = Synthetic::new(d.clone(), owner.clone(), document("written", b"desired"));
    let projection = Projection::default();
    let repair = provider::repair_mutations(
        owner.as_ref(),
        &p,
        &provider::UnsupportedProjection,
        &context(),
    )
    .await
    .unwrap();
    assert_eq!(repair.recovered_unknown, 1);
    assert_eq!(repair.pending, 1);
    assert_eq!(p.writes.load(Ordering::SeqCst), 0);
    assert_eq!(
        owner
            .with_connection(|c| Ok(journal::load(c, d.binding(), operation)?.state))
            .unwrap(),
        journal::MutationState::Accepted
    );
    // A second reopen repairs accepted publication without a second remote write.
    drop(p);
    drop(owner);
    let connection = rusqlite::Connection::open(&file).unwrap();
    let owner = Arc::new(Mutex::new(connection));
    let p = Synthetic::new(d.clone(), owner.clone(), document("written", b"desired"));
    assert_eq!(
        provider::repair_mutations(owner.as_ref(), &p, &projection, &context())
            .await
            .unwrap()
            .published,
        1
    );
    assert_eq!(p.writes.load(Ordering::SeqCst), 0);
    // A snapshot staged before local mutation admission cannot overwrite its head.
    owner
        .with_connection(|c| {
            provider::stage_changes(
                c,
                &progress.snapshot,
                &cursor("continued"),
                &[],
                None,
                Some(&cursor("after")),
                4,
            )
        })
        .unwrap();
    assert_eq!(
        owner.with_connection(|c| provider::publish_snapshot(c, &progress.snapshot, &projection)),
        Err(ProviderError::Conflict)
    );
    assert_eq!(
        owner
            .with_connection(|c| provider::metadata(c, d.binding(), &key()))
            .unwrap()
            .version
            .provider_revision
            .as_str(),
        "written"
    );
}

#[cfg(unix)]
#[test]
fn real_core_projection_receipt_indexes_exact_revision_and_atomic_revoke_hides_it() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path().join("data");
    let mut store = mdreader_core::ReaderStore::open(&data).unwrap();
    let library = store.with_library_connection(provider::library_id).unwrap();
    let d = descriptor_with_library(library.clone());
    let projection =
        provider::CoreProjection::open(&directory.path().join("provider-cache"), library).unwrap();
    store
        .with_library_connection(|c| {
            provider::admit(c, &d, &OpaqueValue::new("synthetic-ref")?, [7; 32])?;
            publish(
                c,
                &d,
                row("v1", "/Plan.md", b"# Plan\nsolar acceptance evidence"),
                "checkpoint",
            );
            Result::Ok(())
        })
        .unwrap();
    let receipt = store
        .with_library_connection(|c| {
            provider::admit_body(
                c,
                d.binding(),
                &document("v1", b"# Plan\nsolar acceptance evidence"),
                &projection,
            )
        })
        .unwrap();
    assert_eq!(
        store
            .source_kind(receipt.reader_source_id.as_str())
            .unwrap(),
        mdreader_core::SourceKind::ProviderDerived
    );
    store
        .index_source(receipt.reader_source_id.as_str())
        .unwrap();
    let reader = mdreader_core::ReaderStore::open_readonly(&data).unwrap();
    assert!(!reader
        .search("solar", &[receipt.reader_source_id.as_str().into()], 10)
        .unwrap()
        .is_empty());
    assert_eq!(
        reader
            .read_document(
                receipt.reader_source_id.as_str(),
                receipt.path.as_str(),
                Some(receipt.revision.as_str())
            )
            .unwrap()
            .markdown,
        "# Plan\nsolar acceptance evidence"
    );
    assert_eq!(
        store
            .with_library_connection(|c| provider::list(c, d.binding().library_id()))
            .unwrap()[0]
            .reader_source_id
            .as_ref(),
        Some(&receipt.reader_source_id)
    );
    store
        .with_library_connection(|c| provider::disable(c, d.binding(), &projection))
        .unwrap();
    assert!(matches!(
        reader.search("solar", &[receipt.reader_source_id.as_str().into()], 10),
        Err(mdreader_core::ReaderError::Unavailable)
    ));
    assert!(reader
        .read_document(
            receipt.reader_source_id.as_str(),
            receipt.path.as_str(),
            None
        )
        .is_err());
}
