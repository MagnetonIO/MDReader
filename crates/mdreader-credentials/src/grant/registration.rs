//! Native registration identities; old App Folder encoding remains unchanged.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DropboxAccess {
    SelectedFolder,
}

/// Configured by the native owner/build, never a renderer attestation.
#[derive(Clone, Serialize, Deserialize)]
#[serde(try_from = "RegistrationWire")]
pub struct Registration {
    provider: ProviderKind,
    client_id: String,
    owner_registration_digest: Option<[u8; 32]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dropbox_access: Option<DropboxAccess>,
    // Native token-endpoint configuration. It is not grant authority and must
    // not change persistent registration identity or enter serialized custody.
    #[serde(skip)]
    google_desktop_secret: Option<std::sync::Arc<zeroize::Zeroizing<String>>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationWire {
    provider: ProviderKind,
    client_id: String,
    owner_registration_digest: Option<[u8; 32]>,
    #[serde(default)]
    dropbox_access: Option<DropboxAccess>,
}
impl TryFrom<RegistrationWire> for Registration {
    type Error = ProviderError;
    fn try_from(w: RegistrationWire) -> Result<Self> {
        match (w.provider, w.owner_registration_digest, w.dropbox_access) {
            (ProviderKind::GoogleDrive, None, None) => Self::google_drive(w.client_id),
            (ProviderKind::Dropbox, Some(digest), None) => {
                Self::dropbox_app_folder(w.client_id, digest)
            }
            (ProviderKind::Dropbox, None, Some(DropboxAccess::SelectedFolder)) => {
                Self::dropbox_selected_folder(w.client_id)
            }
            _ => Err(ProviderError::Invalid),
        }
    }
}
impl Registration {
    pub fn google_drive(client_id: String) -> Result<Self> {
        if client_id.len() > 512
            || !client_id.ends_with(".apps.googleusercontent.com")
            || !client_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self {
            provider: ProviderKind::GoogleDrive,
            client_id,
            owner_registration_digest: None,
            dropbox_access: None,
            google_desktop_secret: None,
        })
    }
    /// Digest identifies reviewed native app-registration metadata establishing
    /// App Folder access. OAuth scopes and a user checkbox are not evidence.
    pub fn dropbox_app_folder(
        client_id: String,
        owner_registration_digest: [u8; 32],
    ) -> Result<Self> {
        if client_id.is_empty()
            || client_id.len() > 128
            || !client_id.bytes().all(|b| b.is_ascii_alphanumeric())
            || owner_registration_digest == [0; 32]
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self {
            provider: ProviderKind::Dropbox,
            client_id,
            owner_registration_digest: Some(owner_registration_digest),
            dropbox_access: None,
            google_desktop_secret: None,
        })
    }
    /// Fresh native OAuth with an application-enforced folder selection. This
    /// does not claim provider-enforced App Folder access. The selected path,
    /// verified account and exact OAuth scopes are bound separately to the grant.
    pub fn dropbox_selected_folder(client_id: String) -> Result<Self> {
        if client_id.is_empty()
            || client_id.len() > 128
            || !client_id.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self {
            provider: ProviderKind::Dropbox,
            client_id,
            owner_registration_digest: None,
            dropbox_access: Some(DropboxAccess::SelectedFolder),
            google_desktop_secret: None,
        })
    }
    /// Google desktop registrations may require this token-endpoint parameter
    /// even with PKCE. Native apps cannot treat it as proof of confidential
    /// client identity. It is excluded from Debug, serialization and digests.
    pub fn with_google_desktop_client_secret(mut self, value: String) -> Result<Self> {
        let value = zeroize::Zeroizing::new(value);
        if self.provider != ProviderKind::GoogleDrive
            || value.is_empty()
            || value.len() > 512
            || !value.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(ProviderError::Invalid);
        }
        self.google_desktop_secret = Some(std::sync::Arc::new(value));
        Ok(self)
    }
    pub fn google_desktop_client_secret(&self) -> Option<&str> {
        self.google_desktop_secret
            .as_deref()
            .map(|value| value.as_str())
    }
    pub fn provider(&self) -> ProviderKind {
        self.provider
    }
    pub fn client_id(&self) -> &str {
        &self.client_id
    }
    pub fn digest(&self) -> [u8; 32] {
        body_hash(&serde_json::to_vec(self).expect("fixed registration"))
    }
}
impl fmt::Debug for Registration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registration")
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_token_parameter_never_changes_or_serializes_grant_identity() {
        let original =
            Registration::google_drive("synthetic.apps.googleusercontent.com".into()).unwrap();
        let configured = original
            .clone()
            .with_google_desktop_client_secret("synthetic-desktop-parameter".into())
            .unwrap();
        assert_eq!(configured.digest(), original.digest());
        assert_eq!(
            serde_json::to_value(&configured).unwrap(),
            serde_json::to_value(&original).unwrap()
        );
        assert!(!format!("{configured:?}").contains("synthetic-desktop-parameter"));
        let restored: Registration =
            serde_json::from_value(serde_json::to_value(&configured).unwrap()).unwrap();
        assert!(restored.google_desktop_client_secret().is_none());
        assert_eq!(
            configured.google_desktop_client_secret(),
            Some("synthetic-desktop-parameter")
        );
        assert!(Registration::dropbox_selected_folder("syntheticapp".into())
            .unwrap()
            .with_google_desktop_client_secret("synthetic".into())
            .is_err());
        let mut injected = serde_json::to_value(&original).unwrap();
        injected["google_desktop_secret"] = serde_json::json!("synthetic-injection");
        assert!(serde_json::from_value::<Registration>(injected).is_err());
    }
    #[test]
    fn old_registration_encoding_and_digest_remain_stable() {
        let digest = vec![7; 32];
        let original = serde_json::json!({
            "provider": "dropbox", "client_id": "syntheticapp",
            "owner_registration_digest": digest,
        });
        let restored: Registration = serde_json::from_value(original.clone()).unwrap();
        assert_eq!(serde_json::to_value(&restored).unwrap(), original);
        assert_eq!(
            restored.digest(),
            Registration::dropbox_app_folder("syntheticapp".into(), [7; 32])
                .unwrap()
                .digest()
        );
    }
    #[test]
    fn selected_folder_is_distinct_and_cannot_claim_app_folder_isolation() {
        let selected = Registration::dropbox_selected_folder("syntheticapp".into()).unwrap();
        let encoded = serde_json::to_value(&selected).unwrap();
        let restored: Registration = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(selected.digest(), restored.digest());
        assert_ne!(
            selected.digest(),
            Registration::dropbox_app_folder("syntheticapp".into(), [7; 32])
                .unwrap()
                .digest()
        );
        let mut mixed = encoded;
        mixed["owner_registration_digest"] = serde_json::json!([7; 32].to_vec());
        assert!(serde_json::from_value::<Registration>(mixed).is_err());
    }
}
