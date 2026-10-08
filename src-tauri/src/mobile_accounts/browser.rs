use mdreader_sync::authority::{AuthorityError, AuthorityResult, NativeBrowser};
use std::{future::Future, pin::Pin, sync::Mutex};
use zeroize::Zeroizing;

pub(super) struct Browser {
    pub app: tauri::AppHandle,
    pub authorization: url::Url,
    pub redirect: String,
    pub callback: Mutex<Option<Zeroizing<String>>>,
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
impl NativeBrowser for Browser {
    fn open<'a>(
        &'a self,
        value: &'a str,
    ) -> Pin<Box<dyn Future<Output = AuthorityResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let url = url::Url::parse(value).map_err(|_| AuthorityError::Configuration)?;
            if url.origin() != self.authorization.origin()
                || url.path() != self.authorization.path()
                || url
                    .query_pairs()
                    .filter(|(k, _)| k == "redirect_uri")
                    .map(|(_, v)| v.into_owned())
                    .collect::<Vec<_>>()
                    != [self.redirect.clone()]
            {
                return Err(AuthorityError::Configuration);
            }
            self.callback
                .lock()
                .map_err(|_| AuthorityError::Interrupted)?
                .take();
            let mut attempt = Attempt {
                app: self.app.clone(),
                id: uuid::Uuid::new_v4().to_string(),
                complete: false,
            };
            let result = mdreader_mobile_transfer::authenticate(
                &self.app,
                &attempt.id,
                value,
                &self.redirect,
            )
            .await;
            attempt.complete = matches!(&result, Ok(_) | Err("cancelled"));
            let callback = result.map_err(|error| match error {
                "cancelled" => AuthorityError::Cancelled,
                "unsupported" => AuthorityError::Unsupported,
                _ => AuthorityError::Interrupted,
            })?;
            *self
                .callback
                .lock()
                .map_err(|_| AuthorityError::Interrupted)? = Some(callback);
            Ok(())
        })
    }
    fn callback<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = AuthorityResult<Zeroizing<String>>> + Send + 'a>> {
        Box::pin(async move {
            self.callback
                .lock()
                .map_err(|_| AuthorityError::Interrupted)?
                .take()
                .ok_or(AuthorityError::Identity)
        })
    }
}
