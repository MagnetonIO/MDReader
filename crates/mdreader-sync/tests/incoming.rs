use base64::{engine::general_purpose::STANDARD, Engine};
use mdreader_sync::{incoming::*, *};
use serde::Deserialize;
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Vector {
    context: EncryptionContext,
    plaintext_utf8: String,
    envelope_utf8: String,
    blob_sha256: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    source_utf8: String,
    asset_base64: String,
    vectors: Vec<Vector>,
}
fn fixture() -> Fixture {
    serde_json::from_str(include_str!("../fixtures/payload-v1.json")).unwrap()
}
fn scope() -> SyncScope {
    SyncScope::new(
        "a".repeat(64),
        fixture().vectors[0].context.library_id.clone(),
        "A".repeat(43),
    )
    .unwrap()
}
fn key() -> LibraryKey {
    LibraryKey::from_bytes(
        fixture().vectors[0].context.key_id.clone(),
        Zeroizing::new(vec![0; 32]),
    )
    .unwrap()
}
fn root(vector: &Vector) -> RemoteRevision {
    RemoteRevision {
        change: RemoteChange {
            sequence: 1,
            item_id: vector.context.item_id.clone(),
            revision_id: vector.context.revision_id.clone(),
            blob: Some(vector.blob_sha256.clone()),
            deleted: false,
            parents: vec![],
            device: "A".repeat(43),
        },
        envelope_utf8: Some(vector.envelope_utf8.as_bytes().to_vec()),
    }
}
fn resources(f: &Fixture) -> Vec<EncryptedResource<'_>> {
    f.vectors[..2]
        .iter()
        .map(|v| EncryptedResource {
            blob_id: &v.blob_sha256,
            envelope_utf8: v.envelope_utf8.as_bytes(),
        })
        .collect()
}
fn rewrite(vector: &Vector, plaintext: &[u8]) -> RemoteRevision {
    let encrypted = encrypt(&key(), &vector.context, plaintext).unwrap();
    let mut revision = root(vector);
    revision.change.blob = Some(encrypted.blob_id().into());
    revision.envelope_utf8 = Some(encrypted.envelope_utf8().into());
    revision
}

#[test]
fn browser_vectors_reassemble_exact_originals_and_keep_typed_positions() {
    let f = fixture();
    let key = key();
    let scope = scope();
    let prepared = prepare(&scope, &root(&f.vectors[2]), &[&key], &resources(&f)).unwrap();
    assert_eq!(prepared.scope(), &scope);
    assert_eq!(
        prepared.change().revision_id,
        f.vectors[2].context.revision_id
    );
    let IncomingPayload::Content(content) = prepared.payload() else {
        panic!("expected content")
    };
    assert_eq!(content.title(), "Public synthetic π.md");
    assert_eq!(content.markdown(), f.source_utf8.as_bytes());
    assert_eq!(content.sha256(), blob_digest(f.source_utf8.as_bytes()));
    assert_eq!(content.assets().len(), 1);
    let asset = &content.assets()[0];
    assert_eq!(asset.path(), "pixel.png");
    assert_eq!(asset.mime(), "image/png");
    assert_eq!(asset.bytes(), STANDARD.decode(&f.asset_base64).unwrap());
    for vector in &f.vectors[3..] {
        let result = prepare(&scope, &root(vector), &[&key], &[]).unwrap();
        let IncomingPayload::UserState(state) = result.payload() else {
            panic!("expected state")
        };
        assert_eq!(
            codec::encode_user_state(state).unwrap(),
            vector.plaintext_utf8.as_bytes()
        );
    }
}

