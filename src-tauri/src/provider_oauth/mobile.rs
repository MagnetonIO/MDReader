//! Native mobile callback custody over the same state/PKCE/selection exchange.
use super::*;

pub(crate) fn validate_redirect(registration: &Registration, value: &str) -> Result<Url> {
    let expected = match registration.provider() {
        ProviderKind::Dropbox => {
            format!("{}.auth://callback/dropbox", env!("MDREADER_NATIVE_APP_ID"))
        }
        ProviderKind::GoogleDrive => {
            // Google does not support this custom-scheme flow on Android.
            if cfg!(target_os = "android") {
                return Err(ProviderError::Unsupported);
            }
            let client = registration
                .client_id()
                .strip_suffix(".apps.googleusercontent.com")
                .filter(|v| {
                    !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
                .ok_or(ProviderError::Invalid)?;
            format!("com.googleusercontent.apps.{client}:/oauth2redirect")
        }
    };
    if value != expected {
        return Err(ProviderError::Invalid);
    }
    Url::parse(value).map_err(|_| ProviderError::Invalid)
}

struct Attempt {
    app: tauri::AppHandle,
    id: String,
    complete: bool,
}
impl Drop for Attempt {
    fn drop(&mut self) {
        if !self.complete {
            let app = self.app.clone();
            let id = self.id.clone();
            tauri::async_runtime::spawn(async move {
                mdreader_mobile_transfer::cancel_authentication(&app, &id).await;
            });
        }
    }
}
pub(crate) async fn authorize(
    app: &tauri::AppHandle,
    registration: Registration,
    redirect: &str,
    selection: Option<NativeSelection>,
    allow_editing: bool,
    transport: &dyn Transport,
    context: &IoContext,
) -> Result<CompletedAuthorization> {
    context.check()?;
    let _permit = AUTHORIZATION_GATE
        .try_acquire()
        .map_err(|_| ProviderError::Unavailable)?;
    if context.deadline().saturating_duration_since(Instant::now()) > Duration::from_secs(300) {
        return Err(ProviderError::Invalid);
    }
    let redirect = validate_redirect(&registration, redirect)?;
    let provider = match registration.provider() {
        ProviderKind::GoogleDrive => "google_drive",
        ProviderKind::Dropbox => "dropbox",
    };
    let pending = PendingAuthorization::with_redirect(
        registration,
        redirect.clone(),
        selection,
        allow_editing,
    )?;
    let mut attempt = Attempt {
        app: app.clone(),
        id: uuid::Uuid::new_v4().to_string(),
        complete: false,
    };
    let authorization = pending.authorization_url()?;
    let callback = bounded(context, async {
        mdreader_mobile_transfer::authenticate_provider(
            app,
            &attempt.id,
            authorization.as_str(),
            redirect.as_str(),
            provider,
        )
        .await
        .map_err(|error| match error {
            "cancelled" => ProviderError::Cancelled,
            "unsupported" => ProviderError::Unsupported,
            _ => ProviderError::Unavailable,
        })
    })
    .await?;
    attempt.complete = true;
    if callback.len() > MAX_CALLBACK_BYTES || !valid_percent_encoding(callback.as_bytes()) {
        return Err(ProviderError::Protocol);
    }
    let mut url = Url::parse(&callback).map_err(|_| ProviderError::Protocol)?;
    let query = Zeroizing::new(url.query().ok_or(ProviderError::Protocol)?.to_owned());
    if url.fragment().is_some() {
        return Err(ProviderError::Protocol);
    }
    url.set_query(None);
    if url != redirect {
        return Err(ProviderError::Unauthorized);
    }
    let target = Zeroizing::new(format!("{}?{}", redirect.path(), query.as_str()));
    let verified = pending.verify_callback(&target)?;
    pending.exchange(verified, transport, context).await
}
