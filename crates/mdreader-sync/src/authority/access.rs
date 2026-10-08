//! API authorization is distinct from nonce-bound OIDC login identity.
use super::*;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use zeroize::{Zeroize, Zeroizing};
#[derive(Deserialize)]
struct Claims {
    sub: String,
    aud: Value,
    client_id: String,
    scope: String,
    exp: u64,
    iat: u64,
    jti: String,
    #[serde(default)]
    azp: Option<String>,
}
impl Drop for Claims {
    fn drop(&mut self) {
        self.sub.zeroize();
    }
}
pub(super) fn verify_pair(
    registration: &ReviewedOidcRegistration,
    id_token: Zeroizing<String>,
    access_token: Zeroizing<String>,
    jwks: &[u8],
    nonce: &str,
    created: u64,
    response_expires: u64,
) -> AuthorityResult<VerifiedIdentity> {
    let id = super::identity::verify(registration, id_token, jwks, nonce, created)?;
    let claims: Claims = super::jwt::verify_claims(
        &registration.issuer,
        &registration.api_audience,
        &access_token,
        jwks,
        super::jwt::TokenKind::Access,
    )?;
    let audiences = match &claims.aud {
        Value::String(v) => vec![v.as_str()],
        Value::Array(v) => v
            .iter()
            .map(|s| s.as_str().ok_or(AuthorityError::Identity))
            .collect::<AuthorityResult<Vec<_>>>()?,
        _ => return Err(AuthorityError::Identity),
    };
    let unique: HashSet<_> = audiences.iter().copied().collect();
    let scopes: Vec<_> = claims.scope.split(' ').collect();
    let scope_unique: HashSet<_> = scopes.iter().copied().collect();
    let now = now()?;
    if claims.sub.as_str() != id.subject.as_str()
        || claims.client_id != registration.client_id
        || claims
            .azp
            .as_deref()
            .is_some_and(|c| c != registration.client_id)
        || audiences.is_empty()
        || audiences.len() > 8
        || unique.len() != audiences.len()
        || !unique.contains(registration.api_audience.as_str())
        || claims.scope.len() > 512
        || scopes.len() > 16
        || scopes.len() != scope_unique.len()
        || scopes.iter().any(|s| {
            s.is_empty()
                || s.bytes().any(|b| {
                    !(b == 0x21 || (0x23..=0x5b).contains(&b) || (0x5d..=0x7e).contains(&b))
                })
        })
        || !scope_unique.contains("sync:read")
        || !scope_unique.contains("sync:write")
        || claims.jti.is_empty()
        || claims.jti.len() > 128
        || claims.jti.chars().any(char::is_control)
        || claims.exp <= now
        || claims.iat > now.saturating_add(5)
        || claims.iat < created.saturating_sub(5)
        || claims.exp <= claims.iat
        || claims.exp - claims.iat > 86400
    {
        return Err(AuthorityError::Identity);
    }
    let expires = id.expires.min(claims.exp).min(response_expires);
    if expires <= now {
        return Err(AuthorityError::Identity);
    }
    Ok(VerifiedIdentity {
        account: id.account,
        registration: registration.digest.clone(),
        token: access_token,
        login_token: id.token,
        expires,
    })
}
