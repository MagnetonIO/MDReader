//! Real SQLite coordinator + native Dropbox factory while discovery is partial.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
struct DropboxHttp {
    uploaded: AtomicBool,
    uploads: AtomicUsize,
}
impl DropboxHttp {
    fn bytes(&self) -> &'static [u8] {
        if self.uploaded.load(Ordering::SeqCst) {
            b"new"
        } else {
            b"old"
        }
    }
    fn file(&self) -> serde_json::Value {
        let bytes = self.bytes();
        let hash = body_hash(&body_hash(bytes));
        let hash: String = hash.iter().map(|v| format!("{v:02x}")).collect();
        serde_json::json!({".tag":"file","id":"id:selected","name":"plan.md","path_lower":"/plans/plan.md","rev":if bytes==b"old" {"abcdef001"}else{"abcdef002"},"content_hash":hash,"size":3,"server_modified":"2026-10-07T12:00:00Z"})
    }
}
#[async_trait::async_trait]
impl Transport for DropboxHttp {
    async fn send(&self, r: HttpRequest) -> Result<Response> {
        if r.url().ends_with("/upload") {
            let args: serde_json::Value =
                serde_json::from_str(&r.headers()["dropbox-api-arg"]).unwrap();
            assert_eq!(args["mode"]["update"], "abcdef001");
            assert_eq!(r.body(), b"new");
            self.uploads.fetch_add(1, Ordering::SeqCst);
            self.uploaded.store(true, Ordering::SeqCst);
            return Err(ProviderError::Unavailable); // accepted upload, lost reply
        } else if r.url().ends_with("/download") {
            return Response::new(
                200,
                BTreeMap::from([(
                    "dropbox-api-result".into(),
                    serde_json::to_string(&self.file()).unwrap(),
                )]),
                self.bytes().to_vec(),
            );
        } else {
            assert!(
                r.url().ends_with("/get_metadata"),
                "unexpected synthetic endpoint"
            );
        }
        Response::new(
            200,
            BTreeMap::new(),
            serde_json::to_vec(&self.file()).unwrap(),
        )
    }
}
#[test]
fn cold_coordinator_native_factory_dispatch_and_readonly_repair_keep_exact_known_membership() {
    native_dispatch(false);
}
#[test]
fn native_dispatch_during_empty_rebuild_uses_retained_published_membership() {
    native_dispatch(true);
}
fn native_dispatch(warm: bool) {
    let path = tempfile::tempdir().unwrap();
    let owner = Arc::new(Mutex::new(
        rusqlite::Connection::open(path.path().join("public.sqlite")).unwrap(),
    ));
    let registration = Registration::dropbox_selected_folder("publicSynthetic".into()).unwrap();
    let grant = ProviderGrant::new(
        &registration,
        AccountBinding::new("public-account").unwrap(),
        NativeSelection::DropboxRoot {
            path: RecordKey::new("/plans").unwrap(),
            recursive: true,
        },
        SecretBundle::new(
            "public-access".into(),
            "public-refresh".into(),
            u64::MAX / 2,
            [9; 32],
        )
        .unwrap(),
    )
    .unwrap()
    .with_verified_scopes(DROPBOX_EDIT_SCOPES)
    .unwrap();
    let reference = CredentialReference::fresh();
    let vault = Arc::new(MemoryVault::default());
    vault.create(&reference, &grant).unwrap();
    let transport = Arc::new(DropboxHttp {
        uploaded: AtomicBool::new(false),
        uploads: AtomicUsize::new(0),
    });
    let factory = Arc::new(
        NativeFactory::new(
            VaultClient::new(vault).unwrap(),
            vec![registration],
            transport.clone(),
        )
        .unwrap(),
    );
    let context = IoContext::new(
        uuid::Uuid::new_v4(),
        std::time::Instant::now() + Duration::from_secs(5),
        16,
        1024 * 1024,
        Cancellation::default(),
    )
    .unwrap();
    let configuration = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(factory.describe(
            owner.with_connection(library_id).unwrap(),
            SourceId::new("public-source").unwrap(),
            DisplayText::new("Public plans").unwrap(),
            reference,
            &context,
        ))
        .unwrap();
    let binding = configuration.descriptor.binding().clone();

    let version = DocumentVersion {
        provider_revision: OpaqueValue::new("abcdef001").unwrap(),
        content_hash: Some(
            ContentHash::new(
                HashAlgorithm::DropboxContentHash,
                body_hash(&body_hash(b"old")).to_vec(),
            )
            .unwrap(),
        ),
        conditional_write_token: Some(OpaqueValue::new("abcdef001").unwrap()),
    };
    let operation = owner
        .with_connection(|c| {
            admit(
                c,
                &configuration.descriptor,
                &configuration.credential_reference,
                configuration.configuration_digest,
            )
            .expect("admit");
            let snapshot = begin_snapshot(
                c,
                &binding,
                Some(&ProviderCursor::new(b"public-before".to_vec())?),
            )
            .expect("snapshot");
            stage_page(
                c,
                &snapshot,
                None,
                &MetadataPage::new(
                    vec![Metadata {
                        membership_ancestors: None,
                        key: RecordKey::new("id:selected")?,
                        name: DisplayText::new("plan.md")?,
                        display_path: DisplayText::new("/plans/plan.md")?,
                        media_type: None,
                        modified_at_ms: Some(1),
                        size_bytes: Some(3),
                        version: version.clone(),
                    }],
                    if warm {
                        None
                    } else {
                        Some(ProviderCursor::new(b"public-continuation".to_vec())?)
                    },
                    1,
                    if warm {
                        Coverage::MetadataCompleteSelected { observed_at_ms: 1 }
                    } else {
                        Coverage::Partial {
                            reason: DisplayText::new("Loading")?,
                        }
                    },
                )
                .expect("page"),
            )
            .expect("stage");
            if warm {
                stage_changes(
                    c,
                    &snapshot,
                    &ProviderCursor::new(b"public-before".to_vec())?,
                    &[],
                    None,
                    Some(&ProviderCursor::new(b"public-after".to_vec())?),
                    2,
                )?;
                publish_snapshot(c, &snapshot, &UnsupportedProjection)?;
                let replacement = begin_snapshot(
                    c,
                    &binding,
                    Some(&ProviderCursor::new(b"public-after".to_vec())?),
                )?;
                stage_page(
                    c,
                    &replacement,
                    None,
                    &MetadataPage::new(
                        vec![],
                        Some(ProviderCursor::new(b"public-continuation".to_vec())?),
                        3,
                        Coverage::Partial {
                            reason: DisplayText::new("Loading")?,
                        },
                    )?,
                )?;
            }
            journal::prepare(
                c,
                &binding,
                &RecordKey::new("id:selected")?,
                &version,
                &MarkdownBytes::new(b"new".to_vec())?,
            )
        })
        .unwrap();
    let coordinator =
        Coordinator::new(owner.clone(), Arc::new(UnsupportedProjection), factory).unwrap();
    // The native connector sends exactly one conditional upload before a lost
    // response. Empty known input previously stopped before sending anything.
    assert_eq!(
        coordinator.execute(ProviderRequest::DispatchSave {
            source_id: binding.source_id().clone(),
            operation_id: operation
        }),
        Err(ProviderError::UnknownWrite {
            operation_id: operation
        })
    );
    assert_eq!(transport.uploads.load(Ordering::SeqCst), 1);
    let repaired = coordinator
        .execute(ProviderRequest::Repair {
            source_id: binding.source_id().clone(),
        })
        .unwrap();
    assert_eq!(repaired["pending"], 1);
    assert_eq!(
        transport.uploads.load(Ordering::SeqCst),
        1,
        "repair must never resend"
    );
    assert_eq!(
        owner
            .with_connection(|c| Ok(journal::load(c, &binding, operation)?.state))
            .unwrap(),
        journal::MutationState::Accepted
    );
    assert_eq!(
        owner
            .with_connection(|c| Ok(resume(c, &binding)?.unwrap().list_cursor))
            .unwrap(),
        Some(ProviderCursor::new(b"public-continuation".to_vec()).unwrap())
    );
}
