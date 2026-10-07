use mdreader_sync::*;
use serde::Deserialize;
use zeroize::Zeroizing;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    context: EncryptionContext,
    key_hex: String,
    aad_utf8: String,
    plaintext_utf8: String,
    envelope_utf8: String,
    blob_sha256: String,
}
fn fixture() -> Fixture {
    serde_json::from_str(include_str!("../fixtures/a256gcm-v1.json")).unwrap()
}
fn key(f: &Fixture) -> LibraryKey {
    let bytes = (0..f.key_hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&f.key_hex[i..i + 2], 16).unwrap())
        .collect();
    LibraryKey::from_bytes(f.context.key_id.clone(), Zeroizing::new(bytes)).unwrap()
}
#[test]
fn shared_browser_fixture_aad_and_authenticated_plaintext_match_exactly() {
    let f = fixture();
    assert_eq!(associated_data(&f.context).unwrap(), f.aad_utf8.as_bytes());
    assert_eq!(
        decrypt(&key(&f), &f.context, f.envelope_utf8.as_bytes())
            .unwrap()
            .as_slice(),
        f.plaintext_utf8.as_bytes()
    );
    assert_eq!(blob_digest(f.envelope_utf8.as_bytes()), f.blob_sha256);
}

#[test]
fn fresh_nonce_roundtrips_and_all_context_components_are_authenticated() {
    let f = fixture();
    let key = key(&f);
    let mut nonces = std::collections::HashSet::new();
    for _ in 0..128 {
        let sealed = encrypt(&key, &f.context, f.plaintext_utf8.as_bytes()).unwrap();
        let value: serde_json::Value = serde_json::from_slice(sealed.envelope_utf8()).unwrap();
        assert!(nonces.insert(value["nonce"].as_str().unwrap().to_owned()));
        assert_eq!(blob_digest(sealed.envelope_utf8()), sealed.blob_id());
        assert_eq!(
            decrypt(&key, &f.context, sealed.envelope_utf8())
                .unwrap()
                .as_slice(),
            f.plaintext_utf8.as_bytes()
        );
    }
    for field in ["libraryId", "itemId", "revisionId", "keyId"] {
        let mut value = serde_json::to_value(&f.context).unwrap();
        value[field] = "99999999-9999-4999-8999-999999999999".into();
        let wrong: EncryptionContext = serde_json::from_value(value).unwrap();
        assert!(decrypt(&key, &wrong, f.envelope_utf8.as_bytes()).is_err());
    }
}
#[test]
fn tamper_wrong_key_and_strict_envelope_shapes_are_rejected() {
    let f = fixture();
    let key = key(&f);
    let wrong =
        LibraryKey::from_bytes(f.context.key_id.clone(), Zeroizing::new(vec![99; 32])).unwrap();
    assert!(decrypt(&wrong, &f.context, f.envelope_utf8.as_bytes()).is_err());
    let original: serde_json::Value = serde_json::from_str(&f.envelope_utf8).unwrap();
    for (field, value) in [
        ("v", serde_json::json!(2)),
        ("alg", serde_json::json!("A128GCM")),
        ("nonce", serde_json::json!("ICEiIyQlJicoKSor=")),
        ("ciphertext", serde_json::json!("AA")),
        ("keyId", serde_json::json!("invalid")),
        ("extra", serde_json::json!("rejected")),
    ] {
        let mut modified = original.clone();
        modified[field] = value;
        assert!(
            decrypt(&key, &f.context, &serde_json::to_vec(&modified).unwrap()).is_err(),
            "{field}"
        );
    }
    let mut tampered = original.clone();
    let mut text = tampered["ciphertext"].as_str().unwrap().to_owned();
    text.replace_range(0..1, "A");
    tampered["ciphertext"] = text.into();
    assert!(decrypt(&key, &f.context, &serde_json::to_vec(&tampered).unwrap()).is_err());
    let duplicate = f.envelope_utf8.replace("\"v\":1", "\"v\":1,\"v\":1");
    assert!(decrypt(&key, &f.context, duplicate.as_bytes()).is_err());
    assert!(encrypt(&key, &f.context, &vec![0; MAX_PLAINTEXT_BYTES + 1]).is_err());
    assert!(decrypt(&key, &f.context, &vec![b' '; MAX_ENVELOPE_BYTES + 1]).is_err());
    assert!(LibraryKey::from_bytes(f.context.key_id.clone(), Zeroizing::new(vec![0; 31])).is_err());
    assert!(!format!("{key:?}").contains(&f.key_hex));
}
#[test]
fn recovery_export_is_an_explicit_secret_artifact_bound_to_library() {
    let f = fixture();
    let key = key(&f);
    let secret = export_recovery(&key, &f.context.library_id).unwrap();
    let recovered = import_recovery(&secret, &f.context.library_id).unwrap();
    assert_eq!(
        decrypt(&recovered, &f.context, f.envelope_utf8.as_bytes())
            .unwrap()
            .as_slice(),
        f.plaintext_utf8.as_bytes()
    );
    assert!(import_recovery(&secret, "99999999-9999-4999-8999-999999999999").is_err());
    let mut damaged = secret.to_string();
    damaged.push('x');
    assert!(import_recovery(&damaged, &f.context.library_id).is_err());
}

#[test]
fn oversized_download_is_rejected_before_digest_or_decrypt_work() {
    let f = fixture();
    assert!(matches!(
        PreparedRevision::from_wire(
            &key(&f),
            &f.context,
            &f.blob_sha256,
            &vec![0; MAX_ENVELOPE_BYTES + 1]
        ),
        Err(SyncError::Bounds)
    ));
}
