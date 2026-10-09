use super::super::*;
pub(in crate::native_providers) fn compiled() -> NativeOwnerConfig {
    let mut registrations = Vec::new();
    let mut diagnostics = [None, None];
    match (
        option_env!("MDREADER_MOBILE_DRIVE_CLIENT_ID"),
        option_env!("MDREADER_MOBILE_DRIVE_REDIRECT_URI"),
    ) {
        (Some(id), Some(redirect)) => match Registration::google_drive(id.into()).and_then(|r| {
            provider_oauth::validate_mobile_redirect(&r, redirect)?;
            Ok(r)
        }) {
            Ok(r) => registrations.push(r),
            Err(_) => diagnostics[0] = Some(
                "Mobile Google authorization registration is invalid or unsupported on this host"
                    .into(),
            ),
        },
        _ => {
            diagnostics[0] =
                Some("Mobile Google authorization registration is not configured".into())
        }
    }
    let mut dropbox_redirect = None;
    match (
        option_env!("MDREADER_MOBILE_DROPBOX_CLIENT_ID"),
        option_env!("MDREADER_MOBILE_DROPBOX_REDIRECT_URI"),
    ) {
        (Some(id), Some(redirect)) => match Registration::dropbox_selected_folder(id.into())
            .and_then(|r| {
                provider_oauth::validate_mobile_redirect(&r, redirect)?;
                Ok(r)
            }) {
            Ok(r) => {
                registrations.push(r);
                dropbox_redirect = Some(redirect.into());
            }
            Err(_) => {
                diagnostics[1] = Some("Mobile Dropbox authorization registration is invalid".into())
            }
        },
        _ => {
            diagnostics[1] =
                Some("Mobile Dropbox authorization registration is not configured".into())
        }
    }
    NativeOwnerConfig {
        registrations,
        oauth: OAuthConfig {
            dropbox_registered_redirect: dropbox_redirect,
        },
        diagnostics,
    }
}
