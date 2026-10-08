use super::*;
use serde_json::{json, Value};
fn fixture() -> Value {
    serde_json::from_str(include_str!("../../../fixtures/pairing-v1.json")).unwrap()
}
fn context(f: &Value) -> PairContext {
    let r = &f["request"];
    PairContext::new(
        r["account"].as_str().unwrap(),
        r["libraryId"].as_str().unwrap(),
        f["approvalPacket"]["senderThumb"].as_str().unwrap(),
        r["pairId"].as_str().unwrap(),
        r["signingThumb"].as_str().unwrap(),
        r["wrappingThumb"].as_str().unwrap(),
        r["nonce"].as_str().unwrap(),
    )
    .unwrap()
}
fn recipient(f: &Value) -> WrappingKey {
    WrappingKey::from_bytes(Zeroizing::new(
        key::decode_fixed(f["recipientPrivateJwk"]["d"].as_str().unwrap()).unwrap(),
    ))
    .unwrap()
}
fn alter_header(jwe: &str, change: impl FnOnce(String) -> String) -> String {
    let mut parts: Vec<String> = jwe.split('.').map(String::from).collect();
    parts[0] = URL_SAFE_NO_PAD.encode(change(
        String::from_utf8(key::decode_bounded(&parts[0], 2048).unwrap()).unwrap(),
    ));
    parts.join(".")
}
#[test]
fn browser_fixture_and_native_profiles_preserve_exact_plaintext() {
    fn zeroizes<T: zeroize::ZeroizeOnDrop>() {}
    zeroizes::<aes_gcm::Aes256Gcm>();
    zeroizes::<aes_kw::KwAes256>();
    let f = fixture();
    let context = context(&f);
    let recipient = recipient(&f);
    let plain = decrypt_pair(f["jwe"].as_str().unwrap(), &recipient, &context).unwrap();
    assert_eq!(
        plain.as_bytes(),
        f["plaintextUtf8"].as_str().unwrap().as_bytes()
    );
    let jwe = encrypt_pair(&plain, &recipient.public().unwrap(), &context).unwrap();
    assert_eq!(
        decrypt_pair(&jwe, &recipient, &context).unwrap().as_bytes(),
        plain.as_bytes()
    );
    let secret = RecoverySecret::generate().unwrap();
    let recovery = encrypt_recovery(&plain, &secret).unwrap();
    assert_eq!(
        decrypt_recovery(&recovery, &secret).unwrap().as_bytes(),
        plain.as_bytes()
    );
    let wrong = RecoverySecret::generate().unwrap();
    assert!(decrypt_recovery(&recovery, &wrong).is_err());
}
#[test]
fn strict_profile_rejects_substitution_duplicate_json_and_wrong_trusted_context() {
    let f = fixture();
    let context = context(&f);
    let recipient = recipient(&f);
    let jwe = f["jwe"].as_str().unwrap();
    let changed = |value: &str| alter_header(jwe, |h| h.replacen("{", value, 1));
    for bad in [
        changed("{\"alg\":\"ECDH-ES+A256KW\","),
        changed("{\"crit\":[],"),
        changed("{\"zip\":\"DEF\","),
        changed("{\"jku\":\"https://attacker.invalid\","),
        alter_header(jwe, |h| h.replace("\"epk\":{", "\"epk\":{\"kty\":\"EC\",")),
        alter_header(jwe, |h| h.replace("P-256", "P-384")),
        alter_header(jwe, |h| h.replace("ECDH-ES+A256KW", "ECDH-ES")),
        format!("{jwe}.extra"),
        format!("{}{}", jwe, "A".repeat(4096)),
    ] {
        assert!(decrypt_pair(&bad, &recipient, &context).is_err());
    }
    let mut bad: Value =
        serde_json::from_slice(&key::decode_bounded(jwe.split('.').next().unwrap(), 2048).unwrap())
            .unwrap();
    bad["epk"]["x"] = json!(URL_SAFE_NO_PAD.encode([0; 32]));
    bad["epk"]["y"] = json!(URL_SAFE_NO_PAD.encode([0; 32]));
    let invalid = alter_header(jwe, |_| serde_json::to_string(&bad).unwrap());
    assert!(decrypt_pair(&invalid, &recipient, &context).is_err());
    let wrong = PairContext {
        apu: context.apu.clone(),
        apv: URL_SAFE_NO_PAD.encode(b"wrong pinned context"),
    };
    assert!(decrypt_pair(jwe, &recipient, &wrong).is_err());
    let wrong_key = WrappingKey::generate().unwrap();
    assert!(decrypt_pair(jwe, &wrong_key, &context).is_err());
    let mut tampered: Vec<String> = jwe.split('.').map(String::from).collect();
    let mut bytes = key::decode_bounded(&tampered[3], 16384).unwrap();
    bytes[0] ^= 1;
    tampered[3] = URL_SAFE_NO_PAD.encode(bytes);
    assert!(decrypt_pair(&tampered.join("."), &recipient, &context).is_err());
    let secret = RecoverySecret::generate().unwrap();
    let plain = JwePlaintext::new(Zeroizing::new(vec![1])).unwrap();
    let recovery = encrypt_recovery(&plain, &secret).unwrap();
    assert!(decrypt_recovery(
        &alter_header(&recovery, |h| h.replacen("{", "{\"alg\":\"dir\",", 1)),
        &secret
    )
    .is_err());
}
#[test]
#[ignore = "Requires companion web locked jose dependency; explicit release conformance gate"]
fn native_pair_and_recovery_interoperate_with_browser_jose() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let f = fixture();
    let context = context(&f);
    let recipient = recipient(&f);
    let plain = JwePlaintext::new(Zeroizing::new(
        f["plaintextUtf8"].as_str().unwrap().as_bytes().to_vec(),
    ))
    .unwrap();
    let secret = RecoverySecret::generate().unwrap();
    let request = json!({"fixture":f,"pair":encrypt_pair(&plain,&recipient.public().unwrap(),&context).unwrap(),"recovery":encrypt_recovery(&plain,&secret).unwrap(),"secret":secret.export().as_str()});
    let mut child = Command::new("node")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fixtures/verify-native-jwe.mjs"
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "synthetic browser JOSE conformance failed"
    );
    let recovery = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        decrypt_recovery(recovery.trim(), &secret)
            .unwrap()
            .as_bytes(),
        plain.as_bytes()
    );
}