#[test]
fn missing_extra_tampered_wrong_key_and_cross_context_resources_fail_closed() {
    let f = fixture();
    let key = key();
    let revision = root(&f.vectors[2]);
    assert_eq!(
        dependencies(&scope(), &revision, &[&key]).unwrap(),
        f.vectors[..2]
            .iter()
            .map(|v| v.blob_sha256.clone())
            .collect::<Vec<_>>()
    );
    assert!(matches!(
        prepare(&scope(), &revision, &[&key], &resources(&f)[..1]),
        Err(SyncError::NotReady)
    ));
    let mut duplicate = resources(&f);
    duplicate.push(EncryptedResource {
        blob_id: &f.vectors[0].blob_sha256,
        envelope_utf8: f.vectors[0].envelope_utf8.as_bytes(),
    });
    assert!(prepare(&scope(), &revision, &[&key], &duplicate).is_err());
    let mut tampered = f.vectors[0].envelope_utf8.as_bytes().to_vec();
    tampered[0] ^= 1;
    let mut bad = resources(&f);
    bad[0].envelope_utf8 = &tampered;
    assert!(matches!(
        prepare(&scope(), &revision, &[&key], &bad),
        Err(SyncError::Authentication)
    ));
    let wrong = LibraryKey::from_bytes(key.key_id().into(), Zeroizing::new(vec![1; 32])).unwrap();
    assert!(matches!(
        dependencies(&scope(), &revision, &[&wrong]),
        Err(SyncError::Authentication)
    ));
    assert!(matches!(
        prepare(&scope(), &revision, &[&wrong], &resources(&f)),
        Err(SyncError::Authentication)
    ));
    let other_scope = SyncScope::new(
        "a".repeat(64),
        "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee".into(),
        "A".repeat(43),
    )
    .unwrap();
    assert!(prepare(&other_scope, &revision, &[&key], &resources(&f)).is_err());
    let mut payload: serde_json::Value =
        serde_json::from_str(&f.vectors[2].plaintext_utf8).unwrap();
    payload["source"]["chunks"][0]["itemId"] = "dddddddd-dddd-4ddd-8ddd-dddddddddddd".into();
    let changed = rewrite(&f.vectors[2], &serde_json::to_vec(&payload).unwrap());
    assert!(matches!(
        prepare(&scope(), &changed, &[&key], &resources(&f)),
        Err(SyncError::Authentication)
    ));
}

#[test]
fn authenticated_chunk_metadata_and_full_resource_hash_are_authoritative() {
    let f = fixture();
    let key = key();
    for field in ["documentId", "revisionId", "role", "path", "index", "count"] {
        let mut chunk: serde_json::Value =
            serde_json::from_str(&f.vectors[0].plaintext_utf8).unwrap();
        chunk[field] = match field {
            "documentId" | "revisionId" => "dddddddd-dddd-4ddd-8ddd-dddddddddddd".into(),
            "role" => "asset".into(),
            "path" => "other.png".into(),
            _ => 2.into(),
        };
        let changed = rewrite(&f.vectors[0], &serde_json::to_vec(&chunk).unwrap());
        let mut descriptor: serde_json::Value =
            serde_json::from_str(&f.vectors[2].plaintext_utf8).unwrap();
        descriptor["source"]["chunks"][0]["blob"] = changed.change.blob.clone().unwrap().into();
        let root = rewrite(&f.vectors[2], &serde_json::to_vec(&descriptor).unwrap());
        let bodies = [
            EncryptedResource {
                blob_id: changed.change.blob.as_deref().unwrap(),
                envelope_utf8: changed.envelope_utf8.as_deref().unwrap(),
            },
            resources(&f).remove(1),
        ];
        assert!(
            prepare(&scope(), &root, &[&key], &bodies).is_err(),
            "field {field}"
        );
    }
    let mut descriptor: serde_json::Value =
        serde_json::from_str(&f.vectors[2].plaintext_utf8).unwrap();
    descriptor["source"]["sha256"] = "0".repeat(64).into();
    let changed = rewrite(&f.vectors[2], &serde_json::to_vec(&descriptor).unwrap());
    assert!(matches!(
        prepare(&scope(), &changed, &[&key], &resources(&f)),
        Err(SyncError::Authentication)
    ));
}

