use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use ring::{
    rand::SystemRandom,
    signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING},
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;
pub(super) struct SigningKey {
    encoding: EncodingKey,
    pub(super) jwk: serde_json::Value,
    pub(super) device: String,
}
impl SigningKey {
    pub(super) fn load(pkcs8: &[u8]) -> AuthorityResult<Self> {
        let key = EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_FIXED_SIGNING,
            pkcs8,
            &SystemRandom::new(),
        )
        .map_err(|_| AuthorityError::Custody)?;
        let point = key.public_key().as_ref();
        if point.len() != 65 || point[0] != 4 {
            return Err(AuthorityError::Custody);
        }
        let jwk = serde_json::json!({"crv":"P-256","kty":"EC","x":URL_SAFE_NO_PAD.encode(&point[1..33]),"y":URL_SAFE_NO_PAD.encode(&point[33..65])});
        let device = URL_SAFE_NO_PAD.encode(Sha256::digest(
            serde_json::to_vec(&jwk).map_err(|_| AuthorityError::Custody)?,
        ));
        Ok(Self {
            encoding: EncodingKey::from_ec_der(pkcs8),
            jwk,
            device,
        })
    }
    pub(super) fn recovery_grant(&self, claims: &impl serde::Serialize) -> AuthorityResult<String> {
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some("mdreader-recovery-grant+jwt".into());
        jsonwebtoken::encode(&header, claims, &self.encoding).map_err(|_| AuthorityError::Custody)
    }
    pub(super) fn pair_grant(&self, claims: &impl serde::Serialize) -> AuthorityResult<String> {
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some("mdreader-pair-grant+jwt".into());
        header.jwk =
            Some(serde_json::from_value(self.jwk.clone()).map_err(|_| AuthorityError::Custody)?);
        jsonwebtoken::encode(&header, claims, &self.encoding).map_err(|_| AuthorityError::Custody)
    }
    pub(super) fn authorize(
        &self,
        identity: &VerifiedIdentity,
        method: &str,
        url: &str,
        body: &[u8],
    ) -> AuthorityResult<crate::transport::Authorization> {
        if now()? >= identity.expires || body.len() > crate::MAX_ENVELOPE_BYTES {
            return Err(AuthorityError::Identity);
        }
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some("mdreader-proof+jwt".into());
        header.jwk =
            Some(serde_json::from_value(self.jwk.clone()).map_err(|_| AuthorityError::Custody)?);
        let mut claims = serde_json::json!({"v":2,"htm":method,"htu":url,"ath":URL_SAFE_NO_PAD.encode(Sha256::digest(identity.token.as_bytes())),"bht":URL_SAFE_NO_PAD.encode(Sha256::digest(body)),"iat":now()?,"jti":uuid::Uuid::new_v4().to_string()});
        let candidate = method == "POST"
            && url::Url::parse(url).ok().is_some_and(|u| {
                matches!(
                    u.path(),
                    "/v1/enrollment/bootstrap" | "/v1/pairings" | "/v1/enrollment/recover"
                )
            });
        if candidate {
            claims["lth"] = serde_json::json!(
                URL_SAFE_NO_PAD.encode(Sha256::digest(identity.login_token.as_bytes()))
            );
        }
        let jwt = Zeroizing::new(
            jsonwebtoken::encode(&header, &claims, &self.encoding)
                .map_err(|_| AuthorityError::Custody)?,
        );
        let auth = crate::transport::Authorization::new(&identity.token, &jwt)
            .map_err(|_| AuthorityError::Identity)?;
        if candidate {
            auth.with_login_token(&identity.login_token)
                .map_err(|_| AuthorityError::Identity)
        } else {
            Ok(auth)
        }
    }
}
