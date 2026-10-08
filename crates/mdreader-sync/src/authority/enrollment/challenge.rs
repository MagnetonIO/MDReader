//! Bounded inspection of an opaque server challenge received over pinned HTTPS.
//! The native client does not possess/verify the server's HS256 secret.
use super::super::{
    custody::{Lifecycle, Phase},
    signing::SigningKey,
    wrapping,
};
use super::*;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    alg: String,
    typ: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in super::super) struct Claims {
    v: u8,
    pub kind: String,
    library_id: String,
    signing_thumb: String,
    wrapping_thumb: String,
    recovery_thumb: Option<String>,
    pub nonce: String,
    iss: String,
    pub aud: String,
    iat: u64,
    pub exp: u64,
    pub jti: String,
}
impl Claims {
    pub(in super::super) fn parse(token: &str) -> AuthorityResult<Self> {
        if token.len() >= 4096 {
            return Err(AuthorityError::Bounds);
        }
        let parts: Vec<_> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(AuthorityError::Enrollment);
        }
        let decode =
            |s: &str| crate::crypto::decode_canonical(s).map_err(|_| AuthorityError::Enrollment);
        let header: Header =
            serde_json::from_slice(&decode(parts[0])?).map_err(|_| AuthorityError::Enrollment)?;
        if header.alg != "HS256"
            || header.typ != "mdreader-enrollment+jwt"
            || decode(parts[2])?.len() != 32
        {
            return Err(AuthorityError::Enrollment);
        }
        let claims: Self =
            serde_json::from_slice(&decode(parts[1])?).map_err(|_| AuthorityError::Enrollment)?;
        crate::crypto::validate_uuid(&claims.jti).map_err(|_| AuthorityError::Enrollment)?;
        if claims.v != 2
            || claims.iss != "mdreader-sync"
            || decode(&claims.nonce)?.len() != 32
            || claims.exp <= claims.iat
            || claims.exp - claims.iat > 300
        {
            return Err(AuthorityError::Enrollment);
        }
        Ok(claims)
    }
    pub(in super::super) fn check_record(&self, record: &CustodyRecord) -> AuthorityResult<()> {
        let Lifecycle::V2 {
            wrapping: raw,
            recovery,
            phase,
        } = &record.lifecycle
        else {
            return Err(AuthorityError::LifecycleGated);
        };
        let wrapping = wrapping::WrappingKey::from_bytes(zeroize::Zeroizing::new(
            raw.as_slice()
                .try_into()
                .map_err(|_| AuthorityError::Custody)?,
        ))?;
        let purpose_ok = match self.kind.as_str() {
            "bootstrap" => {
                matches!(phase, Phase::BootstrapPending | Phase::Enrolled)
                    && self.recovery_thumb.as_deref()
                        == recovery
                            .as_ref()
                            .map(|r| SigningKey::load(r).map(|s| s.device))
                            .transpose()?
                            .as_deref()
            }
            "recover" => {
                matches!(
                    phase,
                    Phase::CandidatePending(CandidateKind::Recover) | Phase::Enrolled
                ) && self.recovery_thumb.is_none()
            }
            _ => false,
        };
        if !purpose_ok
            || self.library_id != record.scope.library_id()
            || self.signing_thumb != record.scope.device_id()
            || self.wrapping_thumb != wrapping.public()?.thumbprint()?
        {
            return Err(AuthorityError::Scope);
        }
        Ok(())
    }
    pub(in super::super) fn check_fresh(&self, audience: &str) -> AuthorityResult<()> {
        let current = now()?;
        if self.aud != audience
            || self.iat > current.saturating_add(5)
            || self.exp <= current
            || self.exp > current.saturating_add(300)
        {
            return Err(AuthorityError::Enrollment);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in super::super) struct Response {
    pub challenge: String,
    pub nonce: String,
    pub expires: u64,
}
pub(in super::super) async fn fetch(
    factory: &NativeAuthorityFactory,
    record: &CustodyRecord,
    kind: Kind,
) -> AuthorityResult<Response> {
    let (signing, wrapping, recovery) = super::bootstrap::public_material(record)?;
    let kind_text = match kind {
        Kind::Bootstrap => "bootstrap",
        Kind::Recover => "recover",
    };
    let request=zeroize::Zeroizing::new(serde_json::to_vec(&serde_json::json!({"kind":kind_text,"libraryId":record.scope.library_id(),"signingJwk":signing,"wrappingJwk":wrapping,"recoveryJwk":if kind==Kind::Bootstrap {recovery}else{serde_json::Value::Null}})).map_err(|_|AuthorityError::Custody)?);
    let url = factory
        .client
        .registration
        .sync
        .join("/v1/enrollment/challenge")
        .map_err(|_| AuthorityError::Configuration)?;
    let bytes = factory
        .client
        .http
        .send(
            reqwest::Method::POST,
            url,
            request,
            None,
            "application/json",
            8192,
        )
        .await?;
    let response: Response =
        serde_json::from_slice(&bytes).map_err(|_| AuthorityError::Enrollment)?;
    let c = Claims::parse(&response.challenge)?;
    c.check_record(record)?;
    c.check_fresh(&factory.client.registration.api_audience)?;
    if c.kind != kind_text || c.nonce != response.nonce || c.exp != response.expires {
        return Err(AuthorityError::Scope);
    }
    Ok(response)
}
