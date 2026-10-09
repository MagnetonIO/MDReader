//! Native public registration configuration, separate from credential custody.
use super::{NativeProviderKind, RegistrationAvailability};
use crate::provider_oauth::{validate_redirect, OAuthConfig};
use mdreader_credentials::Registration;
use mdreader_provider::{ProviderError, Result};

/// No secret values or registration metadata are runtime renderer configuration.
pub(crate) struct NativeOwnerConfig {
    pub(super) registrations: Vec<Registration>,
    pub(super) oauth: OAuthConfig,
    pub(super) diagnostics: [Option<String>; 2],
}
impl NativeOwnerConfig {
    pub(super) fn registration(&self, kind: NativeProviderKind) -> Option<Registration> {
        self.registrations
            .iter()
            .find(|r| NativeProviderKind::from(r.provider()) == kind)
            .cloned()
    }
    pub(super) fn availability(&self) -> Vec<RegistrationAvailability> {
        [NativeProviderKind::GoogleDrive, NativeProviderKind::Dropbox]
            .into_iter()
            .enumerate()
            .map(|(index, provider)| RegistrationAvailability {
                provider,
                configured: self.registration(provider).is_some(),
                supported: super::platform_supported(provider),
                diagnostic: if !super::platform_supported(provider) {
                    Some("Native provider authorization is unsupported on this host".into())
                } else {
                    self.diagnostics[index].clone()
                },
            })
            .collect()
    }
}

fn app_folder_registration(id: &str, digest: &str) -> Result<Registration> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ProviderError::Invalid);
    }
    let mut bytes = [0; 32];
    for (i, pair) in digest.as_bytes().chunks_exact(2).enumerate() {
        bytes[i] = u8::from_str_radix(
            std::str::from_utf8(pair).map_err(|_| ProviderError::Invalid)?,
            16,
        )
        .map_err(|_| ProviderError::Invalid)?;
    }
    Registration::dropbox_app_folder(id.into(), bytes)
}

pub(super) fn config_with_access(
    drive: Option<&str>,
    dropbox: Option<&str>,
    attestation: Option<&str>,
    redirect: Option<&str>,
    access: Option<&str>,
) -> NativeOwnerConfig {
    let mut registrations = Vec::new();
    let mut diagnostics = [None, None];
    match drive {
        Some(id) => match Registration::google_drive(id.into()) {
            Ok(r) => registrations.push(r),
            Err(_) => diagnostics[0] = Some("Native Google registration is invalid".into()),
        },
        None => diagnostics[0] = Some("Native Google registration is not configured".into()),
    }
    let mut dropbox_redirect = None;
    match (dropbox, redirect) {
        (None, None) if attestation.is_none() && access.is_none() => {
            diagnostics[1] = Some("Native Dropbox registration is not configured".into());
        }
        (Some(id), Some(uri)) => {
            let parsed = validate_redirect(uri).and_then(|_| match (access, attestation) {
                (Some("selected_folder"), None) => Registration::dropbox_selected_folder(id.into()),
                (None | Some("app_folder"), Some(digest)) => app_folder_registration(id, digest),
                _ => Err(ProviderError::Invalid),
            });
            match parsed {
                Ok(registration) => {
                    registrations.push(registration);
                    dropbox_redirect = Some(uri.into());
                }
                Err(_) => {
                    diagnostics[1] = Some(
                        if access.is_none() && attestation.is_none() {
                            "Native Dropbox registration is incomplete"
                        } else {
                            "Native Dropbox registration is invalid"
                        }
                        .into(),
                    )
                }
            }
        }
        _ => diagnostics[1] = Some("Native Dropbox registration is incomplete".into()),
    }
    NativeOwnerConfig {
        registrations,
        oauth: OAuthConfig {
            dropbox_registered_redirect: dropbox_redirect,
        },
        diagnostics,
    }
}

#[cfg(test)]
pub(super) fn config_from(
    drive: Option<&str>,
    dropbox: Option<&str>,
    attestation: Option<&str>,
    redirect: Option<&str>,
) -> NativeOwnerConfig {
    config_with_access(drive, dropbox, attestation, redirect, None)
}

#[cfg(not(mobile))]
pub(super) fn compiled_config() -> NativeOwnerConfig {
    // Reuse public app identities, never legacy credentials or broad grants.
    // A native override is authoritative, including invalid/empty values.
    let legacy_dropbox = option_env!("MDREADER_DROPBOX_CLIENT_ID");
    let native_dropbox = option_env!("MDREADER_NATIVE_DROPBOX_CLIENT_ID");
    let attestation = option_env!("MDREADER_NATIVE_DROPBOX_APP_FOLDER_REGISTRATION_SHA256");
    let access = option_env!("MDREADER_NATIVE_DROPBOX_ACCESS_TYPE").or_else(|| {
        (native_dropbox.is_none() && legacy_dropbox.is_some() && attestation.is_none())
            .then_some("selected_folder")
    });
    config_with_access(
        option_env!("MDREADER_NATIVE_DRIVE_CLIENT_ID")
            .or(option_env!("MDREADER_GOOGLE_DRIVE_CLIENT_ID")),
        native_dropbox.or(legacy_dropbox),
        attestation,
        option_env!("MDREADER_NATIVE_DROPBOX_REDIRECT_URI")
            .or(option_env!("MDREADER_DROPBOX_REDIRECT_URI")),
        access,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_folder_reuses_registered_callback_without_an_app_folder_claim() {
        let configured = config_with_access(
            None,
            Some("syntheticapp"),
            None,
            Some("http://127.0.0.1:53682/oauth/dropbox"),
            Some("selected_folder"),
        );
        assert_eq!(configured.registrations.len(), 1);
        assert!(configured.diagnostics[1].is_none());
        for redirect in [
            "https://example.com/oauth/dropbox",
            "http://127.0.0.1:53682/other",
            "http://127.0.0.1:53682/oauth/dropbox?injected=true",
        ] {
            assert!(config_with_access(
                None,
                Some("syntheticapp"),
                None,
                Some(redirect),
                Some("selected_folder")
            )
            .registrations
            .is_empty());
        }
        assert!(config_with_access(
            None,
            Some("syntheticapp"),
            Some(&"7".repeat(64)),
            Some("http://127.0.0.1:53682/oauth/dropbox"),
            Some("selected_folder")
        )
        .registrations
        .is_empty());
    }
}

#[cfg(mobile)]
pub(super) fn compiled_config() -> NativeOwnerConfig {
    super::mobile::configuration::compiled()
}
