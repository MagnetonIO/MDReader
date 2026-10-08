//! Standard P-256 JWK/PKCS8 conversion; never derive a replacement recovery key.
use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use p256::{
    pkcs8::{DecodePrivateKey, EncodePrivateKey},
    SecretKey,
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PrivateJwk {
    kty: String,
    crv: String,
    x: String,
    y: String,
    d: String,
}

impl Drop for PrivateJwk {
    fn drop(&mut self) {
        self.d.zeroize();
    }
}
impl PrivateJwk {
    pub(super) fn from_pkcs8(bytes: &[u8]) -> AuthorityResult<Self> {
        let key = SecretKey::from_pkcs8_der(bytes).map_err(|_| AuthorityError::Custody)?;
        let raw = Zeroizing::new(key.to_bytes());
        let signing = signing::SigningKey::load(bytes)?;
        Ok(Self {
            kty: "EC".into(),
            crv: "P-256".into(),
            x: signing.jwk["x"]
                .as_str()
                .ok_or(AuthorityError::Custody)?
                .into(),
            y: signing.jwk["y"]
                .as_str()
                .ok_or(AuthorityError::Custody)?
                .into(),
            d: URL_SAFE_NO_PAD.encode(raw.as_slice()),
        })
    }
    pub(super) fn to_pkcs8(&self) -> AuthorityResult<Zeroizing<Vec<u8>>> {
        if self.kty != "EC"
            || self.crv != "P-256"
            || self.d.len() != 43
            || self.x.len() != 43
            || self.y.len() != 43
        {
            return Err(AuthorityError::Custody);
        }
        let raw = Zeroizing::new(
            crate::crypto::decode_canonical(&self.d).map_err(|_| AuthorityError::Custody)?,
        );
        if raw.len() != 32 {
            return Err(AuthorityError::Custody);
        }
        let private = SecretKey::from_slice(&raw).map_err(|_| AuthorityError::Custody)?;
        let document = private
            .to_pkcs8_der()
            .map_err(|_| AuthorityError::Custody)?;
        let bytes = Zeroizing::new(document.as_bytes().to_vec());
        let expected = wrapping::PublicJwk::parse(
            &serde_json::to_vec(
                &serde_json::json!({"kty":self.kty,"crv":self.crv,"x":self.x,"y":self.y}),
            )
            .map_err(|_| AuthorityError::Custody)?,
        )?
        .thumbprint()?;
        if signing::SigningKey::load(&bytes)?.device != expected {
            return Err(AuthorityError::Custody);
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_private_jwk_pkcs8_ring_roundtrip_retains_exact_anchor() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/pairing-v1.json")).unwrap();
        let bytes = serde_json::to_vec(&fixture["recipientPrivateJwk"]).unwrap();
        let private: PrivateJwk = serde_json::from_slice(&bytes).unwrap();
        let pkcs8 = private.to_pkcs8().unwrap();
        let signing = signing::SigningKey::load(&pkcs8).unwrap();
        let public =
            serde_json::json!({"kty":private.kty,"crv":private.crv,"x":private.x,"y":private.y});
        assert_eq!(
            signing.device,
            wrapping::PublicJwk::parse(&serde_json::to_vec(&public).unwrap())
                .unwrap()
                .thumbprint()
                .unwrap()
        );
        let exported = PrivateJwk::from_pkcs8(&pkcs8).unwrap();
        assert_eq!(
            serde_json::to_value(&exported).unwrap(),
            fixture["recipientPrivateJwk"]
        );
        let duplicate = String::from_utf8(bytes)
            .unwrap()
            .replacen("{", "{\"d\":\"duplicate\",", 1);
        assert!(serde_json::from_str::<PrivateJwk>(&duplicate).is_err());
        let mut mismatch = exported;
        mismatch.x = URL_SAFE_NO_PAD.encode([0; 32]);
        assert!(mismatch.to_pkcs8().is_err());
    }
}
