//! Native service composition. Public metadata stays compiled; custody stays native.
mod configuration;
#[cfg(target_os = "macos")]
mod recovery;
use mdreader_service::accounts::{AccountError, CacheRoots, NativeSetup};
use mdreader_sync::authority::{AuthorityError, AuthorityResult, NativeBrowser};
use std::{future::Future, path::Path, pin::Pin};
use url::Url;

pub(crate) fn setup(profile: &Path) -> Result<Option<NativeSetup>, AccountError> {
    configuration::Configuration {
        stage: option_env!("MDREADER_NATIVE_SYNC_STAGE"),
        issuer: option_env!("MDREADER_NATIVE_SYNC_ISSUER"),
        client: option_env!("MDREADER_NATIVE_SYNC_CLIENT_ID"),
        audience: option_env!("MDREADER_NATIVE_SYNC_AUDIENCE"),
        authorization: option_env!("MDREADER_NATIVE_SYNC_AUTHORIZATION_ENDPOINT"),
        token: option_env!("MDREADER_NATIVE_SYNC_TOKEN_ENDPOINT"),
        jwks: option_env!("MDREADER_NATIVE_SYNC_JWKS_ENDPOINT"),
        endpoint: option_env!("MDREADER_NATIVE_SYNC_ENDPOINT"),
        redirect: option_env!("MDREADER_NATIVE_SYNC_REDIRECT_URI"),
    }
    .build(profile, env!("MDREADER_NATIVE_APP_ID"))
}
/// Desktop portable documents/assets belong to this explicitly selected owner
/// directory. Mobile supplies its already selected roots when embedding an owner.
pub(crate) fn roots(profile: &Path) -> CacheRoots {
    CacheRoots {
        documents: profile.join("device-markdown-v1"),
        assets: profile.join("device-assets-v1"),
    }
}

struct Browser {
    authorization: Url,
}
impl Browser {
    fn new(endpoint: &str) -> Result<Self, AccountError> {
        Ok(Self {
            authorization: Url::parse(endpoint).map_err(|_| AccountError::Invalid)?,
        })
    }
}
impl NativeBrowser for Browser {
    fn open<'a>(
        &'a self,
        value: &'a str,
    ) -> Pin<Box<dyn Future<Output = AuthorityResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let url = Url::parse(value).map_err(|_| AuthorityError::Configuration)?;
            if url.origin() != self.authorization.origin()
                || url.path() != self.authorization.path()
            {
                return Err(AuthorityError::Configuration);
            }
            crate::native_browser::open(&url)
                .await
                .map_err(|e| match e {
                    crate::native_browser::Error::Busy => AuthorityError::Busy,
                    crate::native_browser::Error::Invalid => AuthorityError::Configuration,
                    crate::native_browser::Error::Unavailable => AuthorityError::Interrupted,
                })
        })
    }
}
