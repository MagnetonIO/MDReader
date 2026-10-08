//! Strict browser-compatible RFC 7516/7518 profiles. No token/custody/product RPC.
mod compact;
mod key;
use super::{AuthorityError, AuthorityResult};
use crate::crypto::{validate_digest, validate_uuid};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
pub use key::{PublicJwk, WrappingKey};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// Native-only plaintext capability. Never Debug/Serialize or renderer data.
pub struct JwePlaintext(Zeroizing<Vec<u8>>);
impl JwePlaintext {
    pub fn new(bytes: Zeroizing<Vec<u8>>) -> AuthorityResult<Self> {
        if bytes.is_empty() || bytes.len() > 16 * 1024 {
            return Err(AuthorityError::Bounds);
        }
        Ok(Self(bytes))
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}
/// Independent, randomly generated 256-bit recovery secret. Never a password.
pub struct RecoverySecret(Zeroizing<[u8; 32]>);
impl RecoverySecret {
    pub fn generate() -> AuthorityResult<Self> {
        let mut bytes = Zeroizing::new([0; 32]);
        getrandom::fill(bytes.as_mut()).map_err(|_| AuthorityError::Custody)?;
        Ok(Self(bytes))
    }
    pub fn from_bytes(bytes: Zeroizing<[u8; 32]>) -> Self {
        Self(bytes)
    }
    /// Only a native explicit export owner may handle this buffer. No ordinary DTO.
    pub fn export(&self) -> Zeroizing<String> {
        Zeroizing::new(URL_SAFE_NO_PAD.encode(self.0.as_slice()))
    }
}
/// Constructed from locally pinned request + user-verified sender, never JWE headers.
pub struct PairContext {
    apu: String,
    apv: String,
}
impl PairContext {
    pub fn new(
        account: &str,
        library: &str,
        sender: &str,
        pair: &str,
        signing: &str,
        wrapping: &str,
        nonce: &str,
    ) -> AuthorityResult<Self> {
        validate_digest(account).map_err(|_| AuthorityError::Scope)?;
        for id in [library, pair] {
            validate_uuid(id).map_err(|_| AuthorityError::Scope)?;
        }
        for thumb in [sender, signing, wrapping, nonce] {
            key::decode_fixed::<32>(thumb)?;
        }
        if sender == signing || signing == wrapping {
            return Err(AuthorityError::Scope);
        }
        Ok(Self {
            apu: URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&("mdreader-pair", 1, account, library, sender))
                    .map_err(|_| AuthorityError::Scope)?,
            ),
            apv: URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&(pair, signing, wrapping, nonce))
                    .map_err(|_| AuthorityError::Scope)?,
            ),
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PairHeader {
    alg: String,
    enc: String,
    typ: String,
    epk: PublicJwk,
    apu: String,
    apv: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryHeader {
    alg: String,
    enc: String,
    typ: String,
}

pub fn encrypt_pair(
    plaintext: &JwePlaintext,
    recipient: &PublicJwk,
    context: &PairContext,
) -> AuthorityResult<String> {
    let ephemeral = WrappingKey::generate()?;
    let kek = ephemeral.agree(recipient, context)?;
    let mut cek = Zeroizing::new([0; 32]);
    getrandom::fill(cek.as_mut()).map_err(|_| AuthorityError::Custody)?;
    let header = PairHeader {
        alg: "ECDH-ES+A256KW".into(),
        enc: "A256GCM".into(),
        typ: "mdreader-pair+jwe".into(),
        epk: ephemeral.public()?,
        apu: context.apu.clone(),
        apv: context.apv.clone(),
    };
    use aes_kw::KeyInit;
    let kw =
        aes_kw::KwAes256::new_from_slice(kek.as_slice()).map_err(|_| AuthorityError::Custody)?;
    let mut wrapped = [0; 40];
    kw.wrap_key(cek.as_slice(), &mut wrapped)
        .map_err(|_| AuthorityError::Custody)?;
    compact::encrypt(
        &header,
        &wrapped,
        cek.as_slice(),
        plaintext,
        compact::MAX_PAIR,
    )
}
pub fn decrypt_pair(
    jwe: &str,
    recipient: &WrappingKey,
    context: &PairContext,
) -> AuthorityResult<JwePlaintext> {
    let parts = compact::parse(jwe, compact::MAX_PAIR)?;
    let header: PairHeader =
        serde_json::from_slice(&parts.header).map_err(|_| AuthorityError::Custody)?;
    if header.alg != "ECDH-ES+A256KW"
        || header.enc != "A256GCM"
        || header.typ != "mdreader-pair+jwe"
        || header.apu != context.apu
        || header.apv != context.apv
    {
        return Err(AuthorityError::Scope);
    }
    let kek = recipient.agree(&header.epk, context)?;
    let wrapped = key::decode_fixed::<40>(parts.encrypted_key)?;
    use aes_kw::KeyInit;
    let kw =
        aes_kw::KwAes256::new_from_slice(kek.as_slice()).map_err(|_| AuthorityError::Custody)?;
    let mut cek = Zeroizing::new([0; 32]);
    kw.unwrap_key(&wrapped, cek.as_mut())
        .map_err(|_| AuthorityError::Custody)?;
    compact::decrypt(parts, cek.as_slice())
}
pub fn encrypt_recovery(
    plaintext: &JwePlaintext,
    secret: &RecoverySecret,
) -> AuthorityResult<String> {
    let header = RecoveryHeader {
        alg: "dir".into(),
        enc: "A256GCM".into(),
        typ: "mdreader-recovery+jwe".into(),
    };
    compact::encrypt(
        &header,
        &[],
        secret.0.as_slice(),
        plaintext,
        compact::MAX_RECOVERY,
    )
}
pub fn decrypt_recovery(jwe: &str, secret: &RecoverySecret) -> AuthorityResult<JwePlaintext> {
    let parts = compact::parse(jwe, compact::MAX_RECOVERY)?;
    let header: RecoveryHeader =
        serde_json::from_slice(&parts.header).map_err(|_| AuthorityError::Custody)?;
    if header.alg != "dir"
        || header.enc != "A256GCM"
        || header.typ != "mdreader-recovery+jwe"
        || !parts.encrypted_key.is_empty()
    {
        return Err(AuthorityError::Custody);
    }
    compact::decrypt(parts, secret.0.as_slice())
}

#[cfg(test)]
mod tests;
