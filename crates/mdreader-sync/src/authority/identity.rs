use super::*;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use zeroize::{Zeroize, Zeroizing};
pub struct VerifiedIdentity {
    pub(super) account: String,
    pub(super) registration: String,
    pub(super) token: Zeroizing<String>,
    pub(super) expires: u64,
}
impl std::fmt::Debug for VerifiedIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        redacted("VerifiedIdentity", f)
    }
}
impl VerifiedIdentity {
    pub(super) fn check(&self, registration: &ReviewedOidcRegistration) -> AuthorityResult<()> {
        if self.registration != registration.digest || now()? >= self.expires {
            Err(AuthorityError::Identity)
        } else {
            Ok(())
        }
    }
}
#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    aud: Value,
    exp: u64,
    iat: u64,
    nonce: String,
    #[serde(default)]
    azp: Option<String>,
}
impl Drop for Claims {
    fn drop(&mut self) {
        self.sub.zeroize();
        self.nonce.zeroize();
    }
}
pub(super) fn verify(
    registration: &ReviewedOidcRegistration,
    token: Zeroizing<String>,
    jwks: &[u8],
    nonce: &str,
    created: u64,
) -> AuthorityResult<VerifiedIdentity> {
    if token.is_empty() || token.len() >= 16 * 1024 || jwks.len() > 64 * 1024 {
        return Err(AuthorityError::Bounds);
    }
    // Unverified protected header selects a key only; no identity comes from an unverified decode.
    let header = decode_header(&token).map_err(|_| AuthorityError::Identity)?;
    let protected = token.split('.').next().ok_or(AuthorityError::Identity)?;
    let protected: Value = serde_json::from_slice(
        &crate::crypto::decode_canonical(protected).map_err(|_| AuthorityError::Identity)?,
    )
    .map_err(|_| AuthorityError::Identity)?;
    if protected.get("crit").is_some() || protected.get("b64").is_some() {
        return Err(AuthorityError::Identity);
    }
    if !matches!(header.alg, Algorithm::RS256 | Algorithm::ES256)
        || header.jku.is_some()
        || header.jwk.is_some()
        || header.x5u.is_some()
    {
        return Err(AuthorityError::Identity);
    }
    let kid = header
        .kid
        .as_ref()
        .filter(|k| !k.is_empty() && k.len() <= 128)
        .ok_or(AuthorityError::Identity)?;
    let set: Value = serde_json::from_slice(jwks).map_err(|_| AuthorityError::Identity)?;
    let keys = set
        .get("keys")
        .and_then(Value::as_array)
        .filter(|k| !k.is_empty() && k.len() <= 16)
        .ok_or(AuthorityError::Bounds)?;
    let matches: Vec<_> = keys
        .iter()
        .filter(|k| k.get("kid").and_then(Value::as_str) == Some(kid))
        .collect();
    if matches.len() != 1 {
        return Err(AuthorityError::Identity);
    }
    let key = matches[0];
    let alg = match header.alg {
        Algorithm::RS256 => "RS256",
        Algorithm::ES256 => "ES256",
        _ => return Err(AuthorityError::Identity),
    };
    if key.get("alg").is_some_and(|a| a.as_str() != Some(alg))
        || key.get("use").is_some_and(|u| u.as_str() != Some("sig"))
        || key
            .get("key_ops")
            .is_some_and(|ops| ops != &serde_json::json!(["verify"]))
        || ["d", "p", "q", "dp", "dq", "qi", "oth", "k"]
            .iter()
            .any(|p| key.get(p).is_some())
    {
        return Err(AuthorityError::Identity);
    }
    if (header.alg == Algorithm::RS256 && key.get("kty").and_then(Value::as_str) != Some("RSA"))
        || (header.alg == Algorithm::ES256
            && (key.get("kty").and_then(Value::as_str) != Some("EC")
                || key.get("crv").and_then(Value::as_str) != Some("P-256")))
    {
        return Err(AuthorityError::Identity);
    }
    let jwk: jsonwebtoken::jwk::Jwk =
        serde_json::from_value(key.clone()).map_err(|_| AuthorityError::Identity)?;
    let key = DecodingKey::from_jwk(&jwk).map_err(|_| AuthorityError::Identity)?;
    let mut validation = Validation::new(header.alg);
    validation.leeway = 5;
    validation.validate_nbf = true;
    validation.set_required_spec_claims(&["iss", "aud", "sub", "exp"]);
    validation.set_issuer(&[&registration.issuer]);
    validation.set_audience(&[&registration.client_id]);
    let claims = decode::<Claims>(&token, &key, &validation)
        .map_err(|_| AuthorityError::Identity)?
        .claims;
    let now = now()?;
    let audiences = match &claims.aud {
        Value::String(a) => vec![a.as_str()],
        Value::Array(values) => values
            .iter()
            .map(|v| v.as_str().ok_or(AuthorityError::Identity))
            .collect::<AuthorityResult<Vec<_>>>()?,
        _ => return Err(AuthorityError::Identity),
    };
    let unique: HashSet<_> = audiences.iter().copied().collect();
    if claims.iss != registration.issuer
        || claims.sub.is_empty()
        || claims.sub.len() > 255
        || audiences.is_empty()
        || audiences.len() > 8
        || unique.len() != audiences.len()
        || !unique.contains(registration.client_id.as_str())
        || claims
            .azp
            .as_ref()
            .is_some_and(|a| a != &registration.client_id)
        || (audiences.len() > 1 && claims.azp.as_deref() != Some(registration.client_id.as_str()))
        || claims.exp <= now
        || claims.iat > now.saturating_add(5)
        || claims.iat < created.saturating_sub(5)
        || claims.exp <= claims.iat
        || claims.exp - claims.iat > 86400
        || !secret_equal(claims.nonce.as_bytes(), nonce.as_bytes())
    {
        return Err(AuthorityError::Identity);
    }
    let account = crate::blob_digest(
        &serde_json::to_vec(&(&claims.iss, &claims.sub)).map_err(|_| AuthorityError::Identity)?,
    );
    Ok(VerifiedIdentity {
        account,
        registration: registration.digest.clone(),
        token,
        expires: claims.exp,
    })
}
