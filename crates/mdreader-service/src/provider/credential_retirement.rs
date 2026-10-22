//! Best-effort provider revocation of a grant already deleted from local custody.
//! Every failure is reported as `Failed`; nothing here can undo a local disconnect.
use super::*;
use mdreader_credentials::{ProviderGrant, Registration};
use mdreader_provider::transport::{self, Method, Request, Transport};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt,
    time::{Duration, Instant, SystemTime},
};
use zeroize::Zeroizing;

const REVOKE_BUDGET: Duration = Duration::from_secs(10);
/// Leaves the coordinator time to reply after a slow provider.
const REPLY_MARGIN: Duration = Duration::from_secs(2);
const GOOGLE_REVOKE: &str = "https://oauth2.googleapis.com/revoke";
const DROPBOX_TOKEN: &str = "https://api.dropboxapi.com/oauth2/token";
const DROPBOX_REVOKE: &str = "https://api.dropboxapi.com/2/auth/token/revoke";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Revocation {
    Revoked,
    Failed,
    NotAttempted,
}

/// A grant removed from the vault and held only long enough to revoke it.
pub struct RetiredCredential(ProviderGrant);
impl RetiredCredential {
    pub fn new(grant: ProviderGrant) -> Self {
        Self(grant)
    }
}
impl fmt::Debug for RetiredCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RetiredCredential(..)")
    }
}

pub(super) async fn revoke(
    retired: RetiredCredential,
    registrations: &[Registration],
    transport: &dyn Transport,
    context: &IoContext,
) -> Revocation {
    let Ok(context) = bounded(context) else {
        return Revocation::Failed;
    };
    let grant = retired.0;
    let result = match grant.registration().provider() {
        ProviderKind::GoogleDrive => revoke_google(&grant, transport, &context).await,
        ProviderKind::Dropbox => revoke_dropbox(&grant, registrations, transport, &context).await,
    };
    match result {
        Ok(true) => Revocation::Revoked,
        Ok(false) | Err(_) => Revocation::Failed,
    }
}

fn bounded(context: &IoContext) -> Result<IoContext> {
    let deadline = context
        .deadline()
        .checked_sub(REPLY_MARGIN)
        .ok_or(ProviderError::Timeout)?
        .min(Instant::now() + REVOKE_BUDGET);
    IoContext::new(
        Uuid::new_v4(),
        deadline,
        1,
        64 * 1024,
        context.cancellation().clone(),
    )
}

/// Revoking the refresh token ends the whole Google authorization.
async fn revoke_google(
    grant: &ProviderGrant,
    transport: &dyn Transport,
    context: &IoContext,
) -> Result<bool> {
    let form = Zeroizing::new(
        url::form_urlencoded::Serializer::new(String::new())
            .append_pair("token", grant.secret().refresh_token())
            .finish(),
    );
    let response = transport::send(
        transport,
        Request::new(
            Method::Post,
            GOOGLE_REVOKE.into(),
            BTreeMap::from([(
                "Content-Type".into(),
                "application/x-www-form-urlencoded".into(),
            )]),
            form.as_bytes().to_vec(),
        )?,
        context,
        None,
    )
    .await?;
    Ok(response.status() == 200)
}

/// Dropbox revokes the authorization behind a live access token, so an expired
/// one is exchanged first. The exchanged token is never written back to custody.
async fn revoke_dropbox(
    grant: &ProviderGrant,
    registrations: &[Registration],
    transport: &dyn Transport,
    context: &IoContext,
) -> Result<bool> {
    let access = if grant.secret().expires_at() > SystemTime::now() + Duration::from_secs(60) {
        Zeroizing::new(grant.secret().access_token().to_owned())
    } else {
        let registration = registrations
            .iter()
            .find(|r| r.digest() == grant.registration().digest())
            .ok_or(ProviderError::Unauthorized)?;
        exchange_dropbox(grant, registration, transport, context).await?
    };
    let response = transport::send(
        transport,
        Request::new(
            Method::Post,
            DROPBOX_REVOKE.into(),
            BTreeMap::from([
                (
                    "Authorization".into(),
                    format!("Bearer {}", access.as_str()),
                ),
                ("Content-Type".into(), "application/json".into()),
            ]),
            b"null".to_vec(),
        )?,
        context,
        None,
    )
    .await?;
    Ok(response.status() == 200)
}

async fn exchange_dropbox(
    grant: &ProviderGrant,
    registration: &Registration,
    transport: &dyn Transport,
    context: &IoContext,
) -> Result<Zeroizing<String>> {
    let form = Zeroizing::new(
        url::form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "refresh_token")
            .append_pair("refresh_token", grant.secret().refresh_token())
            .append_pair("client_id", registration.client_id())
            .finish(),
    );
    let response = transport::send(
        transport,
        Request::new(
            Method::Post,
            DROPBOX_TOKEN.into(),
            BTreeMap::from([(
                "Content-Type".into(),
                "application/x-www-form-urlencoded".into(),
            )]),
            form.as_bytes().to_vec(),
        )?,
        context,
        None,
    )
    .await?;
    if response.status() != 200 {
        return Err(ProviderError::ReconnectRequired);
    }
    #[derive(Deserialize)]
    struct Token {
        access_token: String,
    }
    let body = Zeroizing::new(response.into_body());
    let mut token: Token = serde_json::from_slice(&body).map_err(|_| ProviderError::Protocol)?;
    Ok(Zeroizing::new(std::mem::take(&mut token.access_token)))
}
