use super::*;
use mdreader_core::{ProviderProjection, ProviderProjectionReceipt, ReaderStore};
use mdreader_provider::{
    AccountBinding, DocumentBody, DocumentVersion, MarkdownBytes, Metadata, OpaqueValue, RecordKey,
    SourceBinding,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;

struct LocalRpc {
    store: Mutex<ReaderStore>,
    reads: AtomicUsize,
}
#[async_trait::async_trait]
impl Rpc for LocalRpc {
    async fn provider(&self, _: ProviderRequest) -> Result<Value> {
        panic!("cached current-head reads must not contact a provider")
    }
    async fn read(&self, p: &ProjectionDto) -> Result<mdreader_core::Document> {
        self.store
            .lock()
            .unwrap()
            .read_document(&p.reader_source_id, &p.path, Some(&p.revision))
            .map_err(core_error)
    }
    async fn read_current(&self, p: &ProjectionDto) -> Result<mdreader_core::Document> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.store
            .lock()
            .unwrap()
            .read_document(&p.reader_source_id, &p.path, None)
            .map_err(core_error)
    }
}
struct Fixture {
    temp: TempDir,
    projection: ProviderProjection,
    binding: SourceBinding,
    rpc: LocalRpc,
}
impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let library = LibraryId::new("public-current-head-library").unwrap();
        let projection =
            ProviderProjection::open(&temp.path().join("cache"), library.clone()).unwrap();
        let binding = SourceBinding::new(
            library,
            SourceId::new("public-current-head-source").unwrap(),
            AccountBinding::new("public-current-head-account").unwrap(),
            Uuid::new_v4(),
            "1".repeat(64),
        )
        .unwrap();
        let store = ReaderStore::open(&temp.path().join("data")).unwrap();
        Self {
            temp,
            projection,
            binding,
            rpc: LocalRpc {
                store: Mutex::new(store),
                reads: AtomicUsize::new(0),
            },
        }
    }
    fn admit(&self, remote_revision: &str, text: &str) -> ProviderProjectionReceipt {
        let key = RecordKey::new("public-current-note").unwrap();
        let version = DocumentVersion {
            provider_revision: OpaqueValue::new(remote_revision).unwrap(),
            content_hash: None,
            conditional_write_token: None,
        };
        let metadata = Metadata {
            membership_ancestors: None,
            key: key.clone(),
            name: DisplayText::new("Public current note.md").unwrap(),
            display_path: DisplayText::new("Public / Current note.md").unwrap(),
            media_type: None,
            modified_at_ms: Some(1000),
            size_bytes: Some(text.len() as u64),
            version: version.clone(),
        };
        let document = DocumentBody {
            key,
            version,
            body: MarkdownBytes::new(text.as_bytes().to_vec()).unwrap(),
            observed_at_ms: 1001,
        };
        self.rpc.store.lock().unwrap().with_library_connection(|c| {
            let tx = c.transaction().unwrap();
            let receipt = self
                .projection
                .admit(&tx, &self.binding, &metadata, &document)
                .unwrap();
            tx.commit().unwrap();
            receipt
        })
    }
    fn projection(receipt: &ProviderProjectionReceipt) -> ProjectionDto {
        ProjectionDto {
            job_id: receipt.job_id.to_string(),
            reader_source_id: receipt.reader_source_id.clone(),
            path: receipt.path.clone(),
            revision: receipt.revision.clone(),
        }
    }
    async fn current(&self, receipt: &ProviderProjectionReceipt) -> Result<NativeProviderResult> {
        current(
            &self.rpc,
            receipt.reader_source_id.clone(),
            receipt.path.clone(),
            receipt.revision.clone(),
        )
        .await
    }
}

#[tokio::test]
async fn stale_exact_receipt_conflicts_but_reopen_reads_admitted_current_head_after_restart() {
    let fixture = Fixture::new();
    let text = "\u{feff}# Public current note\r\n\r\nExact literal bytes.\r\n";
    let old = fixture.admit("public-remote-1", text);
    // Same literal body with a later provider head still has a distinct exact receipt.
    let latest = fixture.admit("public-remote-2", text);
    assert_ne!(old.revision, latest.revision);
    *fixture.rpc.store.lock().unwrap() =
        ReaderStore::open(&fixture.temp.path().join("data")).unwrap();
    assert!(matches!(
        fixture.rpc.read(&Fixture::projection(&old)).await,
        Err(NativeProviderError::Conflict)
    ));
    let before = fixture
        .rpc
        .store
        .lock()
        .unwrap()
        .with_library_connection(|c| c.total_changes());
    let result = fixture.current(&old).await.unwrap();
    let NativeProviderResult::ReadCurrentDocument {
        source_id,
        path,
        revision,
        markdown,
        line_start,
        line_end,
    } = result
    else {
        panic!("unexpected current read reply")
    };
    assert_eq!(source_id, old.reader_source_id);
    assert_eq!(path, old.path);
    assert_eq!(revision, latest.revision);
    assert_eq!(markdown.as_bytes(), text.as_bytes());
    assert_eq!((line_start, line_end), (1, 3));
    assert_eq!(
        before,
        fixture
            .rpc
            .store
            .lock()
            .unwrap()
            .with_library_connection(|c| c.total_changes())
    );
    assert_eq!(
        fixture
            .rpc
            .read(&Fixture::projection(&latest))
            .await
            .unwrap()
            .revision,
        latest.revision
    );
}

#[tokio::test]
async fn current_reopen_refuses_wrong_canonical_refs_and_revoked_source() {
    let fixture = Fixture::new();
    let receipt = fixture.admit("public-remote-1", "# Public note\n");
    for (source, path, revision) in [
        (
            "local-source".into(),
            receipt.path.clone(),
            receipt.revision.clone(),
        ),
        (
            receipt.reader_source_id.clone(),
            "../note.md".into(),
            receipt.revision.clone(),
        ),
        (
            receipt.reader_source_id.clone(),
            receipt.path.clone(),
            "not-a-receipt".into(),
        ),
    ] {
        assert!(matches!(
            current(&fixture.rpc, source, path, revision).await,
            Err(NativeProviderError::Invalid)
        ));
    }
    assert_eq!(fixture.rpc.reads.load(Ordering::SeqCst), 0);
    assert!(matches!(
        current(
            &fixture.rpc,
            receipt.reader_source_id.clone(),
            format!("records/{}.md", "f".repeat(64)),
            receipt.revision.clone()
        )
        .await,
        Err(NativeProviderError::ServiceUnavailable)
    ));
    fixture
        .rpc
        .store
        .lock()
        .unwrap()
        .with_library_connection(|c| {
            let tx = c.transaction().unwrap();
            fixture.projection.revoke(&tx, &fixture.binding).unwrap();
            tx.commit().unwrap();
        });
    assert!(matches!(
        fixture.current(&receipt).await,
        Err(NativeProviderError::ServiceUnavailable)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn current_reopen_preserves_exact_cached_body_hash_guard() {
    let fixture = Fixture::new();
    let receipt = fixture.admit("public-remote-1", "# Public note\n");
    let cache_path: String = fixture.rpc.store.lock().unwrap().with_library_connection(|c| {
        c.query_row("SELECT cache_path FROM core_provider_records WHERE reader_source_id=?1 AND path=?2", rusqlite::params![receipt.reader_source_id, receipt.path], |r| r.get(0)).unwrap()
    });
    std::fs::write(
        fixture.temp.path().join("cache").join(cache_path),
        "different public bytes",
    )
    .unwrap();
    assert!(matches!(
        fixture.current(&receipt).await,
        Err(NativeProviderError::Conflict)
    ));
}
