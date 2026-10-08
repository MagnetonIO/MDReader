use super::*;
#[path = "legacy.rs"]
mod legacy;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wrapping_private: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery_private: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    phase: Option<Phase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    intent: Option<super::super::enrollment::Intent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_recovery: Option<String>,
}
impl Drop for Wire {
    fn drop(&mut self) {
        self.signing_pkcs8.zeroize();
        if let Some(raw) = &mut self.wrapping_private {
            raw.zeroize();
        }
        if let Some(raw) = &mut self.pending_recovery {
            raw.zeroize();
        }
        if let Some(raw) = &mut self.recovery_private {
            raw.zeroize();
        }
    }
}
pub(in super::super) fn encode(record: &CustodyRecord) -> AuthorityResult<Zeroizing<Vec<u8>>> {
    record.validate(&record.reference)?;
    if let Lifecycle::Legacy(bytes) = &record.lifecycle {
        let original = decode(&record.reference, bytes.clone())?;
        if !record.same_material(&original)
            || record.generation != original.generation
            || record.state != original.state
        {
            return Err(AuthorityError::Generation);
        }
        return Ok(bytes.clone());
    }
    let Lifecycle::V2 {
        wrapping,
        recovery,
        phase,
    } = &record.lifecycle
    else {
        return Err(AuthorityError::Custody);
    };
    let wire = Wire {
        v: 2,
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
        wrapping_private: Some(URL_SAFE_NO_PAD.encode(wrapping.as_slice())),
        recovery_private: recovery
            .as_ref()
            .map(|raw| URL_SAFE_NO_PAD.encode(raw.as_slice())),
        phase: Some(*phase),
        intent: record.intent.clone(),
        pending_recovery: record
            .pending_recovery
            .as_ref()
            .map(|raw| URL_SAFE_NO_PAD.encode(raw.as_slice())),
    };
    let bytes = Zeroizing::new(serde_json::to_vec(&wire).map_err(|_| AuthorityError::Custody)?);
    if bytes.len() > MAX_RECORD {
        return Err(AuthorityError::Bounds);
    }
    Ok(bytes)
}
pub(in super::super) fn decode(
    reference: &SyncCredentialReference,
    bytes: Zeroizing<Vec<u8>>,
) -> AuthorityResult<CustodyRecord> {
    if bytes.len() > MAX_RECORD {
        return Err(AuthorityError::Bounds);
    }
    #[derive(Deserialize)]
    struct Version {
        v: u8,
    }
    let version: Version = serde_json::from_slice(&bytes).map_err(|_| AuthorityError::Custody)?;
    if version.v == 1 {
        return legacy::decode(reference, bytes);
    }
    if version.v != 2 {
        return Err(AuthorityError::Custody);
    }
    let wire: Wire = serde_json::from_slice(&bytes).map_err(|_| AuthorityError::Custody)?;
    if !matches!(wire.v, 1 | 2) || wire.keys.len() > 16 || wire.signing_pkcs8.len() > 1368 {
        return Err(AuthorityError::Custody);
    }
    let mut keys = BTreeMap::new();
    for key in &wire.keys {
        if key.raw.len() != 43 {
            return Err(AuthorityError::Custody);
        }
        if keys
            .insert(key.key_id.clone(), decode_secret(&key.raw, 32)?)
            .is_some()
        {
            return Err(AuthorityError::Custody);
        }
    }
    let lifecycle = match wire.v {
        2 => {
            let wrapping = wire
                .wrapping_private
                .as_ref()
                .ok_or(AuthorityError::Custody)?;
            if wrapping.len() != 43
                || wire
                    .recovery_private
                    .as_ref()
                    .is_some_and(|r| r.len() > 1368)
            {
                return Err(AuthorityError::Bounds);
            }
            Lifecycle::V2 {
                wrapping: decode_secret(wrapping, 32)?,
                recovery: wire
                    .recovery_private
                    .as_ref()
                    .map(|raw| decode_secret(raw, 1024))
                    .transpose()?,
                phase: wire.phase.ok_or(AuthorityError::Custody)?,
            }
        }
        _ => return Err(AuthorityError::Custody),
    };
    let record = CustodyRecord {
        reference: SyncCredentialReference::parse(&wire.reference)?,
        registration: wire.registration.clone(),
        scope: wire.scope.clone(),
        generation: wire.generation,
        state: wire.enrollment,
        pkcs8: decode_secret(&wire.signing_pkcs8, 1024)?,
        keys,
        lifecycle,
        intent: wire.intent.clone(),
        pending_recovery: wire
            .pending_recovery
            .as_ref()
            .map(|raw| decode_secret(raw, 16384))
            .transpose()?,
    };
    record.validate(reference)?;
    Ok(record)
}
fn decode_secret(raw: &str, max: usize) -> AuthorityResult<Zeroizing<Vec<u8>>> {
    if raw.len() > max.div_ceil(3) * 4 {
        return Err(AuthorityError::Bounds);
    }
    let bytes = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(raw)
            .map_err(|_| AuthorityError::Custody)?,
    );
    let encoded = Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_slice()));
    if bytes.len() > max || encoded.as_str() != raw {
        return Err(AuthorityError::Custody);
    }
    Ok(bytes)
}
