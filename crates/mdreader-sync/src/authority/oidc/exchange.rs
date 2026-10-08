//! Bounded token exchange; preserves both login and API token purposes.
use super::*;
use crate::authority::access::verify_pair;
use serde::Deserialize;
use zeroize::Zeroize;
impl OidcClient {
    pub(super) async fn exchange(
        &self,
        pending: &PendingLogin,
        code: &str,
    ) -> AuthorityResult<VerifiedIdentity> {
        let form = Zeroizing::new(
            url::form_urlencoded::Serializer::new(String::new())
                .append_pair("grant_type", "authorization_code")
                .append_pair("client_id", &self.registration.client_id)
                .append_pair("redirect_uri", pending.redirect.as_str())
                .append_pair("code", code)
                .append_pair("code_verifier", &pending.verifier)
                .finish()
                .into_bytes(),
        );
        let response = self
            .http
            .send(
                reqwest::Method::POST,
                self.registration.token.clone(),
                form,
                None,
                "application/x-www-form-urlencoded",
                64 * 1024,
            )
            .await?;
        let mut wire: TokenResponse =
            serde_json::from_slice(&response).map_err(|_| AuthorityError::Identity)?;
        if !wire.token_type.eq_ignore_ascii_case("Bearer")
            || wire.access_token.is_empty()
            || wire.access_token.len() > 16 * 1024
            || wire.id_token.len() >= 16 * 1024
        {
            return Err(AuthorityError::Identity);
        }
        if wire.expires_in == 0 || wire.expires_in > 86400 {
            return Err(AuthorityError::Identity);
        }
        let token = Zeroizing::new(std::mem::take(&mut wire.id_token));
        let access = Zeroizing::new(std::mem::take(&mut wire.access_token));
        let response_expires = now()?
            .checked_add(wire.expires_in)
            .ok_or(AuthorityError::Identity)?;
        let jwks = self
            .http
            .send(
                reqwest::Method::GET,
                self.registration.jwks.clone(),
                Zeroizing::new(Vec::new()),
                None,
                "application/json",
                64 * 1024,
            )
            .await?;
        if Instant::now() >= pending.expires {
            return Err(AuthorityError::Identity);
        }
        verify_pair(
            &self.registration,
            token,
            access,
            &jwks,
            &pending.nonce,
            pending.created,
            response_expires,
        )
    }
}
#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
    access_token: String,
    token_type: String,
    expires_in: u64,
    #[serde(default)]
    refresh_token: Option<String>,
}
impl Drop for TokenResponse {
    fn drop(&mut self) {
        self.id_token.zeroize();
        self.access_token.zeroize();
        if let Some(refresh) = &mut self.refresh_token {
            refresh.zeroize();
        }
    }
}
