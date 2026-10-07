use mdreader_sync::{codec, *};
use serde::Deserialize;
use zeroize::Zeroizing;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Vector {
    name: String,
    context: EncryptionContext,
    aad_utf8: String,
    plaintext_utf8: String,
    envelope_utf8: String,
    blob_sha256: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    key_hex: String,
    vectors: Vec<Vector>,
}
#[test]
fn public_browser_payload_vectors_decrypt_and_reencode_exactly() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("../fixtures/payload-v1.json")).unwrap();
    for vector in fixture.vectors {
        let bytes = (0..fixture.key_hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&fixture.key_hex[i..i + 2], 16).unwrap())
            .collect();
        let key =
            LibraryKey::from_bytes(vector.context.key_id.clone(), Zeroizing::new(bytes)).unwrap();
        assert_eq!(
            associated_data(&vector.context).unwrap(),
            vector.aad_utf8.as_bytes()
        );
        assert_eq!(
            blob_digest(vector.envelope_utf8.as_bytes()),
            vector.blob_sha256
        );
        let plaintext = decrypt(&key, &vector.context, vector.envelope_utf8.as_bytes()).unwrap();
        assert_eq!(plaintext.as_slice(), vector.plaintext_utf8.as_bytes());
        let encoded = match vector.name.as_str() {
            "content" => {
                codec::encode_content(&codec::decode_content(&plaintext).unwrap()).unwrap()
            }
            "markdown-chunk" | "png-chunk" => {
                codec::encode_chunk(&codec::decode_chunk(&plaintext).unwrap()).unwrap()
            }
            _ => codec::encode_user_state(&codec::decode_user_state(&plaintext).unwrap()).unwrap(),
        };
        assert_eq!(encoded, plaintext.as_slice());
    }
}
#[test]
fn unknown_ratio_stale_scope_and_unbounded_payload_shapes_are_rejected() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/payload-v1.json")).unwrap();
    let text = fixture["vectors"][3]["plaintextUtf8"].as_str().unwrap();
    let mut state: serde_json::Value = serde_json::from_str(text).unwrap();
    state["position"] =
        serde_json::json!({"revisionId":"22222222-2222-4222-8222-222222222222","ratio":0.5});
    assert!(codec::decode_user_state(&serde_json::to_vec(&state).unwrap()).is_err());
    let mut content: serde_json::Value =
        serde_json::from_str(fixture["vectors"][2]["plaintextUtf8"].as_str().unwrap()).unwrap();
    content["assets"][0]["path"] = "../outside.png".into();
    assert!(codec::decode_content(&serde_json::to_vec(&content).unwrap()).is_err());
    content["assets"][0]["path"] = "pixel.png".into();
    content["source"]["chunks"][0]["bytes"] = 999999999.into();
    assert!(codec::decode_content(&serde_json::to_vec(&content).unwrap()).is_err());
}
