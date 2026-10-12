//! Real owner, provider journals/projection and a synthetic remote boundary.
use super::*;
use crate::{provider as p, runtime::Runtime, Request, ServiceOptions};
use mdreader_provider::*;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
struct Remote {
    descriptor: SourceDescriptor,
    rows: Mutex<BTreeMap<String, DocumentBody>>,
    writes: AtomicUsize,
    revoke: AtomicBool,
    lose_receipt: AtomicBool,
    owner: Arc<Mutex<ReaderStore>>,
    grant: String,
}
fn body(key: &str, version: &str, bytes: &[u8]) -> DocumentBody {
    DocumentBody {
        key: RecordKey::new(key).unwrap(),
        version: DocumentVersion {
            provider_revision: OpaqueValue::new(version).unwrap(),
            conditional_write_token: Some(OpaqueValue::new(version).unwrap()),
            content_hash: Some(
                ContentHash::new(HashAlgorithm::Sha256, body_hash(bytes).to_vec()).unwrap(),
            ),
        },
        body: MarkdownBytes::new(bytes.to_vec()).unwrap(),
        observed_at_ms: 10,
    }
}
fn metadata(doc: &DocumentBody) -> Metadata {
    Metadata {
        membership_ancestors: None,
        key: doc.key.clone(),
        name: DisplayText::new(doc.key.as_str().trim_start_matches('/')).unwrap(),
        display_path: DisplayText::new(doc.key.as_str()).unwrap(),
        media_type: Some(DisplayText::new("text/markdown").unwrap()),
        modified_at_ms: Some(10),
        size_bytes: Some(doc.body.as_bytes().len() as u64),
        version: doc.version.clone(),
    }
}
#[async_trait::async_trait]
impl DocumentProvider for Remote {
    fn descriptor(&self) -> &SourceDescriptor {
        &self.descriptor
    }
    async fn list(
        &self,
        _: Option<&ProviderCursor>,
        _: &IoContext,
    ) -> mdreader_provider::Result<MetadataPage> {
        MetadataPage::new(
            self.rows.lock().unwrap().values().map(metadata).collect(),
            None,
            10,
            Coverage::MetadataCompleteSelected { observed_at_ms: 10 },
        )
    }
    async fn read(
        &self,
        key: &RecordKey,
        _: &IoContext,
    ) -> mdreader_provider::Result<DocumentBody> {
        self.rows
            .lock()
            .unwrap()
            .get(key.as_str())
            .cloned()
            .ok_or(ProviderError::NotFound)
    }
    async fn metadata(
        &self,
        key: &RecordKey,
        ctx: &IoContext,
    ) -> mdreader_provider::Result<Metadata> {
        Ok(metadata(&self.read(key, ctx).await?))
    }
    async fn start_changes(&self, _: &IoContext) -> mdreader_provider::Result<ProviderCursor> {
        ProviderCursor::new(b"public-checkpoint".to_vec())
    }
    async fn changes(
        &self,
        cursor: &ProviderCursor,
        _: &IoContext,
    ) -> mdreader_provider::Result<ChangeOutcome> {
        ChangeOutcome::page(vec![], None, Some(cursor.clone()))
    }
    async fn guarded_create(
        &self,
        permit: creation::CreatePermit,
        bytes: &MarkdownBytes,
        _: &IoContext,
    ) -> mdreader_provider::Result<Metadata> {
        permit.validate(self.descriptor.binding(), bytes)?;
        let key = format!("/{}", permit.filename());
        let mut rows = self.rows.lock().unwrap();
        if rows.contains_key(&key) {
            return Err(ProviderError::Conflict);
        }
        let doc = body(&key, "created", bytes.as_bytes());
        rows.insert(key, doc.clone());
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(metadata(&doc))
    }
    async fn guarded_write(
        &self,
        permit: journal::DispatchPermit,
        bytes: &MarkdownBytes,
        _: &IoContext,
    ) -> mdreader_provider::Result<WriteReceipt> {
        permit.validate(self.descriptor.binding(), bytes)?;
        let mut rows = self.rows.lock().unwrap();
        let previous = rows
            .get(permit.key().as_str())
            .ok_or(ProviderError::NotFound)?;
        if previous.version != *permit.expected() {
            return Err(ProviderError::Conflict);
        }
        let count = self.writes.fetch_add(1, Ordering::SeqCst);
        let doc = body(
            permit.key().as_str(),
            &format!("written-{count}"),
            bytes.as_bytes(),
        );
        rows.insert(permit.key().as_str().into(), doc.clone());
        drop(rows);
        if self.revoke.swap(false, Ordering::SeqCst) {
            permissions::set(
                &mut self.owner.lock().unwrap(),
                &self.grant,
                false,
                &[],
                &[],
                &[],
            )
            .unwrap();
        }
        if self.lose_receipt.swap(false, Ordering::SeqCst) {
            return Err(ProviderError::Timeout);
        }
        Ok(WriteReceipt {
            operation_id: permit.operation_id(),
            key: doc.key,
            version: doc.version,
            body_hash: body_hash(bytes.as_bytes()),
            size_bytes: bytes.as_bytes().len() as u64,
        })
    }
}
struct Factory(Arc<Remote>);
#[async_trait::async_trait]
impl p::ProviderFactory for Factory {
    async fn describe(
        &self,
        _: LibraryId,
        _: SourceId,
        _: DisplayText,
        _: mdreader_credentials::CredentialReference,
        _: &IoContext,
    ) -> mdreader_provider::Result<p::SourceConfiguration> {
        Err(ProviderError::Unsupported)
    }
    async fn open(
        &self,
        configuration: p::SourceConfiguration,
        _: Arc<dyn ProviderAuthority>,
        _: Vec<Metadata>,
        _: &IoContext,
    ) -> mdreader_provider::Result<Arc<dyn DocumentProvider>> {
        if configuration.descriptor.binding() != self.0.descriptor.binding() {
            return Err(ProviderError::IdentityMismatch);
        }
        Ok(self.0.clone())
    }
}
fn setup() -> (
    tempfile::TempDir,
    Runtime,
    Arc<Remote>,
    String,
    p::ProjectionReceipt,
) {
    let (temp, mut store, _, grant) = fixture();
    let library = store.with_library_connection(p::library_id).unwrap();
    let scope = SelectedScope::ScopedRoot {
        root: RecordKey::new("/").unwrap(),
        recursive: true,
    };
    let descriptor = SourceDescriptor::new(
        SourceBinding::new(
            library.clone(),
            SourceId::new("public-provider").unwrap(),
            AccountBinding::new("public-account").unwrap(),
            uuid::Uuid::new_v4(),
            scope.digest(),
        )
        .unwrap(),
        ProviderKind::Dropbox,
        DisplayText::new("Public fixture").unwrap(),
        scope,
        ProviderCapabilities {
            read: true,
            list: true,
            changes: true,
            guarded_write: true,
            assets: false,
        },
        SourceHealth::Ready,
        Coverage::CompleteSelected { observed_at_ms: 1 },
    )
    .unwrap();
    let doc = body("/Fixture.md", "original", b"old");
    let projection =
        p::CoreProjection::open(&temp.path().join("reader/provider-cache"), library).unwrap();
    let receipt = store
        .with_library_connection(|db| -> mdreader_provider::Result<_> {
            p::admit(
                db,
                &descriptor,
                &OpaqueValue::new("public-reference").unwrap(),
                [7; 32],
            )?;
            let cursor = ProviderCursor::new(b"public-before".to_vec())?;
            let snapshot = p::begin_snapshot(db, descriptor.binding(), Some(&cursor))?;
            p::stage_page(
                db,
                &snapshot,
                None,
                &MetadataPage::new(
                    vec![metadata(&doc)],
                    None,
                    1,
                    Coverage::MetadataCompleteSelected { observed_at_ms: 1 },
                )?,
            )?;
            p::stage_changes(db, &snapshot, &cursor, &[], None, Some(&cursor), 2)?;
            p::publish_snapshot(db, &snapshot, &projection)?;
            p::admit_body(db, descriptor.binding(), &doc, &projection)
        })
        .unwrap();
    permissions::set(
        &mut store,
        &grant,
        true,
        &[],
        &[receipt.reader_source_id.as_str().into()],
        &[receipt.reader_source_id.as_str().into()],
    )
    .unwrap();
    let owner = Arc::new(Mutex::new(store));
    let remote = Arc::new(Remote {
        descriptor,
        rows: Mutex::new(BTreeMap::from([("/Fixture.md".into(), doc)])),
        writes: AtomicUsize::new(0),
        revoke: AtomicBool::new(false),
        lose_receipt: AtomicBool::new(false),
        owner: owner.clone(),
        grant: grant.clone(),
    });
    let runtime = Runtime::new_with_provider(
        owner,
        temp.path().join("reader/models"),
        ServiceOptions {
            disable_model: true,
        },
        Some(Arc::new(Factory(remote.clone()))),
    )
    .unwrap();
    (temp, runtime, remote, grant, receipt)
}
fn call(
    runtime: &Runtime,
    id: &str,
    request: AgentRequest,
) -> mdreader_core::Result<serde_json::Value> {
    runtime.execute(Request::AgentCall {
        grant_id: id.into(),
        request,
    })
}
#[test]
fn provider_create_update_exact_replay_and_revocation_before_publication() {
    let (_temp, runtime, remote, grant, receipt) = setup();
    let operation_id = uuid::Uuid::new_v4().to_string();
    let create = AgentRequest::CreateDocument {
        source_id: receipt.reader_source_id.as_str().into(),
        path: "Created.md".into(),
        operation_id,
        markdown: "# Created".into(),
    };
    let created = call(&runtime, &grant, create.clone()).unwrap();
    assert_eq!(created["state"], "saved");
    assert_eq!(call(&runtime, &grant, create).unwrap(), created);
    assert_eq!(remote.writes.load(Ordering::SeqCst), 1);
    let update = AgentRequest::UpdateDocument {
        source_id: receipt.reader_source_id.as_str().into(),
        path: receipt.path.as_str().into(),
        operation_id: uuid::Uuid::new_v4().to_string(),
        expected_revision: receipt.revision.as_str().into(),
        markdown: "new".into(),
    };
    let saved = call(&runtime, &grant, update.clone()).unwrap();
    assert_eq!(saved["state"], "saved");
    assert_eq!(call(&runtime, &grant, update).unwrap(), saved);
    assert_eq!(remote.writes.load(Ordering::SeqCst), 2);
    let projection: &serde_json::Value = &saved["document"];
    let operation_id = uuid::Uuid::new_v4().to_string();
    remote.revoke.store(true, Ordering::SeqCst);
    let pending = call(
        &runtime,
        &grant,
        AgentRequest::UpdateDocument {
            source_id: projection["reader_source_id"].as_str().unwrap().into(),
            path: projection["path"].as_str().unwrap().into(),
            operation_id: operation_id.clone(),
            expected_revision: projection["revision"].as_str().unwrap().into(),
            markdown: "revoked remotely accepted".into(),
        },
    )
    .unwrap();
    assert_eq!(pending["state"], "pending");
    assert!(call(
        &runtime,
        &grant,
        AgentRequest::InspectOperation { operation_id }
    )
    .is_err());
    let mut owner = remote.owner.lock().unwrap();
    let cached = owner
        .with_library_connection(|db| {
            p::cached(
                db,
                remote.descriptor.binding(),
                &RecordKey::new("/Fixture.md").unwrap(),
            )
        })
        .unwrap()
        .unwrap();
    assert_eq!(
        cached.document.body.as_bytes(),
        b"new",
        "revocation after remote acceptance must not publish the changed body into MDReader"
    );
}
#[test]
fn provider_lost_response_is_reconciled_by_inspect_without_another_write() {
    let (_temp, runtime, remote, grant, receipt) = setup();
    remote.lose_receipt.store(true, Ordering::SeqCst);
    let operation_id = uuid::Uuid::new_v4().to_string();
    let update = AgentRequest::UpdateDocument {
        source_id: receipt.reader_source_id.as_str().into(),
        path: receipt.path.as_str().into(),
        operation_id: operation_id.clone(),
        expected_revision: receipt.revision.as_str().into(),
        markdown: "# Accepted, response lost".into(),
    };
    let pending = call(&runtime, &grant, update.clone()).unwrap();
    assert_eq!(pending["state"], "pending");
    let saved = call(
        &runtime,
        &grant,
        AgentRequest::InspectOperation { operation_id },
    )
    .unwrap();
    assert_eq!(saved["state"], "saved");
    assert_eq!(call(&runtime, &grant, update).unwrap(), saved);
    assert_eq!(remote.writes.load(Ordering::SeqCst), 1);
}
