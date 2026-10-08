//! Same public issuer/registration as desktop; exact mobile callback and stable
//! native profile UUID. Missing registration leaves local reading available.
use super::browser::Browser;
use mdreader_service::accounts::{AccountError, NativeSetup};
use mdreader_sync::{
    authority::{
        NativeAuthorityFactory, OidcClient, OidcEndpoints, ReviewedOidcRegistration,
        SessionController, SyncVaultClient,
    },
    transport::Transport,
};
use std::sync::{Arc, Mutex};

pub(super) fn setup(
    app: &tauri::AppHandle,
    profile: uuid::Uuid,
) -> Result<Option<NativeSetup>, AccountError> {
    let stage = option_env!("MDREADER_NATIVE_SYNC_STAGE");
    let redirect = option_env!("MDREADER_MOBILE_SYNC_REDIRECT_URI");
    if stage == Some("disabled") || redirect.is_none() {
        return Ok(None);
    }
    if !matches!(stage, Some("development" | "production")) {
        return Err(AccountError::Invalid);
    }
    let values = [
        option_env!("MDREADER_NATIVE_SYNC_ISSUER"),
        option_env!("MDREADER_NATIVE_SYNC_CLIENT_ID"),
        option_env!("MDREADER_NATIVE_SYNC_AUDIENCE"),
        option_env!("MDREADER_NATIVE_SYNC_AUTHORIZATION_ENDPOINT"),
        option_env!("MDREADER_NATIVE_SYNC_TOKEN_ENDPOINT"),
        option_env!("MDREADER_NATIVE_SYNC_JWKS_ENDPOINT"),
        option_env!("MDREADER_NATIVE_SYNC_ENDPOINT"),
        redirect,
    ];
    let [
        Some(issuer),
        Some(client),
        Some(audience),
        Some(authorization),
        Some(token),
        Some(jwks),
        Some(endpoint),
        Some(redirect),
    ] = values.map(|v| v.filter(|s| !s.is_empty() && s.len() <= 2048))
    else {
        return Err(AccountError::Invalid);
    };
    if redirect
        != format!(
            "{}.auth://callback/mdreader-sync",
            env!("MDREADER_NATIVE_APP_ID")
        )
    {
        return Err(AccountError::Invalid);
    }
    if !mdreader_mobile_transfer::available(app) {
        return Err(AccountError::Unsupported);
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
    )?;
    let namespace = format!("{}.{}", env!("MDREADER_NATIVE_APP_ID"), profile);
    #[cfg(target_os = "ios")]
    let vault = SyncVaultClient::new(Arc::new(mdreader_sync::authority::SyncKeyringVault::new(
        &namespace,
    )?))?;
    #[cfg(target_os = "android")]
    let vault = SyncVaultClient::new(Arc::new(mdreader_sync::authority::SyncPlatformVault::new(
        Arc::new(super::vault::Vault {
            app: app.clone(),
            namespace,
        }),
    )))?;
    Ok(Some(NativeSetup {
        factory: NativeAuthorityFactory::new(
            OidcClient::new(registration)?,
            vault,
            SessionController::new(),
        ),
        transport: Transport::new(endpoint).map_err(|_| AccountError::Invalid)?,
        browser: Arc::new(Browser {
            app: app.clone(),
            authorization: url::Url::parse(authorization).map_err(|_| AccountError::Invalid)?,
            redirect: redirect.into(),
            callback: Mutex::new(None),
        }),
        registered_redirect: redirect.into(),
        recovery: None,
    }))
}
