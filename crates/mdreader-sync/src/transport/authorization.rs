use super::*;
use reqwest::header::HeaderValue;

/// Sensitive headers have no Debug/Serialize/getters; only the transport consumes them.
pub struct Authorization {
    pub(super) bearer: HeaderValue,
    pub(super) proof: HeaderValue,
    #[cfg(feature = "native-authority")]
    login_token: Option<HeaderValue>,
}
impl Authorization {
    #[cfg(feature = "native-authority")]
    pub(crate) fn into_headers(self) -> (HeaderValue, HeaderValue, Option<HeaderValue>) {
        (self.bearer, self.proof, self.login_token)
    }
    #[cfg(feature = "native-authority")]
    pub(crate) fn with_login_token(mut self, token: &str) -> TransportResult<Self> {
        if token.is_empty() || token.len() >= 16 * 1024 {
            return Err(TransportError::Authority);
        }
        let mut header = HeaderValue::from_str(token).map_err(|_| TransportError::Authority)?;
        header.set_sensitive(true);
        self.login_token = Some(header);
        Ok(self)
    }
    pub fn new(bearer_token: &str, proof_jwt: &str) -> TransportResult<Self> {
        if bearer_token.len() > 16 * 1024
            || bearer_token.is_empty()
            || proof_jwt.len() > 4096
            || proof_jwt.is_empty()
        {
            return Err(TransportError::Authority);
        }
        let bearer_text = zeroize::Zeroizing::new(format!("Bearer {bearer_token}"));
        let mut bearer =
            HeaderValue::from_str(&bearer_text).map_err(|_| TransportError::Authority)?;
        let mut proof = HeaderValue::from_str(proof_jwt).map_err(|_| TransportError::Authority)?;
        bearer.set_sensitive(true);
        proof.set_sensitive(true);
        Ok(Self {
            bearer,
            proof,
            #[cfg(feature = "native-authority")]
            login_token: None,
        })
    }
}
