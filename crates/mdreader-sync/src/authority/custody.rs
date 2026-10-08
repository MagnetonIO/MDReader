use super::signing::SigningKey;
use super::*;
use crate::{
    crypto::{decode_canonical, validate_uuid},
    LibraryKey, SyncScope,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::{
    rand::SystemRandom,
    signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[cfg(test)]
use std::{sync::Mutex, time::Duration};
use zeroize::{Zeroize, Zeroizing};
const PREFIX: &str = "native-sync-v1-";
const MAX_RECORD: usize = 16 * 1024;
#[derive(Clone, PartialEq, Eq)]
pub struct SyncCredentialReference(pub(super) String);
impl std::fmt::Debug for SyncCredentialReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        redacted("SyncCredentialReference", f)
    }
}
impl SyncCredentialReference {
    /// Native owner may persist this opaque reference only, never private record bytes.
    pub fn parse(value: &str) -> AuthorityResult<Self> {
        let id = value.strip_prefix(PREFIX).ok_or(AuthorityError::Scope)?;
        validate_uuid(id).map_err(|_| AuthorityError::Scope)?;
        Ok(Self(value.into()))
    }
    pub fn opaque_reference(&self) -> &str {
        &self.0
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum EnrollmentState {
    Pending,
    Confirmed,
}
pub struct CustodyRecord {
    pub(super) reference: SyncCredentialReference,
    pub(super) registration: String,
    pub(super) scope: SyncScope,
    pub(super) generation: u64,
    pub(super) state: EnrollmentState,
    pub(super) pkcs8: Zeroizing<Vec<u8>>,
    pub(super) keys: BTreeMap<String, Zeroizing<Vec<u8>>>,
}
impl std::fmt::Debug for CustodyRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        redacted("CustodyRecord", f)
    }
}
impl CustodyRecord {
    pub(super) fn copy(&self) -> Self {
        Self {
            reference: self.reference.clone(),
            registration: self.registration.clone(),
            scope: self.scope.clone(),
            generation: self.generation,
            state: self.state,
            pkcs8: self.pkcs8.clone(),
            keys: self.keys.clone(),
        }
    }
    pub(super) fn validate(&self, reference: &SyncCredentialReference) -> AuthorityResult<()> {
        SyncCredentialReference::parse(&self.reference.0)?;
        if &self.reference != reference
            || self.registration.len() != 64
            || !self
                .registration
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || self.generation == 0
            || self.pkcs8.len() > 1024
            || self.keys.is_empty()
            || self.keys.len() > 16
        {
            return Err(AuthorityError::Custody);
        }
        let signer = SigningKey::load(&self.pkcs8)?;
        let expected = SyncScope::new(
            self.scope.account_digest().into(),
            self.scope.library_id().into(),
            signer.device.clone(),
        )
        .map_err(|_| AuthorityError::Scope)?;
        if expected != self.scope {
            return Err(AuthorityError::Scope);
        }
        for (id, raw) in &self.keys {
            validate_uuid(id).map_err(|_| AuthorityError::Custody)?;
            if raw.len() != 32 {
                return Err(AuthorityError::Custody);
            }
        }
        Ok(())
    }
    pub(super) fn same_material(&self, other: &Self) -> bool {
        self.reference == other.reference
            && self.registration == other.registration
            && self.scope == other.scope
            && self.pkcs8 == other.pkcs8
            && self.keys == other.keys
    }
    pub(super) fn check_identity(
        &self,
        identity: &VerifiedIdentity,
        registration: &ReviewedOidcRegistration,
        library: &str,
    ) -> AuthorityResult<()> {
        self.validate(&self.reference)?;
        identity.check(registration)?;
        if self.registration != registration.digest
            || self.scope.account_digest() != identity.account
            || self.scope.library_id() != library
        {
            return Err(AuthorityError::Scope);
        }
        Ok(())
    }
}
/// Preparing keys performs no OS/network I/O. Persist the reference in the native owner
/// before persisting this same pending record; retain it after uncertain OS replies.
pub struct PendingCustody {
    pub(super) record: CustodyRecord,
}
impl std::fmt::Debug for PendingCustody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        redacted("PendingCustody", f)
    }
}
impl PendingCustody {
    pub fn reference(&self) -> SyncCredentialReference {
        self.record.reference.clone()
    }
    pub(super) fn prepare(
        identity: &VerifiedIdentity,
        registration: &ReviewedOidcRegistration,
        library: &str,
    ) -> AuthorityResult<Self> {
        identity.check(registration)?;
        validate_uuid(library).map_err(|_| AuthorityError::Scope)?;
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
                .map_err(|_| AuthorityError::Custody)?;
        let pkcs8 = Zeroizing::new(pkcs8.as_ref().to_vec());
        let signer = SigningKey::load(&pkcs8)?;
        let library_key = LibraryKey::generate().map_err(|_| AuthorityError::Custody)?;
        let reference = SyncCredentialReference(format!("{PREFIX}{}", uuid::Uuid::new_v4()));
        let record = CustodyRecord {
            reference,
            registration: registration.digest.clone(),
            scope: SyncScope::new(identity.account.clone(), library.into(), signer.device)
                .map_err(|_| AuthorityError::Scope)?,
            generation: 1,
            state: EnrollmentState::Pending,
            pkcs8,
            keys: BTreeMap::from([(library_key.id.clone(), library_key.bytes.clone())]),
        };
        record.validate(&record.reference)?;
        Ok(Self { record })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
struct Wire {
    v: u8,
    reference: String,
    registration: String,
    scope: SyncScope,
    generation: u64,
    enrollment: EnrollmentState,
    signing_pkcs8: String,
    keys: Vec<WireKey>,
}
impl Drop for Wire {
    fn drop(&mut self) {
        self.signing_pkcs8.zeroize();
    }
}
pub(super) fn encode(record: &CustodyRecord) -> AuthorityResult<Zeroizing<Vec<u8>>> {
    record.validate(&record.reference)?;
    let wire = Wire {
        v: 1,
        reference: record.reference.0.clone(),
        registration: record.registration.clone(),
        scope: record.scope.clone(),
        generation: record.generation,
        enrollment: record.state,
        signing_pkcs8: URL_SAFE_NO_PAD.encode(record.pkcs8.as_slice()),
        keys: record
            .keys
            .iter()
            .map(|(id, raw)| WireKey {
                key_id: id.clone(),
                raw: URL_SAFE_NO_PAD.encode(raw.as_slice()),
            })
            .collect(),
    };
    let bytes = Zeroizing::new(serde_json::to_vec(&wire).map_err(|_| AuthorityError::Custody)?);
    if bytes.len() > MAX_RECORD {
        return Err(AuthorityError::Bounds);
    }
    Ok(bytes)
}
pub(super) fn decode(
    reference: &SyncCredentialReference,
    bytes: Zeroizing<Vec<u8>>,
) -> AuthorityResult<CustodyRecord> {
    if bytes.len() > MAX_RECORD {
        return Err(AuthorityError::Bounds);
    }
    let wire: Wire = serde_json::from_slice(&bytes).map_err(|_| AuthorityError::Custody)?;
    if wire.v != 1 || wire.keys.is_empty() || wire.keys.len() > 16 {
        return Err(AuthorityError::Custody);
    }
    let mut keys = BTreeMap::new();
    for key in &wire.keys {
        if keys
            .insert(
                key.key_id.clone(),
                Zeroizing::new(decode_canonical(&key.raw).map_err(|_| AuthorityError::Custody)?),
            )
            .is_some()
        {
            return Err(AuthorityError::Custody);
        }
    }
    let record = CustodyRecord {
        reference: SyncCredentialReference::parse(&wire.reference)?,
        registration: wire.registration.clone(),
        scope: wire.scope.clone(),
        generation: wire.generation,
        state: wire.enrollment,
        pkcs8: Zeroizing::new(
            decode_canonical(&wire.signing_pkcs8).map_err(|_| AuthorityError::Custody)?,
        ),
        keys,
    };
    record.validate(reference)?;
    Ok(record)
}
pub(super) fn admission(
    old: &CustodyRecord,
    new: &CustodyRecord,
    expected: Option<u64>,
) -> AuthorityResult<()> {
    if !old.same_material(new) {
        return Err(AuthorityError::Generation);
    }
    match expected {
        None if old.generation == new.generation && old.state == new.state => Ok(()),
        Some(g)
            if old.generation == g
                && new.generation == g.checked_add(1).ok_or(AuthorityError::Generation)?
                && old.state == EnrollmentState::Pending
                && new.state == EnrollmentState::Confirmed =>
        {
            Ok(())
        }
        _ => Err(AuthorityError::Generation),
    }
}
#[cfg(test)]
pub(super) mod test_support {
    use super::*;
    /// Synthetic bytes only. Models process restart by private encode/decode, not shared key objects.
    pub struct MemoryVault {
        pub records: Mutex<BTreeMap<String, Zeroizing<Vec<u8>>>>,
        pub delay: std::sync::atomic::AtomicBool,
    }
    impl MemoryVault {
        pub fn new() -> Self {
            Self {
                records: Mutex::new(BTreeMap::new()),
                delay: std::sync::atomic::AtomicBool::new(false),
            }
        }
    }
    impl SyncVault for MemoryVault {
        fn get(&self, r: &SyncCredentialReference) -> AuthorityResult<Option<CustodyRecord>> {
            self.records
                .lock()
                .unwrap()
                .get(&r.0)
                .map(|b| decode(r, b.clone()))
                .transpose()
        }
        fn create(&self, r: &CustodyRecord) -> AuthorityResult<()> {
            if self.delay.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(80));
            }
            let mut rows = self.records.lock().unwrap();
            if let Some(old) = rows.get(&r.reference.0) {
                return admission(&decode(&r.reference, old.clone())?, r, None);
            }
            if r.state != EnrollmentState::Pending || r.generation != 1 {
                return Err(AuthorityError::Generation);
            }
            rows.insert(r.reference.0.clone(), encode(r)?);
            Ok(())
        }
        fn compare_exchange(&self, g: u64, r: &CustodyRecord) -> AuthorityResult<()> {
            let mut rows = self.records.lock().unwrap();
            let old = decode(
                &r.reference,
                rows.get(&r.reference.0)
                    .ok_or(AuthorityError::Custody)?
                    .clone(),
            )?;
            admission(&old, r, Some(g))?;
            rows.insert(r.reference.0.clone(), encode(r)?);
            Ok(())
        }
    }
}
