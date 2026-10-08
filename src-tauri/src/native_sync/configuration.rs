//! Public build-time native registration. No discovery, tokens or renderer input.
use mdreader_service::accounts::{AccountError, NativeSetup};
use mdreader_sync::{
    authority::{
        NativeAuthorityFactory, OidcClient, OidcEndpoints, ReviewedOidcRegistration,
        SessionController, SyncKeyringVault, SyncVaultClient,
    },
    transport::Transport,
};
use std::{path::Path, sync::Arc};

pub(super) struct Configuration<'a> {
    pub stage: Option<&'a str>,
    pub issuer: Option<&'a str>,
    pub client: Option<&'a str>,
    pub audience: Option<&'a str>,
    pub authorization: Option<&'a str>,
    pub token: Option<&'a str>,
    pub jwks: Option<&'a str>,
    pub endpoint: Option<&'a str>,
    pub redirect: Option<&'a str>,
}
impl Configuration<'_> {
    pub(super) fn build(
        &self,
        profile: &Path,
        identifier: &str,
    ) -> Result<Option<NativeSetup>, AccountError> {
        let values = [
            self.issuer,
            self.client,
            self.audience,
            self.authorization,
            self.token,
            self.jwks,
            self.endpoint,
            self.redirect,
        ];
        if self.stage == Some("disabled")
            || (self.stage.is_none() && values.iter().all(Option::is_none))
        {
            return Ok(None);
        }
        if !matches!(self.stage, Some("development" | "production")) {
            return Err(AccountError::Invalid);
        }
        let values = values.map(|v| v.filter(|s| !s.is_empty() && s.len() <= 2048));
        let [Some(issuer), Some(client), Some(audience), Some(authorization), Some(token), Some(jwks), Some(endpoint), Some(redirect)] =
            values
        else {
            return Err(AccountError::Invalid);
        };
        let redirect_url = url::Url::parse(redirect).map_err(|_| AccountError::Invalid)?;
        if redirect_url.scheme() != "http"
            || redirect_url.host_str() != Some("127.0.0.1")
            || redirect_url.port().is_none_or(|p| p == 0)
            || redirect_url.path() != "/mdreader-sync/callback"
            || !redirect_url.username().is_empty()
            || redirect_url.password().is_some()
            || redirect_url.query().is_some()
            || redirect_url.fragment().is_some()
        {
            return Err(AccountError::Invalid);
        }
        let registration = ReviewedOidcRegistration::new(
            issuer,
            client,
            OidcEndpoints {
                authorization: authorization.into(),
                token: token.into(),
                jwks: jwks.into(),
            },
            endpoint,
            audience,
        )
        .map_err(|_| AccountError::Invalid)?;
        let browser = super::Browser::new(authorization)?;
        let transport = Transport::new(endpoint).map_err(|_| AccountError::Invalid)?;
        let namespace = namespace(profile, identifier)?;
        let vault = SyncVaultClient::new(Arc::new(SyncKeyringVault::new(&namespace)?))?;
        Ok(Some(NativeSetup {
            factory: NativeAuthorityFactory::new(
                OidcClient::new(registration)?,
                vault,
                SessionController::new(),
            ),
            transport,
            browser: Arc::new(browser),
            registered_redirect: redirect.into(),
            recovery: None,
        }))
    }
}

fn namespace(profile: &Path, identifier: &str) -> Result<String, AccountError> {
    if identifier.is_empty()
        || identifier.len() > 128
        || !identifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
    {
        return Err(AccountError::Invalid);
    }
    let root = profile.canonicalize().map_err(|_| AccountError::Storage)?;
    Ok(format!(
        "{identifier}.{}",
        mdreader_sync::blob_digest(root.as_os_str().as_encoded_bytes())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Configuration<'static> {
        Configuration {
            stage: Some("development"),
            issuer: Some("https://identity.example.invalid/"),
            client: Some("public-native-client"),
            audience: Some("https://api.example.invalid/sync"),
            authorization: Some("https://identity.example.invalid/authorize"),
            token: Some("https://identity.example.invalid/oauth/token"),
            jwks: Some("https://identity.example.invalid/.well-known/jwks.json"),
            endpoint: Some("https://sync.example.invalid"),
            redirect: Some("http://127.0.0.1:43821/mdreader-sync/callback"),
        }
    }
    #[test]
    fn absent_partial_and_invalid_registration_keep_native_authority_closed() {
        let root = tempfile::tempdir().unwrap();
        let absent = Configuration {
            stage: None,
            issuer: None,
            client: None,
            audience: None,
            authorization: None,
            token: None,
            jwks: None,
            endpoint: None,
            redirect: None,
        };
        assert!(matches!(
            absent.build(root.path(), "dev.mdreader.test"),
            Ok(None)
        ));
        let mut c = config();
        c.client = None;
        assert!(matches!(
            c.build(root.path(), "dev.mdreader.test"),
            Err(AccountError::Invalid)
        ));
        c.stage = Some("disabled");
        assert!(matches!(
            c.build(root.path(), "dev.mdreader.test"),
            Ok(None)
        ));
        c = config();
        c.redirect = Some("http://localhost:43821/mdreader-sync/callback");
        assert!(matches!(
            c.build(root.path(), "dev.mdreader.test"),
            Err(AccountError::Invalid)
        ));
        c = config();
        c.endpoint = Some("https://sync.example.invalid/unreviewed-path");
        assert!(matches!(
            c.build(root.path(), "dev.mdreader.test"),
            Err(AccountError::Invalid)
        ));
        c = config();
        assert!(c.build(root.path(), "dev.mdreader.test").unwrap().is_some());
        // Construction only: no browser request, network exchange or OS credential read/write.
    }
    #[test]
    fn vault_namespace_is_bound_to_exact_native_profile_and_app() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let first = namespace(a.path(), "dev.mdreader.test").unwrap();
        assert_eq!(first, namespace(a.path(), "dev.mdreader.test").unwrap());
        assert_ne!(first, namespace(b.path(), "dev.mdreader.test").unwrap());
        assert_ne!(first, namespace(a.path(), "dev.mdreader.other").unwrap());
        assert!(namespace(a.path(), "../untrusted").is_err());
        assert!(!first.contains(a.path().to_str().unwrap()));
    }
}
