use super::*;
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
pub(super) const MAX_PAIR: usize = 4095; // Actual deployed Worker bound is <4096.
pub(super) const MAX_RECOVERY: usize = 32768;
pub(super) struct Parts<'a> {
    pub header: Vec<u8>,
    protected: &'a str,
    pub encrypted_key: &'a str,
    iv: [u8; 12],
    ciphertext: Vec<u8>,
}
pub(super) fn parse(jwe: &str, max: usize) -> AuthorityResult<Parts<'_>> {
    if jwe.len() > max {
        return Err(AuthorityError::Bounds);
    }
    let mut values = jwe.split('.');
    let mut next = || values.next().ok_or(AuthorityError::Custody);
    let protected = next()?;
    let encrypted_key = next()?;
    let iv = next()?;
    let ciphertext = next()?;
    let tag = next()?;
    if values.next().is_some() {
        return Err(AuthorityError::Custody);
    }
    let header = key::decode_bounded(protected, 2048)?;
    let iv = key::decode_fixed::<12>(iv)?;
    let tag = key::decode_fixed::<16>(tag)?;
    let mut ciphertext = key::decode_bounded(ciphertext, 16 * 1024)?;
    if ciphertext.is_empty() {
        return Err(AuthorityError::Bounds);
    }
    ciphertext.extend_from_slice(&tag);
    Ok(Parts {
        header,
        protected,
        encrypted_key,
        iv,
        ciphertext,
    })
}
pub(super) fn encrypt(
    header: &impl Serialize,
    wrapped: &[u8],
    key: &[u8],
    plaintext: &JwePlaintext,
    max: usize,
) -> AuthorityResult<String> {
    let protected =
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(header).map_err(|_| AuthorityError::Custody)?);
    let mut iv = [0; 12];
    getrandom::fill(&mut iv).map_err(|_| AuthorityError::Custody)?;
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| AuthorityError::Custody)?;
    let data = cipher
        .encrypt(
            &Nonce::from(iv),
            Payload {
                msg: plaintext.as_bytes(),
                aad: protected.as_bytes(),
            },
        )
        .map_err(|_| AuthorityError::Custody)?;
    let (ciphertext, tag) = data.split_at(data.len() - 16);
    let jwe = format!(
        "{}.{}.{}.{}.{}",
        protected,
        URL_SAFE_NO_PAD.encode(wrapped),
        URL_SAFE_NO_PAD.encode(iv),
        URL_SAFE_NO_PAD.encode(ciphertext),
        URL_SAFE_NO_PAD.encode(tag)
    );
    if jwe.len() > max {
        return Err(AuthorityError::Bounds);
    }
    Ok(jwe)
}
pub(super) fn decrypt(parts: Parts<'_>, key: &[u8]) -> AuthorityResult<JwePlaintext> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| AuthorityError::Custody)?;
    let bytes = cipher
        .decrypt(
            &Nonce::from(parts.iv),
            Payload {
                msg: &parts.ciphertext,
                aad: parts.protected.as_bytes(),
            },
        )
        .map_err(|_| AuthorityError::Custody)?;
    JwePlaintext::new(Zeroizing::new(bytes))
}
