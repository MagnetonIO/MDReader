use super::*;
use p256::{PublicKey, SecretKey};
use sha2::{Digest, Sha256};

/// Public-only P-256 JWK, exact fields; serde rejects duplicate members.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicJwk {
    kty: String,
    crv: String,
    x: String,
    y: String,
}
impl PublicJwk {
    pub fn parse(json: &[u8]) -> AuthorityResult<Self> {
        if json.len() > 512 {
            return Err(AuthorityError::Bounds);
        }
        let value: Self = serde_json::from_slice(json).map_err(|_| AuthorityError::Custody)?;
        value.point()?;
        Ok(value)
    }
    fn point(&self) -> AuthorityResult<PublicKey> {
        if self.kty != "EC" || self.crv != "P-256" {
            return Err(AuthorityError::Custody);
        }
        let x = decode_fixed::<32>(&self.x)?;
        let y = decode_fixed::<32>(&self.y)?;
        let mut bytes = [0; 65];
        bytes[0] = 4;
        bytes[1..33].copy_from_slice(&x);
        bytes[33..].copy_from_slice(&y);
        PublicKey::from_sec1_bytes(&bytes).map_err(|_| AuthorityError::Custody)
    }
    pub fn thumbprint(&self) -> AuthorityResult<String> {
        self.point()?;
        // RFC7638 lexicographic required-member order.
        let canonical = serde_json::json!({"crv":self.crv,"kty":self.kty,"x":self.x,"y":self.y});
        Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(
            serde_json::to_vec(&canonical).map_err(|_| AuthorityError::Custody)?,
        )))
    }
}
pub struct WrappingKey(Zeroizing<[u8; 32]>);
impl WrappingKey {
    pub fn generate() -> AuthorityResult<Self> {
        for _ in 0..16 {
            let mut bytes = Zeroizing::new([0; 32]);
            getrandom::fill(bytes.as_mut()).map_err(|_| AuthorityError::Custody)?;
            if SecretKey::from_slice(bytes.as_slice()).is_ok() {
                return Ok(Self(bytes));
            }
        }
        Err(AuthorityError::Custody)
    }
    pub fn from_bytes(bytes: Zeroizing<[u8; 32]>) -> AuthorityResult<Self> {
        SecretKey::from_slice(bytes.as_slice()).map_err(|_| AuthorityError::Custody)?;
        Ok(Self(bytes))
    }
    pub fn public(&self) -> AuthorityResult<PublicJwk> {
        let key = SecretKey::from_slice(self.0.as_slice()).map_err(|_| AuthorityError::Custody)?;
        let point = p256::Sec1Point::from(key.public_key());
        let bytes = point.as_bytes();
        if bytes.len() != 65 || bytes[0] != 4 {
            return Err(AuthorityError::Custody);
        }
        Ok(PublicJwk {
            kty: "EC".into(),
            crv: "P-256".into(),
            x: URL_SAFE_NO_PAD.encode(&bytes[1..33]),
            y: URL_SAFE_NO_PAD.encode(&bytes[33..]),
        })
    }
    pub(in super::super) fn secret(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(self.0.to_vec())
    }
    pub(super) fn agree(
        &self,
        other: &PublicJwk,
        context: &PairContext,
    ) -> AuthorityResult<Zeroizing<[u8; 32]>> {
        let private =
            SecretKey::from_slice(self.0.as_slice()).map_err(|_| AuthorityError::Custody)?;
        let public = other.point()?;
        let z = p256::ecdh::diffie_hellman(private.to_nonzero_scalar(), public.as_affine());
        // RFC7518§4.6.2 ConcatKDF: counter || Z || length-prefixed alg/U/V || keybits.
        let mut hash = Sha256::new();
        hash.update(1u32.to_be_bytes());
        hash.update(z.raw_secret_bytes());
        for data in [
            b"ECDH-ES+A256KW".as_slice(),
            &decode_bounded(&context.apu, 1024)?,
            &decode_bounded(&context.apv, 1024)?,
        ] {
            hash.update(
                u32::try_from(data.len())
                    .map_err(|_| AuthorityError::Bounds)?
                    .to_be_bytes(),
            );
            hash.update(data);
        }
        hash.update(256u32.to_be_bytes());
        let output = Zeroizing::new(hash.finalize());
        let mut key = Zeroizing::new([0; 32]);
        key.copy_from_slice(&output);
        Ok(key)
    }
}
pub(super) fn decode_bounded(value: &str, max: usize) -> AuthorityResult<Vec<u8>> {
    if value.len() > max.div_ceil(3) * 4 {
        return Err(AuthorityError::Bounds);
    }
    let bytes = crate::crypto::decode_canonical(value).map_err(|_| AuthorityError::Custody)?;
    if bytes.len() > max {
        return Err(AuthorityError::Bounds);
    }
    Ok(bytes)
}
pub(super) fn decode_fixed<const N: usize>(value: &str) -> AuthorityResult<[u8; N]> {
    decode_bounded(value, N)?
        .try_into()
        .map_err(|_| AuthorityError::Custody)
}