#[test]
fn invalid_utf8_and_raster_are_rejected_after_valid_encryption_and_hashes() {
    let f = fixture();
    let key = key();
    for (index, data) in [(0, vec![0xff]), (1, b"<svg>active</svg>".to_vec())] {
        let mut chunk: serde_json::Value =
            serde_json::from_str(&f.vectors[index].plaintext_utf8).unwrap();
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        chunk["data"] = URL_SAFE_NO_PAD.encode(&data).into();
        let changed = rewrite(&f.vectors[index], &serde_json::to_vec(&chunk).unwrap());
        let mut descriptor: serde_json::Value =
            serde_json::from_str(&f.vectors[2].plaintext_utf8).unwrap();
        let resource = if index == 0 {
            &mut descriptor["source"]
        } else {
            &mut descriptor["assets"][0]
        };
        resource["bytes"] = data.len().into();
        resource["sha256"] = blob_digest(&data).into();
        resource["chunks"][0]["bytes"] = data.len().into();
        resource["chunks"][0]["sha256"] = blob_digest(&data).into();
        resource["chunks"][0]["blob"] = changed.change.blob.clone().unwrap().into();
        let root = rewrite(&f.vectors[2], &serde_json::to_vec(&descriptor).unwrap());
        let mut bodies = resources(&f);
        bodies[index] = EncryptedResource {
            blob_id: changed.change.blob.as_deref().unwrap(),
            envelope_utf8: changed.envelope_utf8.as_deref().unwrap(),
        };
        assert!(matches!(
            prepare(&scope(), &root, &[&key], &bodies),
            Err(SyncError::Invalid)
        ));
    }
}

#[test]
fn tombstone_stays_untyped_and_preparation_never_advances_a_cursor() {
    let f = fixture();
    let mut revision = root(&f.vectors[2]);
    revision.change.deleted = true;
    revision.change.blob = None;
    revision.envelope_utf8 = None;
    let prepared = prepare(&scope(), &revision, &[], &[]).unwrap();
    assert!(dependencies(&scope(), &revision, &[]).unwrap().is_empty());
    assert!(matches!(
        prepared.payload(),
        IncomingPayload::OpaqueTombstone
    ));
    let data = tempfile::tempdir().unwrap();
    let mut db = rusqlite::Connection::open(data.path().join("library.sqlite")).unwrap();
    migrate(&mut db).unwrap();
    let mut sync = SyncStore::attach(&mut db, scope()).unwrap();
    let body = PreparedRevision::from_wire(
        &key(),
        &f.vectors[2].context,
        &f.vectors[2].blob_sha256,
        f.vectors[2].envelope_utf8.as_bytes(),
    )
    .unwrap();
    let page = serde_json::to_vec(&serde_json::json!({
        "changes": [root(&f.vectors[2]).change], "cursor": 1,
        "highWater": 1, "hasMore": false, "checkpoint": 0,
    }))
    .unwrap();
    sync.stage_changes(&scope(), 200, 0, &page, &[body])
        .unwrap();
    let cursors = CursorState {
        received: 1,
        applied: 0,
        server_acknowledged: 0,
    };
    let staged = sync.staged(0, 1).unwrap();
    prepare(&scope(), &staged[0], &[&key()], &resources(&f)).unwrap();
    assert_eq!(sync.cursors().unwrap(), cursors);
    drop(sync);
    drop(db);
    let mut db = rusqlite::Connection::open(data.path().join("library.sqlite")).unwrap();
    assert_eq!(
        SyncStore::attach(&mut db, scope())
            .unwrap()
            .cursors()
            .unwrap(),
        cursors
    );
    revision.envelope_utf8 = Some(f.vectors[2].envelope_utf8.as_bytes().to_vec());
    assert!(prepare(&scope(), &revision, &[], &[]).is_err());
}

