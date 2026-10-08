use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use private_jwk::PrivateJwk;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use zeroize::Zeroize;
pub(in super::super) struct Material {
    pub account: String,
    pub library: String,
    pub keys: BTreeMap<String, Zeroizing<Vec<u8>>>,
    pub recovery: Zeroizing<Vec<u8>>,
}
impl Material {
    pub(in super::super) fn check_scope(
        &self,
        account: &str,
        library: &str,
    ) -> AuthorityResult<()> {
        if self.account != account || self.library != library {
            return Err(AuthorityError::Scope);
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireKey {
    key_id: String,
    raw: String,
}
impl Drop for WireKey {
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
    keys: Vec<WireKey>,
    recovery_private_jwk: PrivateJwk,
}
pub(super) fn parse(bytes: &[u8], account: &str, library: &str) -> AuthorityResult<Material> {
    if bytes.len() > 16384 {
        return Err(AuthorityError::Bounds);
    }
    let wire: Wire = serde_json::from_slice(bytes).map_err(|_| AuthorityError::Custody)?;
    if wire.v != 1
        || wire.kind != "mdreader-library-keys"
        || wire.account != account
        || wire.library_id != library
        || wire.keys.is_empty()
        || wire.keys.len() > 16
    {
        return Err(AuthorityError::Scope);
    }
    let mut keys = BTreeMap::new();
    for key in &wire.keys {
        crate::crypto::validate_uuid(&key.key_id).map_err(|_| AuthorityError::Custody)?;
        if key.raw.len() != 43 {
            return Err(AuthorityError::Custody);
        }
        let raw = Zeroizing::new(
            crate::crypto::decode_canonical(&key.raw).map_err(|_| AuthorityError::Custody)?,
        );
        if raw.len() != 32 || keys.insert(key.key_id.clone(), raw).is_some() {
            return Err(AuthorityError::Custody);
        }
    }
    Ok(Material {
        account: wire.account,
        library: wire.library_id,
        keys,
        recovery: wire.recovery_private_jwk.to_pkcs8()?,
    })
}
pub(super) fn export(record: &CustodyRecord) -> AuthorityResult<Zeroizing<Vec<u8>>> {
    let custody::Lifecycle::V2 {
        recovery: Some(private),
        phase: custody::Phase::Enrolled,
        ..
    } = &record.lifecycle
    else {
        return Err(AuthorityError::LifecycleGated);
    };
    encode(&Material {
        account: record.scope.account_digest().into(),
        library: record.scope.library_id().into(),
        keys: record.keys.clone(),
        recovery: private.clone(),
    })
}
pub(super) fn encode(material: &Material) -> AuthorityResult<Zeroizing<Vec<u8>>> {
    let wire = Wire {
        v: 1,
        kind: "mdreader-library-keys".into(),
        account: material.account.clone(),
        library_id: material.library.clone(),
        keys: material
            .keys
            .iter()
            .map(|(id, raw)| WireKey {
                key_id: id.clone(),
                raw: URL_SAFE_NO_PAD.encode(raw.as_slice()),
            })
            .collect(),
        recovery_private_jwk: PrivateJwk::from_pkcs8(&material.recovery)?,
    };
    let bytes = Zeroizing::new(serde_json::to_vec(&wire).map_err(|_| AuthorityError::Custody)?);
    if bytes.len() > 16384 {
        return Err(AuthorityError::Bounds);
    }
    Ok(bytes)
}
