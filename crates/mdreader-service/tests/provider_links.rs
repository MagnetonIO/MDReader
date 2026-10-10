//! Synthetic registry + real core projection; resolving never opens a provider.
#![cfg(unix)]
use async_trait::async_trait;
use mdreader_credentials::CredentialReference;
use mdreader_provider::*;
use mdreader_service::provider::{self, Owner};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

struct NoNetwork;
#[async_trait]
impl provider::ProviderFactory for NoNetwork {
    async fn describe(
        &self,
        _: LibraryId,
        _: SourceId,
        _: DisplayText,
        _: CredentialReference,
        _: &IoContext,
    ) -> Result<provider::SourceConfiguration> {
        panic!("link resolution must not read credentials or call provider factory")
    }
    async fn open(
        &self,
        _: provider::SourceConfiguration,
        _: Arc<dyn ProviderAuthority>,
        _: Vec<Metadata>,
        _: &IoContext,
    ) -> Result<Arc<dyn DocumentProvider>> {
        panic!("link resolution must not open/enumerate/fetch a provider")
    }
}
struct Fixture {
    _directory: tempfile::TempDir,
    owner: Arc<Mutex<mdreader_core::ReaderStore>>,
    projection: Arc<provider::CoreProjection>,
    descriptor: SourceDescriptor,
    rows: Vec<Metadata>,
    bodies: BTreeMap<String, DocumentBody>,
    receipts: BTreeMap<String, provider::ProjectionReceipt>,
    coordinator: provider::Coordinator,
}
impl Fixture {
    fn new(kind: ProviderKind, files: &[(&str, &str, &str, bool)]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = mdreader_core::ReaderStore::open(directory.path()).unwrap();
        let owner = Arc::new(Mutex::new(store));
        let library = owner.with_connection(provider::library_id).unwrap();
        let scope = if kind == ProviderKind::GoogleDrive {
            SelectedScope::exact(
                files
                    .iter()
                    .map(|(k, ..)| RecordKey::new(*k).unwrap())
                    .collect(),
            )
            .unwrap()
        } else {
            SelectedScope::ScopedRoot {
                root: RecordKey::new("/selected").unwrap(),
                recursive: true,
            }
        };
        let descriptor = SourceDescriptor::new(
            SourceBinding::new(
                library.clone(),
                SourceId::new("synthetic-source").unwrap(),
                AccountBinding::new("private-account").unwrap(),
                uuid::Uuid::new_v4(),
                scope.digest(),
            )
            .unwrap(),
            kind,
            DisplayText::new("Synthetic selected documents").unwrap(),
            scope,
            ProviderCapabilities {
                read: true,
                list: true,
                changes: false,
                assets: false,
                guarded_write: false,
            },
            SourceHealth::Ready,
            Coverage::NeverIndexed,
        )
        .unwrap();
        owner
            .with_connection(|c| {
                provider::admit(
                    c,
                    &descriptor,
                    &OpaqueValue::new("private-reference").unwrap(),
                    [7; 32],
                )
            })
            .unwrap();
        let projection = Arc::new(
            provider::CoreProjection::open(&directory.path().join("provider-cache"), library)
                .unwrap(),
        );
        let mut rows = vec![];
        let mut bodies = BTreeMap::new();
        for (key, path, body, _) in files {
            let version = DocumentVersion {
                provider_revision: OpaqueValue::new(format!("private-version-{key}")).unwrap(),
                content_hash: Some(
                    ContentHash::new(HashAlgorithm::Sha256, body_hash(body.as_bytes()).to_vec())
                        .unwrap(),
                ),
                conditional_write_token: None,
            };
            rows.push(Metadata {
                membership_ancestors: None,
                key: RecordKey::new(*key).unwrap(),
                name: DisplayText::new(path.rsplit('/').next().unwrap()).unwrap(),
                display_path: DisplayText::new(*path).unwrap(),
                media_type: None,
                modified_at_ms: Some(1),
                size_bytes: Some(body.len() as u64),
                version: version.clone(),
            });
            bodies.insert(
                (*key).into(),
                DocumentBody {
                    key: RecordKey::new(*key).unwrap(),
                    version,
                    body: MarkdownBytes::new(body.as_bytes().to_vec()).unwrap(),
                    observed_at_ms: 1,
                },
            );
        }
        let coordinator =
            provider::Coordinator::new(owner.clone(), projection.clone(), Arc::new(NoNetwork))
                .unwrap();
        let mut fixture = Self {
            _directory: directory,
            owner,
            projection,
            descriptor,
            rows,
            bodies,
            receipts: BTreeMap::new(),
            coordinator,
        };
        fixture.publish();
        for (key, _, _, admitted) in files {
            if *admitted {
                let receipt = fixture
                    .owner
                    .with_connection(|c| {
                        provider::admit_body(
                            c,
                            fixture.descriptor.binding(),
                            &fixture.bodies[*key],
                            &*fixture.projection,
                        )
                    })
                    .unwrap();
                fixture.receipts.insert((*key).into(), receipt);
            }
        }
        fixture
    }
    fn publish(&mut self) {
        self.owner
            .with_connection(|c| {
                let snapshot = provider::begin_snapshot(c, self.descriptor.binding(), None)?;
                let page = MetadataPage::new(
                    self.rows.clone(),
                    None,
                    1,
                    Coverage::MetadataCompleteSelected { observed_at_ms: 1 },
                )?;
                provider::stage_page(c, &snapshot, None, &page)?;
                provider::publish_snapshot(c, &snapshot, &*self.projection)
            })
            .unwrap();
    }
    fn link(&self, from: &str, kind: &str, target: &str) -> Result<Value> {
        let receipt = &self.receipts[from];
        let wire = json!({"operation":"resolve_link","reader_source_id":receipt.reader_source_id,"path":receipt.path,"revision":receipt.revision,"kind":kind,"target":target});
        let request = serde_json::from_value::<provider::ProviderRequest>(wire);
        assert!(
            request.is_ok(),
            "selected-provider link protocol must be supported: {request:?}"
        );
        self.coordinator.execute(request.unwrap())
    }
}
#[test]
fn relative_exact_cached_heading_and_selected_uncached_never_fetch() {
    let f = Fixture::new(
        ProviderKind::Dropbox,
        &[
            (
                "private-from",
                "/selected/folder/start.md",
                "# Start\n",
                true,
            ),
            (
                "private-near",
                "/selected/folder/plan.md",
                "# Plan\n\n## Decision\nExact body\n",
                true,
            ),
            (
                "private-far",
                "/selected/elsewhere/plan.md",
                "# Other\n",
                true,
            ),
            (
                "private-uncached",
                "/selected/folder/later.md",
                "# Later\n",
                false,
            ),
            (
                "private-outside",
                "/unselected/hidden.md",
                "# Hidden\n",
                false,
            ),
        ],
    );
    let exact = f.link("private-from", "wikilink", "plan#decision").unwrap();
    assert_eq!(exact["state"], "resolved");
    assert_eq!(
        exact["projection"],
        serde_json::to_value(&f.receipts["private-near"]).unwrap()
    );
    assert_eq!(exact["line"], 3);
    let cached = f
        .owner
        .lock()
        .unwrap()
        .read_document(
            f.receipts["private-near"].reader_source_id.as_str(),
            f.receipts["private-near"].path.as_str(),
            Some(f.receipts["private-near"].revision.as_str()),
        )
        .unwrap();
    assert!(cached.markdown.contains("Exact body"));
    assert_eq!(
        f.link("private-from", "relative", "later.md").unwrap()["state"],
        "selected_uncached"
    );
    assert_eq!(
        f.link("private-from", "wikilink", "hidden").unwrap()["state"],
        "missing"
    );
    assert_eq!(
        f.link("private-from", "relative", "../missing/plan.md")
            .unwrap()["state"],
        "missing"
    );
    assert_eq!(
        f.link("private-from", "relative", "plan.md#unknown")
            .unwrap()["state"],
        "resolved"
    );
    for target in [
        "../../../plan.md",
        "https://example.com/plan.md",
        "%00plan.md",
        "%2e%2e/%2e%2e/%2e%2e/plan.md",
        "file%3a/private",
        "bad\\path",
    ] {
        assert!(
            matches!(
                f.link("private-from", "relative", target),
                Err(ProviderError::Invalid)
            ),
            "{target}"
        );
    }
    assert_eq!(
        f.link("private-from", "relative", &"x".repeat(4097)),
        Err(ProviderError::TooLarge)
    );
    let text = exact.to_string();
    for secret in [
        "private-from",
        "private-near",
        "private-account",
        "private-version",
        "private-reference",
    ] {
        assert!(!text.contains(secret));
    }
}
#[test]
fn provider_fragments_preserve_exact_cached_documents_without_fetch_or_wider_grants() {
    for kind in [ProviderKind::Dropbox, ProviderKind::GoogleDrive] {
        let path = |name: &str| {
            if kind == ProviderKind::Dropbox {
                format!("/selected/folder/{name}")
            } else {
                name.to_owned()
            }
        };
        let from = path("Start.md");
        let plan = path("Plan.md");
        let later = path("Later.md");
        let f = Fixture::new(kind, &[
            ("private-from", &from, "# Start\n", true),
            ("private-plan", &plan, "# Plan\n\n<div id=\"Raw:ID\">Exact raw anchor.</div>\n\n## Repeated\n\n## Repeated\n\nText[^1].\n\n[^1]: Exact footnote.\n", true),
            ("private-later", &later, "# Later\n", false),
        ]);
        for (target, decoded) in [
            ("Plan.md#fn%3A1", "fn:1"),
            ("Plan.md#Raw%3AID", "Raw:ID"),
            ("Plan.md#repeated-1", "repeated-1"),
            ("Plan.md#missing-fragment", "missing-fragment"),
        ] {
            let resolved = f.link("private-from", "relative", target).unwrap();
            assert_eq!(resolved["state"], "resolved");
            assert_eq!(resolved["heading"], decoded);
            assert_eq!(resolved["line"], 1);
            assert_eq!(
                resolved["projection"],
                serde_json::to_value(&f.receipts["private-plan"]).unwrap()
            );
            let body = f
                .owner
                .lock()
                .unwrap()
                .read_document(
                    f.receipts["private-plan"].reader_source_id.as_str(),
                    f.receipts["private-plan"].path.as_str(),
                    Some(f.receipts["private-plan"].revision.as_str()),
                )
                .unwrap();
            assert!(body.markdown.contains("Exact footnote."));
        }
        let alias = f.link("private-from", "wikilink", "Plan#Raw%3AID").unwrap();
        assert_eq!(alias["state"], "resolved");
        assert_eq!(alias["heading"], "Raw:ID");
        let uncached = f
            .link("private-from", "relative", "Later.md#fn%3A1")
            .unwrap();
        assert_eq!(uncached["state"], "selected_uncached");
        assert_eq!(uncached["heading"], "fn:1");
        assert!(!f.receipts.contains_key("private-later"));
        assert_eq!(
            f.link("private-from", "relative", "Plan.md#invalid%0a"),
            Err(ProviderError::Invalid)
        );
    }
}
#[test]
fn flat_drive_duplicate_names_are_ambiguous_and_nested_paths_unsupported() {
    let f = Fixture::new(
        ProviderKind::GoogleDrive,
        &[
            ("private-from", "Start.md", "# Start\n", true),
            ("private-a", "Plan.md", "# A\n", true),
            ("private-b", "Plan.md", "# B\n", false),
        ],
    );
    let result = f.link("private-from", "wikilink", "Plan").unwrap();
    assert_eq!(result["state"], "ambiguous");
    assert_eq!(result["candidates"].as_array().unwrap().len(), 2);
    assert_eq!(
        f.link("private-from", "relative", "folder/Plan.md"),
        Err(ProviderError::Unsupported)
    );
    assert_eq!(
        f.link("private-from", "relative", "%252e%252e/Plan.md"),
        Err(ProviderError::Unsupported)
    );
    let mut owned = vec![(
        "start".to_string(),
        "Start.md".to_string(),
        "# Start\n".to_string(),
        true,
    )];
    owned.extend((0..17).map(|i| {
        (
            format!("private-duplicate-{i}"),
            "Plan.md".into(),
            "# Plan\n".into(),
            false,
        )
    }));
    let files: Vec<_> = owned
        .iter()
        .map(|(key, path, body, admitted)| (key.as_str(), path.as_str(), body.as_str(), *admitted))
        .collect();
    let bounded = Fixture::new(ProviderKind::GoogleDrive, &files);
    assert_eq!(
        bounded.link("start", "wikilink", "Plan"),
        Err(ProviderError::TooLarge)
    );
}
#[test]
fn current_origin_revision_metadata_deletion_and_revocation_fence_navigation() {
    let mut f = Fixture::new(
        ProviderKind::Dropbox,
        &[
            ("private-from", "/selected/start.md", "# Start\n", true),
            ("private-target", "/selected/plan.md", "# Plan\n", true),
        ],
    );
    assert_eq!(
        f.link("private-from", "relative", "plan.md").unwrap()["state"],
        "resolved"
    );
    f.rows.retain(|m| m.key.as_str() != "private-target");
    f.publish();
    assert_eq!(
        f.link("private-from", "relative", "plan.md").unwrap()["state"],
        "missing"
    );
    f.rows[0].version.provider_revision = OpaqueValue::new("new-origin-version").unwrap();
    f.publish();
    assert_eq!(
        f.link("private-from", "relative", "plan.md"),
        Err(ProviderError::Conflict)
    );
    f.owner
        .with_connection(|c| provider::disable(c, f.descriptor.binding(), &*f.projection))
        .unwrap();
    assert_eq!(
        f.link("private-from", "relative", "plan.md"),
        Err(ProviderError::Unauthorized)
    );
    let old = &f.descriptor;
    let rebound = SourceDescriptor::new(
        SourceBinding::new(
            old.binding().library_id().clone(),
            old.binding().source_id().clone(),
            AccountBinding::new("new-private-account").unwrap(),
            uuid::Uuid::new_v4(),
            old.scope().digest(),
        )
        .unwrap(),
        old.kind(),
        old.name().clone(),
        old.scope().clone(),
        old.capabilities(),
        SourceHealth::Ready,
        Coverage::NeverIndexed,
    )
    .unwrap();
    f.owner
        .with_connection(|c| {
            provider::reconnect(
                c,
                &rebound,
                &OpaqueValue::new("new-private-reference").unwrap(),
                [8; 32],
            )
        })
        .unwrap();
    assert!(
        f.owner
            .with_connection(|c| provider::list(c, rebound.binding().library_id()))
            .unwrap()[0]
            .enabled
    );
    assert_eq!(
        f.link("private-from", "relative", "plan.md"),
        Err(ProviderError::Unauthorized),
        "an enabled replacement generation must not revive an old origin"
    );
}