#[test]
fn multichunk_utf8_boundaries_use_descriptor_order_not_download_order() {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let f = fixture();
    let key = key();
    // The two bytes of π straddle a chunk boundary. Validate UTF-8 after joining.
    let original = format!("{}π\r\n", "a".repeat(codec::CHUNK_BYTES - 1)).into_bytes();
    let mut content = codec::decode_content(f.vectors[2].plaintext_utf8.as_bytes()).unwrap();
    content.assets.clear();
    content.source.bytes = original.len();
    content.source.sha256 = blob_digest(&original);
    content.source.chunks.clear();
    let mut encrypted = Vec::new();
    for (index, bytes) in original.chunks(codec::CHUNK_BYTES).enumerate() {
        let context = EncryptionContext {
            library_id: scope().library_id().into(),
            item_id: uuid::Uuid::new_v4().to_string(),
            revision_id: uuid::Uuid::new_v4().to_string(),
            key_id: key.key_id().into(),
        };
        let chunk = codec::ChunkPayload {
            v: 1,
            kind: "mdreader-chunk".into(),
            document_id: content.document_id.clone(),
            revision_id: content.revision_id.clone(),
            role: "markdown".into(),
            path: None,
            index,
            count: 2,
            data: URL_SAFE_NO_PAD.encode(bytes),
        };
        let body = encrypt(&key, &context, &codec::encode_chunk(&chunk).unwrap()).unwrap();
        content.source.chunks.push(codec::ChunkReference {
            item_id: context.item_id,
            revision_id: context.revision_id,
            blob: body.blob_id().into(),
            bytes: bytes.len(),
            sha256: blob_digest(bytes),
        });
        encrypted.push(body);
    }
    let root = rewrite(&f.vectors[2], &codec::encode_content(&content).unwrap());
    let bodies: Vec<_> = encrypted
        .iter()
        .rev()
        .map(|body| EncryptedResource {
            blob_id: body.blob_id(),
            envelope_utf8: body.envelope_utf8(),
        })
        .collect();
    let result = prepare(&scope(), &root, &[&key], &bodies).unwrap();
    let IncomingPayload::Content(result) = result.payload() else {
        panic!("expected content")
    };
    assert_eq!(result.markdown(), original);
    // A valid descriptor with swapped immutable chunk contexts still fails index authority.
    let first = content.source.chunks[0].clone();
    let second = content.source.chunks[1].clone();
    content.source.chunks[0].item_id = second.item_id;
    content.source.chunks[0].revision_id = second.revision_id;
    content.source.chunks[0].blob = second.blob;
    content.source.chunks[1].item_id = first.item_id;
    content.source.chunks[1].revision_id = first.revision_id;
    content.source.chunks[1].blob = first.blob;
    let changed = rewrite(&f.vectors[2], &codec::encode_content(&content).unwrap());
    assert!(matches!(
        prepare(&scope(), &changed, &[&key], &bodies),
        Err(SyncError::Authentication)
    ));
}

#[test]
fn key_rotation_is_per_envelope_and_input_bounds_reject_unused_dependencies() {
    let f = fixture();
    let old = key();
    let new = LibraryKey::generate().unwrap();
    let mut context = f.vectors[2].context.clone();
    context.key_id = new.key_id().into();
    let encrypted = encrypt(&new, &context, f.vectors[2].plaintext_utf8.as_bytes()).unwrap();
    let mut root = root(&f.vectors[2]);
    root.change.blob = Some(encrypted.blob_id().into());
    root.envelope_utf8 = Some(encrypted.envelope_utf8().into());
    assert!(matches!(
        prepare(&scope(), &root, &[&new], &resources(&f)),
        Err(SyncError::NotReady)
    ));
    assert!(prepare(&scope(), &root, &[&new, &old], &resources(&f)).is_ok());
    assert!(prepare(&scope(), &root, &[&old, &old], &resources(&f)).is_err());
    let mut extra = resources(&f);
    extra.push(EncryptedResource {
        blob_id: &f.vectors[3].blob_sha256,
        envelope_utf8: f.vectors[3].envelope_utf8.as_bytes(),
    });
    assert!(matches!(
        prepare(&scope(), &root, &[&old, &new], &extra),
        Err(SyncError::Invalid)
    ));
    let oversized = vec![0; 512 * 1024 + 1];
    extra = resources(&f);
    extra[0].envelope_utf8 = &oversized;
    assert!(matches!(
        prepare(&scope(), &root, &[&old, &new], &extra),
        Err(SyncError::Bounds)
    ));
    let excess: Vec<_> = (0..=codec::MAX_RESOURCES)
        .map(|_| EncryptedResource {
            blob_id: &f.vectors[0].blob_sha256,
            envelope_utf8: f.vectors[0].envelope_utf8.as_bytes(),
        })
        .collect();
    assert!(matches!(
        prepare(&scope(), &root, &[&old, &new], &excess),
        Err(SyncError::Bounds)
    ));
}
