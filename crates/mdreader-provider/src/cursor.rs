use crate::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;
/// Cursor kinds cannot be exchanged between list traversal and change tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorKind {
    List,
    Changes,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    v: u8,
    provider: ProviderKind,
    binding: SourceBinding,
    kind: CursorKind,
    token: OpaqueValue,
}
/// Account/source/selection/generation-bound authenticated cursor codec. The
/// secret is local credential-custody material; it is never serialized.
pub struct CursorCodec(Zeroizing<[u8; 32]>);
impl CursorCodec {
    pub fn new(key: [u8; 32]) -> Self {
        Self(Zeroizing::new(key))
    }
    pub fn encode(
        &self,
        provider: ProviderKind,
        binding: &SourceBinding,
        kind: CursorKind,
        token: OpaqueValue,
    ) -> Result<ProviderCursor> {
        let bytes = serde_json::to_vec(&Payload {
            v: 1,
            provider,
            binding: binding.clone(),
            kind,
            token,
        })
        .map_err(|_| ProviderError::Invalid)?;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(self.0.as_ref()).map_err(|_| ProviderError::Invalid)?;
        mac.update(b"mdreader-provider-cursor-v1\0");
        mac.update(&bytes);
        let mut signed = bytes;
        signed.extend_from_slice(&mac.finalize().into_bytes());
        ProviderCursor::new(URL_SAFE_NO_PAD.encode(signed).into_bytes())
    }
    pub fn decode(
        &self,
        provider: ProviderKind,
        binding: &SourceBinding,
        kind: CursorKind,
        cursor: &ProviderCursor,
    ) -> Result<OpaqueValue> {
        let signed = URL_SAFE_NO_PAD
            .decode(cursor.as_bytes())
            .map_err(|_| ProviderError::Invalid)?;
        if URL_SAFE_NO_PAD.encode(&signed).as_bytes() != cursor.as_bytes()
            || signed.len() < 33
            || signed.len() > MAX_CURSOR_BYTES
        {
            return Err(ProviderError::Invalid);
        }
        let (bytes, tag) = signed.split_at(signed.len() - 32);
        let mut mac =
            Hmac::<Sha256>::new_from_slice(self.0.as_ref()).map_err(|_| ProviderError::Invalid)?;
        mac.update(b"mdreader-provider-cursor-v1\0");
        mac.update(bytes);
        mac.verify_slice(tag)
            .map_err(|_| ProviderError::Unauthorized)?;
        let payload: Payload = serde_json::from_slice(bytes).map_err(|_| ProviderError::Invalid)?;
        if payload.v != 1
            || payload.provider != provider
            || payload.binding != *binding
            || payload.kind != kind
        {
            return Err(ProviderError::Unauthorized);
        }
        Ok(payload.token)
    }
}
