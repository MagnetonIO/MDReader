use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use zeroize::Zeroize;
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Key {
    key_id: String,
    raw: String,
}
impl Drop for Key {
    fn drop(&mut self) {
        self.raw.zeroize();
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Wire {
    v: u8,
    kind: String,
    account: String,
    library_id: String,
    keys: Vec<Key>,
}
pub(super) fn export(record: &CustodyRecord) -> AuthorityResult<wrapping::JwePlaintext> {
    let wire = Wire {
        v: 1,
        kind: "mdreader-library-keys".into(),
        account: record.scope.account_digest().into(),
        library_id: record.scope.library_id().into(),
        keys: record
            .keys
            .iter()
            .map(|(id, raw)| Key {
                key_id: id.clone(),
                raw: URL_SAFE_NO_PAD.encode(raw.as_slice()),
            })
            .collect(),
    };
    wrapping::JwePlaintext::new(Zeroizing::new(
        serde_json::to_vec(&wire).map_err(|_| AuthorityError::Custody)?,
    ))
}
pub(super) fn parse(
    bytes: &[u8],
    request: &PairRequestPacket,
) -> AuthorityResult<BTreeMap<String, Zeroizing<Vec<u8>>>> {
    if bytes.len() > 16384 {
        return Err(AuthorityError::Bounds);
    }
    let wire: Wire = serde_json::from_slice(bytes).map_err(|_| AuthorityError::Custody)?;
    if wire.v != 1
        || wire.kind != "mdreader-library-keys"
        || wire.account != request.account
        || wire.library_id != request.library_id
        || wire.keys.is_empty()
        || wire.keys.len() > 16
    {
        return Err(AuthorityError::Scope);
    }
    let mut keys = BTreeMap::new();
    for key in &wire.keys {
        crate::crypto::validate_uuid(&key.key_id).map_err(|_| AuthorityError::Scope)?;
        nonce(&key.raw)?;
        let raw = Zeroizing::new(
            crate::crypto::decode_canonical(&key.raw).map_err(|_| AuthorityError::Custody)?,
        );
        if keys.insert(key.key_id.clone(), raw).is_some() {
            return Err(AuthorityError::Custody);
        }
    }
    Ok(keys)
}
