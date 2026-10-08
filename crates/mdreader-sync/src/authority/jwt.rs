//! Shared bounded signature/JWKS checks; token purpose remains explicit.
use super::*;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::de::DeserializeOwned;
use serde_json::Value;
pub(super) enum TokenKind {
    Id,
    Access,
}
pub(super) fn verify_claims<T: DeserializeOwned>(
    issuer: &str,
    audience: &str,
    token: &str,
    jwks: &[u8],
    kind: TokenKind,
) -> AuthorityResult<T> {
    if token.is_empty() || token.len() >= 16 * 1024 || jwks.len() > 64 * 1024 {
        return Err(AuthorityError::Bounds);
    }
    // Unverified protected header selects a key only; no identity comes from an unverified decode.
    let header = decode_header(token).map_err(|_| AuthorityError::Identity)?;
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
    let expected_type = match kind {
        TokenKind::Id => header.typ.as_deref().is_none_or(|v| v == "JWT"),
        TokenKind::Access => {
            header.typ.as_deref() == Some("at+jwt") && header.alg == Algorithm::RS256
        }
    };
    if !expected_type {
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
    validation.set_issuer(&[issuer]);
    validation.set_audience(&[audience]);
    Ok(decode::<T>(token, &key, &validation)
        .map_err(|_| AuthorityError::Identity)?
        .claims)
}
