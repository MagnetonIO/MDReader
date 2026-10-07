#[test]
fn untrusted_record_keys_enforce_bounds_before_provider_access() {
    assert!(!mdreader_provider::validate_record_key(""));
    assert!(!mdreader_provider::validate_record_key(&"x".repeat(4097)));
    assert!(!mdreader_provider::validate_record_key("a\0b"));
    assert!(mdreader_provider::validate_record_key("id:selected-one"));
}
use mdreader_provider::*;
fn descriptor() -> SourceDescriptor {
    let scope = SelectedScope::exact(vec![RecordKey::new("selected").unwrap()]).unwrap();
    let binding = SourceBinding::new(
        LibraryId::new("library").unwrap(),
        SourceId::new("source").unwrap(),
        AccountBinding::new("account").unwrap(),
        uuid::Uuid::new_v4(),
        scope.digest(),
    )
    .unwrap();
    SourceDescriptor::new(
        binding,
        ProviderKind::GoogleDrive,
        DisplayText::new("My selected files").unwrap(),
        scope,
        ProviderCapabilities {
            read: true,
            list: true,
            ..Default::default()
        },
        SourceHealth::Ready,
        Coverage::NeverIndexed,
    )
    .unwrap()
}
#[test]
fn deserialization_enforces_selection_binding_and_unknown_schema_boundaries() {
    let original = descriptor();
    let json = serde_json::to_value(&original).unwrap();
    let roundtrip: SourceDescriptor = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(roundtrip.binding(), original.binding());
    let mut bad = json.clone();
    bad["scope"]["keys"] = serde_json::json!([]);
    assert!(serde_json::from_value::<SourceDescriptor>(bad).is_err());
    let mut bad = json.clone();
    bad["scope"]["keys"] = serde_json::json!(["other"]);
    assert!(serde_json::from_value::<SourceDescriptor>(bad).is_err());
    let mut bad = json;
    bad["oauthToken"] = serde_json::json!("never permitted");
    assert!(serde_json::from_value::<SourceDescriptor>(bad).is_err());
    assert!(serde_json::from_str::<RecordKey>("\"\"").is_err());
    assert!(serde_json::from_value::<RecordKey>(serde_json::json!("x".repeat(4097))).is_err());
    assert!(serde_json::from_value::<SelectedScope>(
        serde_json::json!({"kind":"exact_documents","keys":["a","a"]})
    )
    .is_err());
    assert!(serde_json::from_value::<ProviderCursor>(serde_json::json!([])).is_err());
    assert!(serde_json::from_value::<MarkdownBytes>(serde_json::json!([255])).is_err());
}
#[test]
fn hashes_revisions_and_conditional_validators_are_separate_and_redacted() {
    let version = DocumentVersion {
        provider_revision: OpaqueValue::new("provider-revision").unwrap(),
        content_hash: Some(ContentHash::new(HashAlgorithm::Md5, vec![1; 16]).unwrap()),
        conditional_write_token: Some(OpaqueValue::new("http-validator").unwrap()),
    };
    let roundtrip: DocumentVersion =
        serde_json::from_str(&serde_json::to_string(&version).unwrap()).unwrap();
    assert_eq!(roundtrip, version);
    assert!(!format!("{version:?}").contains("http-validator"));
    assert!(!format!("{version:?}").contains("provider-revision"));
    assert!(ContentHash::new(HashAlgorithm::Md5, vec![1; 32]).is_err());
    assert!(ContentHash::new(HashAlgorithm::DropboxContentHash, vec![1; 16]).is_err());
    assert!(serde_json::from_value::<ContentHash>(
        serde_json::json!({"algorithm":"md5","value":[]})
    )
    .is_err());
    let scope = SelectedScope::exact(vec![RecordKey::new("a").unwrap()]).unwrap();
    assert_eq!(
        serde_json::from_value::<SelectedScope>(serde_json::to_value(&scope).unwrap()).unwrap(),
        scope
    );
}
#[test]
fn cursor_tamper_wrong_account_source_generation_selection_provider_and_kind_reject() {
    let descriptor = descriptor();
    let binding = descriptor.binding();
    let codec = CursorCodec::new([17; 32]);
    let cursor = codec
        .encode(
            ProviderKind::GoogleDrive,
            binding,
            CursorKind::Changes,
            OpaqueValue::new("provider-token").unwrap(),
        )
        .unwrap();
    assert_eq!(
        codec
            .decode(
                ProviderKind::GoogleDrive,
                binding,
                CursorKind::Changes,
                &cursor
            )
            .unwrap()
            .as_str(),
        "provider-token"
    );
    let mut tamper = cursor.as_bytes().to_vec();
    tamper[10] ^= 1;
    assert!(codec
        .decode(
            ProviderKind::GoogleDrive,
            binding,
            CursorKind::Changes,
            &ProviderCursor::new(tamper).unwrap()
        )
        .is_err());
    assert!(codec
        .decode(ProviderKind::Dropbox, binding, CursorKind::Changes, &cursor)
        .is_err());
    assert!(codec
        .decode(
            ProviderKind::GoogleDrive,
            binding,
            CursorKind::List,
            &cursor
        )
        .is_err());
    assert!(CursorCodec::new([18; 32])
        .decode(
            ProviderKind::GoogleDrive,
            binding,
            CursorKind::Changes,
            &cursor
        )
        .is_err());
    for field in [
        "library_id",
        "source_id",
        "account",
        "generation",
        "selection_digest",
    ] {
        let mut other = serde_json::to_value(binding).unwrap();
        other[field] = match field {
            "generation" => serde_json::json!(uuid::Uuid::new_v4()),
            "selection_digest" => serde_json::json!("0".repeat(64)),
            _ => serde_json::json!("different"),
        };
        let other: SourceBinding = serde_json::from_value(other).unwrap();
        assert!(
            codec
                .decode(
                    ProviderKind::GoogleDrive,
                    &other,
                    CursorKind::Changes,
                    &cursor
                )
                .is_err(),
            "{field}"
        );
    }
}
#[tokio::test]
async fn cancellation_is_level_triggered_and_context_bounds_are_local() {
    let cancel = Cancellation::default();
    let task_cancel = cancel.clone();
    let join = tokio::spawn(async move {
        task_cancel.cancelled().await;
    });
    cancel.cancel();
    join.await.unwrap();
    let context = IoContext::new(
        uuid::Uuid::new_v4(),
        std::time::Instant::now() + std::time::Duration::from_secs(1),
        1,
        1024,
        cancel,
    )
    .unwrap();
    assert_eq!(context.check(), Err(ProviderError::Cancelled));
    assert!(IoContext::new(
        uuid::Uuid::new_v4(),
        std::time::Instant::now(),
        257,
        1024,
        Cancellation::default()
    )
    .is_err());
    let context = IoContext::new(
        uuid::Uuid::new_v4(),
        std::time::Instant::now(),
        1,
        1024,
        Cancellation::default(),
    )
    .unwrap();
    assert_eq!(context.check(), Err(ProviderError::Timeout));
}
#[test]
fn page_coverage_never_claims_complete_when_continuation_remains() {
    let cursor = ProviderCursor::new(vec![1]).unwrap();
    assert!(MetadataPage::new(
        vec![],
        Some(cursor.clone()),
        1,
        Coverage::CompleteSelected { observed_at_ms: 1 }
    )
    .is_err());
    assert!(ChangeOutcome::page(vec![], None, None).is_err());
    assert!(ChangeOutcome::page(vec![], Some(cursor.clone()), Some(cursor)).is_err());
    let outcome =
        ChangeOutcome::page(vec![], None, Some(ProviderCursor::new(vec![1]).unwrap())).unwrap();
    assert!(
        serde_json::from_value::<ChangeOutcome>(serde_json::to_value(outcome).unwrap()).is_ok()
    );
}
