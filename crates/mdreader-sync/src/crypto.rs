use crate::{Result, SyncError};
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

pub const MAX_ENVELOPE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_PLAINTEXT_BYTES: usize = 1024 * 1024;
const RECOVERY_PREFIX: &str = "mdreader-recovery-v1:";
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncryptionContext {
    pub library_id: String,
    pub item_id: String,
    pub revision_id: String,
    pub key_id: String,
}
impl EncryptionContext {
    pub fn validate(&self) -> Result<()> {
        for id in [
            &self.library_id,
            &self.item_id,
            &self.revision_id,
            &self.key_id,
        ] {
            validate_uuid(id)?;
        }
        Ok(())
    }
}
/// Non-Clone/non-serializable secret; raw bytes and expanded AES state zeroize on drop.
pub struct LibraryKey {
    pub(crate) id: String,
    pub(crate) bytes: Zeroizing<Vec<u8>>,
}
impl std::fmt::Debug for LibraryKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibraryKey")
            .field("key_id", &self.id)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}
impl LibraryKey {
    pub fn from_bytes(id: String, bytes: Zeroizing<Vec<u8>>) -> Result<Self> {
        validate_uuid(&id)?;
        if bytes.len() != 32 {
            return Err(SyncError::Invalid);
        }
        Ok(Self { id, bytes })
    }
    pub fn generate() -> Result<Self> {
        let mut bytes = Zeroizing::new(vec![0; 32]);
        getrandom::fill(bytes.as_mut_slice()).map_err(|_| SyncError::Entropy)?;
        let mut id = [0u8; 16];
        getrandom::fill(&mut id).map_err(|_| SyncError::Entropy)?;
        id[6] = (id[6] & 0x0f) | 0x40;
        id[8] = (id[8] & 0x3f) | 0x80;
        Self::from_bytes(uuid::Uuid::from_bytes(id).to_string(), bytes)
    }
    pub fn key_id(&self) -> &str {
        &self.id
    }
    pub(crate) fn fingerprint(&self) -> String {
        blob_digest(self.bytes.as_slice())
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    v: u8,
    alg: String,
    #[serde(rename = "keyId")]
    key_id: String,
    nonce: String,
    ciphertext: String,
}
/// Construction verifies encryption or authenticates downloaded bytes under the exact context.
/// Plaintext is never retained in this transport object.
#[derive(Debug, Clone)]
pub struct PreparedRevision {
    pub(crate) context: EncryptionContext,
    pub(crate) blob_id: String,
    pub(crate) envelope_utf8: Vec<u8>,
}
impl PreparedRevision {
    pub fn from_wire(
        key: &LibraryKey,
        context: &EncryptionContext,
        expected_blob: &str,
        bytes: &[u8],
    ) -> Result<Self> {
        if bytes.len() > MAX_ENVELOPE_BYTES {
            return Err(SyncError::Bounds);
        }
        validate_digest(expected_blob)?;
        if blob_digest(bytes) != expected_blob {
            return Err(SyncError::Authentication);
        }
        let _plaintext = decrypt(key, context, bytes)?;
        Ok(Self {
            context: context.clone(),
            blob_id: expected_blob.into(),
            envelope_utf8: bytes.into(),
        })
    }
    pub fn context(&self) -> &EncryptionContext {
        &self.context
    }
    pub fn blob_id(&self) -> &str {
        &self.blob_id
    }
    pub fn envelope_utf8(&self) -> &[u8] {
        &self.envelope_utf8
    }
}
pub fn associated_data(context: &EncryptionContext) -> Result<Vec<u8>> {
    context.validate()?;
    serde_json::to_vec(&(
        "mdreader",
        1,
        &context.library_id,
        &context.item_id,
        &context.revision_id,
        &context.key_id,
    ))
    .map_err(|_| SyncError::Invalid)
}
/// Fresh OS-generated 96-bit nonce. Durable service enqueue additionally reserves it
/// by actual key fingerprint in SQLite before encryption, including failed attempts.
pub fn encrypt(
    key: &LibraryKey,
    context: &EncryptionContext,
    plaintext: &[u8],
) -> Result<PreparedRevision> {
    let nonce = random_nonce()?;
    encrypt_with_nonce(key, context, plaintext, nonce)
}
pub(crate) fn random_nonce() -> Result<[u8; 12]> {
    let mut nonce = [0; 12];
    getrandom::fill(&mut nonce).map_err(|_| SyncError::Entropy)?;
    Ok(nonce)
}
pub(crate) fn encrypt_with_nonce(
    key: &LibraryKey,
    context: &EncryptionContext,
    plaintext: &[u8],
    nonce: [u8; 12],
) -> Result<PreparedRevision> {
    if plaintext.len() > MAX_PLAINTEXT_BYTES {
        return Err(SyncError::Bounds);
    }
    let aad = associated_data(context)?;
    if key.id != context.key_id {
        return Err(SyncError::Scope);
    }
    let cipher = Aes256Gcm::new_from_slice(key.bytes.as_slice()).map_err(|_| SyncError::Invalid)?;
    let ciphertext = cipher
        .encrypt(
            &Nonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| SyncError::Authentication)?;
    // Struct field order is the wire contract; blob identity hashes these exact bytes.
    let envelope = Envelope {
        v: 1,
        alg: "A256GCM".into(),
        key_id: key.id.clone(),
        nonce: URL_SAFE_NO_PAD.encode(nonce),
        ciphertext: URL_SAFE_NO_PAD.encode(ciphertext),
    };
    let envelope_utf8 = serde_json::to_vec(&envelope).map_err(|_| SyncError::Invalid)?;
    if envelope_utf8.len() > MAX_ENVELOPE_BYTES {
        return Err(SyncError::Bounds);
    }
    Ok(PreparedRevision {
        context: context.clone(),
        blob_id: blob_digest(&envelope_utf8),
        envelope_utf8,
    })
}
pub fn decrypt(
    key: &LibraryKey,
    context: &EncryptionContext,
    envelope_utf8: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    let aad = associated_data(context)?;
    let (envelope, nonce, ciphertext) = parse_envelope(envelope_utf8)?;
    if key.id != context.key_id || envelope.key_id != context.key_id {
        return Err(SyncError::Scope);
    }
    let cipher = Aes256Gcm::new_from_slice(key.bytes.as_slice()).map_err(|_| SyncError::Invalid)?;
    let bytes = cipher
        .decrypt(
            &Nonce::from(nonce),
            Payload {
                msg: &ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| SyncError::Authentication)?;
    Ok(Zeroizing::new(bytes))
}
pub(crate) fn envelope_key_id(bytes: &[u8]) -> Result<String> {
    parse_envelope(bytes).map(|(envelope, _, _)| envelope.key_id)
}
pub(crate) fn validate_envelope(bytes: &[u8]) -> Result<()> {
    parse_envelope(bytes).map(|_| ())
}
fn parse_envelope(bytes: &[u8]) -> Result<(Envelope, [u8; 12], Vec<u8>)> {
    if bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(SyncError::Bounds);
    }
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| SyncError::Invalid)?;
    if envelope.v != 1 || envelope.alg != "A256GCM" {
        return Err(SyncError::Invalid);
    }
    validate_uuid(&envelope.key_id)?;
    if envelope.nonce.len() != 16
        || envelope.ciphertext.len() > (MAX_PLAINTEXT_BYTES + 16).div_ceil(3) * 4
    {
        return Err(SyncError::Bounds);
    }
    let nonce = decode_canonical(&envelope.nonce)?;
    let nonce: [u8; 12] = nonce.try_into().map_err(|_| SyncError::Invalid)?;
    let ciphertext = decode_canonical(&envelope.ciphertext)?;
    if ciphertext.len() < 16 || ciphertext.len() > MAX_PLAINTEXT_BYTES + 16 {
        return Err(SyncError::Bounds);
    }
    Ok((envelope, nonce, ciphertext))
}
pub(crate) fn decode_canonical(value: &str) -> Result<Vec<u8>> {
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| SyncError::Invalid)?;
    if URL_SAFE_NO_PAD.encode(&decoded) != value {
        return Err(SyncError::Invalid);
    }
    Ok(decoded)
}
pub fn blob_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub(crate) fn validate_uuid(value: &str) -> Result<()> {
    let uuid = uuid::Uuid::parse_str(value).map_err(|_| SyncError::Invalid)?;
    if uuid.get_version_num() != 4
        || uuid.get_variant() != uuid::Variant::RFC4122
        || uuid.to_string() != value
    {
        return Err(SyncError::Invalid);
    }
    Ok(())
}
pub(crate) fn validate_digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(SyncError::Invalid);
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoveryWire {
    v: u8,
    library_id: String,
    key_id: String,
    key: String,
}
impl Drop for RecoveryWire {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}
/// Sensitive raw-key artifact for user-controlled offline export. It is not a
/// password, cloud backup, device enrollment or authenticated pairing protocol.
pub fn export_recovery(key: &LibraryKey, library_id: &str) -> Result<Zeroizing<String>> {
    validate_uuid(library_id)?;
    let wire = RecoveryWire {
        v: 1,
        library_id: library_id.into(),
        key_id: key.id.clone(),
        key: URL_SAFE_NO_PAD.encode(key.bytes.as_slice()),
    };
    let json = Zeroizing::new(serde_json::to_vec(&wire).map_err(|_| SyncError::Invalid)?);
    let encoded = Zeroizing::new(URL_SAFE_NO_PAD.encode(json.as_slice()));
    Ok(Zeroizing::new(format!(
        "{RECOVERY_PREFIX}{}:{}",
        encoded.as_str(),
        blob_digest(json.as_slice())
    )))
}
pub fn import_recovery(secret: &str, expected_library_id: &str) -> Result<LibraryKey> {
    validate_uuid(expected_library_id)?;
    if secret.len() > 1024 {
        return Err(SyncError::Bounds);
    }
    let rest = secret
        .strip_prefix(RECOVERY_PREFIX)
        .ok_or(SyncError::Invalid)?;
    let (encoded, checksum) = rest.split_once(':').ok_or(SyncError::Invalid)?;
    validate_digest(checksum)?;
    let json = Zeroizing::new(decode_canonical(encoded)?);
    if blob_digest(json.as_slice()) != checksum {
        return Err(SyncError::Authentication);
    }
    let wire: RecoveryWire =
        serde_json::from_slice(json.as_slice()).map_err(|_| SyncError::Invalid)?;
    if wire.v != 1 || wire.library_id != expected_library_id {
        return Err(SyncError::Scope);
    }
    LibraryKey::from_bytes(
        wire.key_id.clone(),
        Zeroizing::new(decode_canonical(&wire.key)?),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_known_answer_encryption_and_expanded_key_zeroization_contract() {
        fn assert_zeroizes<T: zeroize::ZeroizeOnDrop>() {}
        assert_zeroizes::<Aes256Gcm>();
        let f: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/a256gcm-v1.json")).unwrap();
        let context: EncryptionContext = serde_json::from_value(f["context"].clone()).unwrap();
        let key = LibraryKey::from_bytes(context.key_id.clone(), Zeroizing::new((0..32).collect()))
            .unwrap();
        let nonce: [u8; 12] = (32u8..44).collect::<Vec<_>>().try_into().unwrap();
        let sealed = encrypt_with_nonce(
            &key,
            &context,
            f["plaintextUtf8"].as_str().unwrap().as_bytes(),
            nonce,
        )
        .unwrap();
        assert_eq!(
            sealed.envelope_utf8(),
            f["envelopeUtf8"].as_str().unwrap().as_bytes()
        );
        assert_eq!(sealed.blob_id(), f["blobSha256"].as_str().unwrap());
    }
}
