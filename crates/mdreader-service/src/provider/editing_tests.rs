use super::*;
struct Projection;
impl ProjectionAdmission for Projection {
    fn admit(
        &self,
        _: &Transaction<'_>,
        b: &SourceBinding,
        m: &Metadata,
        d: &DocumentBody,
    ) -> Result<ProjectionReceipt> {
        Ok(ProjectionReceipt {
            job_id: Uuid::new_v4(),
            reader_source_id: SourceId::new(format!("provider:{}", hex(&b.digest())))?,
            path: RecordKey::new(format!("records/{}", m.key.as_str()))?,
            revision: OpaqueValue::new(hex(&body_hash(d.body.as_bytes())))?,
        })
    }
}
#[test]
fn exact_cold_edit_requires_matching_admitted_receipt_and_retains_native_version_ticket() {
    cold_edit(true);
}
#[test]
fn confirmed_creation_without_conditional_validator_does_not_grant_editing() {
    cold_edit(false);
}
fn cold_edit(has_validator: bool) {
    let mut c = Connection::open_in_memory().unwrap();
    super::super::initialize_client(&mut c).unwrap();
    let scope = SelectedScope::exact(vec![RecordKey::new("selected").unwrap()]).unwrap();
    let descriptor = SourceDescriptor::new(
        SourceBinding::new(
            library_id(&mut c).unwrap(),
            SourceId::new("source").unwrap(),
            AccountBinding::new("public-account").unwrap(),
            Uuid::new_v4(),
            scope.digest(),
        )
        .unwrap(),
        ProviderKind::GoogleDrive,
        DisplayText::new("Public").unwrap(),
        scope,
        ProviderCapabilities {
            read: true,
            list: true,
            changes: false,
            assets: false,
            guarded_write: true,
        },
        SourceHealth::Ready,
        Coverage::NeverIndexed,
    )
    .unwrap();
    admit(
        &mut c,
        &descriptor,
        &OpaqueValue::new("public-reference").unwrap(),
        [7; 32],
    )
    .unwrap();
    let b = descriptor.binding();
    let version = DocumentVersion {
        provider_revision: OpaqueValue::new("1").unwrap(),
        content_hash: None,
        conditional_write_token: if has_validator {
            Some(OpaqueValue::new("\"etag\"").unwrap())
        } else {
            None
        },
    };
    let row = Metadata {
        key: RecordKey::new("selected").unwrap(),
        name: DisplayText::new("Plan.md").unwrap(),
        display_path: DisplayText::new("Plan.md").unwrap(),
        media_type: None,
        modified_at_ms: None,
        size_bytes: Some(3),
        membership_ancestors: None,
        version: version.clone(),
    };
    let snapshot = begin_snapshot(&mut c, b, None).unwrap();
    stage_page(
        &mut c,
        &snapshot,
        None,
        &MetadataPage::new(
            vec![row.clone()],
            Some(ProviderCursor::new(b"next".to_vec()).unwrap()),
            1,
            Coverage::NeverIndexed,
        )
        .unwrap(),
    )
    .unwrap();
    let doc = DocumentBody {
        key: row.key.clone(),
        version: version.clone(),
        body: MarkdownBytes::new(b"old".to_vec()).unwrap(),
        observed_at_ms: 1,
    };
    cache_body(&mut c, b, &doc).unwrap();
    let source = SourceId::new(format!("provider:{}", hex(&b.digest()))).unwrap();
    let path = RecordKey::new("records/selected").unwrap();
    let revision = OpaqueValue::new(hex(&body_hash(b"old"))).unwrap();
    assert!(begin_edit_exact(&mut c, &source, &path, &revision).is_err());
    let receipt = admit_body(&mut c, b, &doc, &Projection).unwrap();
    assert!(begin_edit_exact(&mut c, &source, &path, &OpaqueValue::new("wrong").unwrap()).is_err());
    if !has_validator {
        assert_eq!(
            begin_edit_exact(&mut c, &source, &path, &revision),
            Err(ProviderError::Unsupported)
        );
        return;
    }
    let edit = begin_edit_exact(&mut c, &source, &path, &revision).unwrap();
    assert_eq!(edit["projection"], json!(receipt));
    let id: Uuid = serde_json::from_value(edit["edit_id"].clone()).unwrap();
    prepare_save(&mut c, &descriptor, id, "new".into()).unwrap();
    assert_eq!(journal::load(&mut c, b, id).unwrap().expected, version);
    assert!(metadata(&mut c, b, &row.key).is_err());
}
