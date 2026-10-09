//! Reader links may open bounded web/mail destinations, never paths or programs.
use serde::Serialize;
use specta::Type;
use tauri::AppHandle;
#[cfg(mobile)]
use tauri_plugin_opener::OpenerExt;

#[derive(Debug, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExternalOpenErrorKind {
    Invalid,
    Unavailable,
}
#[derive(Debug, Serialize, Type)]
pub struct ExternalOpenError {
    pub kind: ExternalOpenErrorKind,
}
fn failure(kind: ExternalOpenErrorKind) -> ExternalOpenError {
    ExternalOpenError { kind }
}
fn destination(raw: &str) -> Result<String, ExternalOpenError> {
    let invalid = || failure(ExternalOpenErrorKind::Invalid);
    if raw.is_empty() || raw.len() > 4096 || raw.chars().any(char::is_control) {
        return Err(invalid());
    }
    let url = url::Url::parse(raw).map_err(|_| invalid())?;
    let allowed = match url.scheme() {
        "http" | "https" => {
            url.host_str().is_some() && url.username().is_empty() && url.password().is_none()
        }
        "mailto" => url.cannot_be_a_base() && !url.path().is_empty(),
        #[cfg(mobile)]
        "tel" => {
            url.cannot_be_a_base()
                && !url.path().is_empty()
                && url.query().is_none()
                && url.fragment().is_none()
        }
        _ => false,
    };
    if !allowed {
        return Err(invalid());
    }
    Ok(url.into())
}
fn open_with(
    raw: &str,
    open: impl FnOnce(String) -> Result<(), ExternalOpenError>,
) -> Result<(), ExternalOpenError> {
    open(destination(raw)?)
}

#[tauri::command]
#[specta::specta]
pub async fn reader_open_external(app: AppHandle, url: String) -> Result<(), ExternalOpenError> {
    tauri::async_runtime::spawn_blocking(move || {
        open_with(&url, |destination| {
            #[cfg(mobile)]
            let result = app.opener().open_url(destination, None::<String>);
            #[cfg(not(mobile))]
            let result = {
                let _ = app;
                tauri_plugin_opener::open_url(destination, None::<&str>)
            };
            result.map_err(|_| failure(ExternalOpenErrorKind::Unavailable))
        })
    })
    .await
    .map_err(|_| failure(ExternalOpenErrorKind::Unavailable))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn external_reader_links_allow_only_bounded_web_or_mail_destinations() {
        for target in [
            "https://example.test/guide?q=a#next",
            "http://localhost:8080/",
            "mailto:reader@example.test?subject=Reading",
        ] {
            let mut called = false;
            open_with(target, |value| {
                assert_eq!(value, target);
                called = true;
                Ok(())
            })
            .unwrap();
            assert!(called);
        }
        for target in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,x",
            "mdreader://read",
            "ssh://example.test",
            "relative.md",
            "//example.test",
            "https://user:password@example.test/",
            "https://example.test/\nnext",
            "mailto:",
            "",
        ] {
            assert_eq!(
                open_with(target, |_| panic!("invalid link reached opener"))
                    .unwrap_err()
                    .kind,
                ExternalOpenErrorKind::Invalid
            );
        }
        assert!(open_with(
            &format!("https://example.test/{}", "a".repeat(4096)),
            |_| panic!("overlong link reached opener")
        )
        .is_err());
        assert_eq!(
            open_with("https://example.test/", |_| Err(failure(
                ExternalOpenErrorKind::Unavailable
            )))
            .unwrap_err()
            .kind,
            ExternalOpenErrorKind::Unavailable
        );
    }
}
