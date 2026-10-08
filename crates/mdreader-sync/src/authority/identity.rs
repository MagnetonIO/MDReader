use super::*;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use zeroize::{Zeroize, Zeroizing};
pub struct VerifiedIdentity {
    pub(super) account: String,
    pub(super) registration: String,
    pub(super) token: Zeroizing<String>,
    pub(super) login_token: Zeroizing<String>,
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
pub(super) struct IdIdentity {
    pub(super) account: String,
    pub(super) subject: Zeroizing<String>,
    pub(super) token: Zeroizing<String>,
    pub(super) expires: u64,
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
) -> AuthorityResult<IdIdentity> {
    let claims: Claims = super::jwt::verify_claims(
        &registration.issuer,
        &registration.client_id,
        &token,
        jwks,
        super::jwt::TokenKind::Id,
    )?;
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
    Ok(IdIdentity {
        account,
        subject: Zeroizing::new(claims.sub.clone()),
        token,
        expires: claims.exp,
    })
}
