//! Lossless compatibility metadata for retained legacy Library rows.
//! No OAuth, vault, SDK catalog, provider sync or network operation lives here.
use serde::{Deserialize, Serialize};
use specta::Type;
use std::{collections::HashSet, fmt};
const MAX_CLOUD_SOURCES: usize = 2;
const MAX_DRIVE_SELECTIONS: usize = 512;
pub(crate) const CLOUD_DOCUMENT_MAPPING_VERSION: &str = "mdreader.Document.cloud-mapping/v2";
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CloudProvider {
    GoogleDrive,
    Dropbox,
}

/// Versioned native OAuth authority retained with each cloud source record.
///
/// Legacy records intentionally default to V1 so a persisted account never
/// gains document-write authority merely by loading an older registry.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CloudAuthorizationProfile {
    /// The original read-only connection capability.
    #[default]
    ReadOnlyV1,
    /// The explicit Markdown read/write consent capability.
    MarkdownReadWriteV2,
}

/// Native-only declaration of a provider's completed Markdown write gate.
///
/// A write-capable OAuth profile does not imply that the provider's guarded
/// mutation path has passed its live acceptance gate. Task 9 changes only the
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct DriveItemSelection {
    id: String,
    resource_key: Option<String>,
    folder: bool,
    #[serde(default)]
    root: bool,
}

impl fmt::Debug for DriveItemSelection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DriveItemSelection")
            .field("id", &"[redacted-provider-id]")
            .field(
                "resource_key",
                &self
                    .resource_key
                    .as_ref()
                    .map(|_| "[redacted-resource-key]"),
            )
            .field("folder", &self.folder)
            .field("root", &self.root)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum CloudSourceConfig {
    GoogleDrive { selections: Vec<DriveItemSelection> },
    Dropbox,
}

impl fmt::Debug for CloudSourceConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GoogleDrive { selections } => formatter
                .debug_struct("GoogleDrive")
                .field("selection_count", &selections.len())
                .finish(),
            Self::Dropbox => formatter.write_str("Dropbox"),
        }
    }
}

impl CloudSourceConfig {
    fn provider(&self) -> CloudProvider {
        match self {
            Self::GoogleDrive { .. } => CloudProvider::GoogleDrive,
            Self::Dropbox => CloudProvider::Dropbox,
        }
    }

    fn validate(&self) -> Result<(), CloudSourceError> {
        match self {
            Self::GoogleDrive { selections }
                if selections.is_empty() || selections.len() > MAX_DRIVE_SELECTIONS =>
            {
                Err(CloudSourceError::InvalidConfiguration)
            }
            Self::GoogleDrive { selections } => {
                let mut ids = HashSet::new();
                for selection in selections {
                    let valid_id = !selection.id.is_empty()
                        && selection.id.len() <= 256
                        && selection.id.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
                        });
                    let valid_resource_key = selection.resource_key.as_ref().is_none_or(|key| {
                        !key.is_empty()
                            && key.len() <= 512
                            && key
                                .bytes()
                                .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b',' | b'/'))
                    });
                    if !valid_id || !valid_resource_key || !ids.insert(selection.id.as_str()) {
                        return Err(CloudSourceError::InvalidConfiguration);
                    }
                    if selection.root && !selection.folder {
                        return Err(CloudSourceError::InvalidConfiguration);
                    }
                }
                Ok(())
            }
            Self::Dropbox => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CloudSourceRecord {
    id: String,
    provider: CloudProvider,
    label: String,
    enabled: bool,
    credential_reference: String,
    config: CloudSourceConfig,
    #[serde(default)]
    authorization_profile: CloudAuthorizationProfile,
    #[serde(default)]
    contract: Option<crate::library::LibrarySourceContract>,
}

impl CloudSourceRecord {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn provider(&self) -> CloudProvider {
        self.provider
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn contract(&self) -> Option<&crate::library::LibrarySourceContract> {
        self.contract.as_ref()
    }

    pub(crate) fn refresh_contract(&mut self) {
        self.contract = Some(crate::library::LibrarySourceContract::cloud(self.provider));
    }
}

pub(crate) fn validate_cloud_source_records(
    sources: &[CloudSourceRecord],
) -> Result<(), CloudSourceError> {
    if sources.len() > MAX_CLOUD_SOURCES {
        return Err(CloudSourceError::CorruptRegistry);
    }
    let mut ids = HashSet::new();
    let mut providers = HashSet::new();
    let mut references = HashSet::new();
    for source in sources {
        let valid_id = source.id.len() == 64
            && source
                .id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
        let valid_reference = valid_credential_reference(&source.credential_reference);
        if !valid_id
            || !valid_reference
            || source.label != source.provider.label()
            || source.config.provider() != source.provider
            || source.config.validate().is_err()
            || source.contract.as_ref()
                != Some(&crate::library::LibrarySourceContract::cloud(
                    source.provider,
                ))
            || !ids.insert(source.id.as_str())
            || !providers.insert(source.provider)
            || !references.insert(source.credential_reference.as_str())
        {
            return Err(CloudSourceError::CorruptRegistry);
        }
    }
    Ok(())
}

impl CloudProvider {
    fn label(self) -> &'static str {
        match self {
            Self::GoogleDrive => "Google Drive",
            Self::Dropbox => "Dropbox",
        }
    }
}
fn valid_credential_reference(reference: &str) -> bool {
    !reference.is_empty()
        && reference.len() <= 256
        && !reference.chars().any(char::is_control)
        && !["cloud-cleanup-google-drive", "cloud-cleanup-dropbox"].contains(&reference)
}
#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum CloudSourceError {
    InvalidConfiguration,
    CorruptRegistry,
    UnsupportedVersion { found: u32, expected: u32 },
    PersistFailed,
}
impl fmt::Display for CloudSourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "Legacy Library metadata unavailable; preserve the offline profile before migration",
        )
    }
}
impl std::error::Error for CloudSourceError {}
