//! Immutable v1 schema. Read it strictly and retain its exact original bytes.
use super::*;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyWire {
    v: u8,
    reference: String,
    registration: String,
    scope: SyncScope,
    generation: u64,
    enrollment: EnrollmentState,
    signing_pkcs8: String,
    keys: Vec<WireKey>,
}
impl Drop for LegacyWire {
    fn drop(&mut self) {
        self.signing_pkcs8.zeroize();
    }
}
pub(super) fn decode(
    reference: &SyncCredentialReference,
    bytes: Zeroizing<Vec<u8>>,
) -> AuthorityResult<CustodyRecord> {
    let wire: LegacyWire = serde_json::from_slice(&bytes).map_err(|_| AuthorityError::Custody)?;
    if wire.v != 1 || wire.keys.is_empty() || wire.keys.len() > 16 {
        return Err(AuthorityError::Custody);
    }
    let mut keys = BTreeMap::new();
    for key in &wire.keys {
        if key.raw.len() != 43
            || keys
                .insert(key.key_id.clone(), decode_secret(&key.raw, 32)?)
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
        pkcs8: decode_secret(&wire.signing_pkcs8, 1024)?,
        keys,
        lifecycle: Lifecycle::Legacy(bytes),
        intent: None,
        pending_recovery: None,
    };
    record.validate(reference)?;
    Ok(record)
}
