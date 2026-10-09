#[path = "grant/scopes.rs"]
mod scopes;
use mdreader_provider::*;
pub use scopes::{
    DRIVE_FILE_SCOPE, DRIVE_READONLY_SCOPE, DROPBOX_EDIT_SCOPES, DROPBOX_READ_SCOPES,
};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zeroize::{Zeroize, ZeroizeOnDrop};

mod registration;
pub use registration::Registration;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DriveFile {
    pub id: RecordKey,
    pub resource_key: Option<OpaqueValue>,
}
impl fmt::Debug for DriveFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DriveFile([redacted])")
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DropboxFile {
    pub id: RecordKey,
    pub path: RecordKey,
}
impl fmt::Debug for DropboxFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DropboxFile([redacted])")
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(try_from = "SelectionWire", tag = "kind", rename_all = "snake_case")]
pub enum NativeSelection {
    DriveFiles { files: Vec<DriveFile> },
    DriveReadOnlyFolder { folder: RecordKey },
    DropboxExact { files: Vec<DropboxFile> },
    DropboxRoot { path: RecordKey, recursive: bool },
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SelectionWire {
    DriveFiles { files: Vec<DriveFile> },
    DriveReadOnlyFolder { folder: RecordKey },
    DropboxExact { files: Vec<DropboxFile> },
    DropboxRoot { path: RecordKey, recursive: bool },
}
impl TryFrom<SelectionWire> for NativeSelection {
    type Error = ProviderError;
    fn try_from(w: SelectionWire) -> Result<Self> {
        let selection = match w {
            SelectionWire::DriveFiles { files } => Self::DriveFiles { files },
            SelectionWire::DriveReadOnlyFolder { folder } => Self::DriveReadOnlyFolder { folder },
            SelectionWire::DropboxExact { files } => Self::DropboxExact { files },
            SelectionWire::DropboxRoot { path, recursive } => Self::DropboxRoot { path, recursive },
        };
        selection.validate()?;
        Ok(selection)
    }
}
fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.contains('\\')
        && !path.split('/').any(|p| p == "." || p == "..")
        && (path == "/" || !path.ends_with('/'))
}
impl NativeSelection {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::DriveFiles { files } => {
                SelectedScope::exact(files.iter().map(|f| f.id.clone()).collect())?;
                for file in files {
                    if file.id.as_str().len() > 256
                        || !file
                            .id
                            .as_str()
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                        || file.resource_key.as_ref().is_some_and(|key| {
                            key.as_str().len() > 512
                                || !key
                                    .as_str()
                                    .bytes()
                                    .all(|b| b.is_ascii_graphic() && !matches!(b, b',' | b'/'))
                        })
                    {
                        return Err(ProviderError::Invalid);
                    }
                }
            }
            Self::DriveReadOnlyFolder { folder } => {
                if folder.as_str().len() > 256
                    || !folder
                        .as_str()
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                {
                    return Err(ProviderError::Invalid);
                }
            }
            Self::DropboxExact { files } => {
                SelectedScope::exact(files.iter().map(|f| f.id.clone()).collect())?;
                for file in files {
                    if !file.id.as_str().starts_with("id:")
                        || file.id.as_str().len() < 4
                        || !valid_path(file.path.as_str())
                    {
                        return Err(ProviderError::Invalid);
                    }
                }
                if files
                    .iter()
                    .map(|f| &f.path)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != files.len()
                {
                    return Err(ProviderError::Invalid);
                }
            }
            Self::DropboxRoot { path, .. } => {
                if !valid_path(path.as_str()) {
                    return Err(ProviderError::Invalid);
                }
            }
        }
        Ok(())
    }
    pub fn provider(&self) -> ProviderKind {
        match self {
            Self::DriveFiles { .. } | Self::DriveReadOnlyFolder { .. } => ProviderKind::GoogleDrive,
            _ => ProviderKind::Dropbox,
        }
    }
    pub fn scope(&self) -> Result<SelectedScope> {
        self.validate()?;
        match self {
            Self::DriveFiles { files } => {
                SelectedScope::exact(files.iter().map(|f| f.id.clone()).collect())
            }
            Self::DropboxExact { files } => {
                SelectedScope::exact(files.iter().map(|f| f.id.clone()).collect())
            }
            Self::DriveReadOnlyFolder { folder } => Ok(SelectedScope::ScopedRoot {
                root: folder.clone(),
                recursive: folder.as_str() == "root",
            }),
            Self::DropboxRoot { path, recursive } => Ok(SelectedScope::ScopedRoot {
                root: path.clone(),
                recursive: *recursive,
            }),
        }
    }
    pub fn digest(&self) -> Result<[u8; 32]> {
        self.validate()?;
        Ok(body_hash(
            &serde_json::to_vec(self).map_err(|_| ProviderError::Invalid)?,
        ))
    }
}
impl fmt::Debug for NativeSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NativeSelection([redacted])")
    }
}
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SecretBundle {
    access_token: String,
    refresh_token: String,
    cursor_key: [u8; 32],
    #[zeroize(skip)]
    expires_at_unix_seconds: u64,
    #[zeroize(skip)]
    generation: u64,
}
impl SecretBundle {
    pub fn new(
        access_token: String,
        refresh_token: String,
        expires_at_unix_seconds: u64,
        cursor_key: [u8; 32],
    ) -> Result<Self> {
        let access_token = zeroize::Zeroizing::new(access_token);
        let refresh_token = zeroize::Zeroizing::new(refresh_token);
        let cursor_key = zeroize::Zeroizing::new(cursor_key);
        let valid = |token: &str| {
            !token.is_empty() && token.len() <= 8192 && !token.chars().any(char::is_control)
        };
        if !valid(&access_token)
            || !valid(&refresh_token)
            || expires_at_unix_seconds == 0
            || *cursor_key == [0; 32]
        {
            return Err(ProviderError::Invalid);
        }
        UNIX_EPOCH
            .checked_add(Duration::from_secs(expires_at_unix_seconds))
            .ok_or(ProviderError::Invalid)?;
        Ok(Self {
            access_token: access_token.to_string(),
            refresh_token: refresh_token.to_string(),
            expires_at_unix_seconds,
            cursor_key: *cursor_key,
            generation: 1,
        })
    }
    pub(crate) fn restore_generation(mut self, generation: u64) -> Result<Self> {
        if generation == 0 {
            return Err(ProviderError::Invalid);
        }
        self.generation = generation;
        Ok(self)
    }
    pub fn access_token(&self) -> &str {
        &self.access_token
    }
    pub fn refresh_token(&self) -> &str {
        &self.refresh_token
    }
    pub fn expires_at_unix_seconds(&self) -> u64 {
        self.expires_at_unix_seconds
    }
    pub fn expires_at(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(self.expires_at_unix_seconds)
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn cursor_key(&self) -> [u8; 32] {
        self.cursor_key
    }
}
impl fmt::Debug for SecretBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretBundle([redacted])")
    }
}
/// Native verified narrow authority plus secret custody. Intentionally has no
/// public Serialize/Deserialize implementation and never crosses a renderer.
#[derive(Clone)]
pub struct ProviderGrant {
    registration: Registration,
    account: AccountBinding,
    selection: NativeSelection,
    secret: SecretBundle,
    // None preserves the exact legacy v3 read contract/digest.
    verified_scopes: Option<String>,
}
impl ProviderGrant {
    pub fn new(
        registration: &Registration,
        account: AccountBinding,
        selection: NativeSelection,
        secret: SecretBundle,
    ) -> Result<Self> {
        selection.validate()?;
        if registration.provider() != selection.provider() {
            return Err(ProviderError::IdentityMismatch);
        }
        Ok(Self {
            registration: registration.clone(),
            account,
            selection,
            secret,
            verified_scopes: None,
        })
    }
    pub fn registration(&self) -> &Registration {
        &self.registration
    }
    pub fn account(&self) -> &AccountBinding {
        &self.account
    }
    pub fn selection(&self) -> &NativeSelection {
        &self.selection
    }
    pub fn secret(&self) -> &SecretBundle {
        &self.secret
    }
    pub fn configuration_digest(&self) -> Result<[u8; 32]> {
        if let Some(scopes) = self.verified_scopes() {
            return Ok(body_hash(
                &serde_json::to_vec(&(&self.registration, &self.account, &self.selection, scopes))
                    .map_err(|_| ProviderError::Invalid)?,
            ));
        }
        Ok(body_hash(
            &serde_json::to_vec(&(&self.registration, &self.account, &self.selection))
                .map_err(|_| ProviderError::Invalid)?,
        ))
    }
    pub fn refreshed(&self, access: String, refresh: Option<String>, expires: u64) -> Result<Self> {
        let mut secret = SecretBundle::new(
            access,
            refresh.unwrap_or_else(|| self.secret.refresh_token.clone()),
            expires,
            self.secret.cursor_key,
        )?;
        secret.generation = self
            .secret
            .generation
            .checked_add(1)
            .ok_or(ProviderError::TooLarge)?;
        let mut updated = Self::new(
            &self.registration,
            self.account.clone(),
            self.selection.clone(),
            secret,
        )?;
        updated.verified_scopes = self.verified_scopes.clone();
        Ok(updated)
    }
}
impl fmt::Debug for ProviderGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProviderGrant([redacted])")
    }
}
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CredentialReference(String);
impl CredentialReference {
    pub fn fresh() -> Self {
        Self(format!("native-provider-v3-{}", uuid::Uuid::new_v4()))
    }
    pub fn new(value: String) -> Result<Self> {
        let id = value
            .strip_prefix("native-provider-v3-")
            .ok_or(ProviderError::Invalid)?;
        let parsed = uuid::Uuid::parse_str(id).map_err(|_| ProviderError::Invalid)?;
        if parsed.get_version_num() != 4 || parsed.to_string() != id {
            return Err(ProviderError::Invalid);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for CredentialReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CredentialReference([redacted])")
    }
}
