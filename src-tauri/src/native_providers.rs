//! Native command boundary for narrow provider custody and the shared owner.
//! Renderer DTOs never contain credentials, account IDs, remote record keys,
//! token validators, cursors, registration attestations, or credential references.
use crate::{
    provider_oauth::{self, CompletedAuthorization, OAuthConfig, SystemBrowser},
    reader::ReaderBackend,
};
use mdreader_credentials::{
    CredentialReference, KeyringVault, NativeSelection, Registration, VaultClient,
};
use mdreader_provider::{
    transport::{ReqwestTransport, Transport},
    DisplayText, DocumentProvider, IoContext, LibraryId, ProviderAuthority, ProviderError,
    ProviderKind, Result as ProviderResult, SourceId,
};
use mdreader_service::provider::{
    NativeFactory, ProjectionReceipt, ProviderFactory, ProviderLinkKind, ProviderLinkResolution,
    ProviderRequest, SelectionSummary, SourceConfiguration, SourceSummary,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use uuid::Uuid;

const MAX_PENDING: usize = 32;
const MAX_LEDGER_BYTES: u64 = 128 * 1024;
mod configuration;
mod creation;
mod disconnect;
pub use disconnect::{RevocationDto, SignInDisposalDto};
mod draft_recovery;
mod drive;
mod editing;
mod reading;
pub use draft_recovery::RecoveredDraftDto;
#[cfg(mobile)]
mod mobile;
#[cfg(not(mobile))]
pub(crate) mod preservation;
#[cfg(mobile)]
#[path = "native_providers/mobile/operation.rs"]
mod preservation;
#[cfg(test)]
use configuration::config_from;
use configuration::{compiled_config, NativeOwnerConfig};

fn platform_supported(provider: NativeProviderKind) -> bool {
    cfg!(all(unix, not(mobile)))
        || cfg!(target_os = "ios")
        || cfg!(target_os = "android") && provider == NativeProviderKind::Dropbox
}

const MAX_MARKDOWN: usize = 2 * 1024 * 1024;
const RPC_BUDGET: Duration = Duration::from_secs(35);

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum NativeProviderError {
    AuthorizationFailed {
        error: Box<NativeProviderError>,
        diagnostic: provider_oauth::OAuthDiagnostic,
    },
    Unsupported,
    Unconfigured,
    Busy,
    Invalid,
    InvalidResponse,
    Storage,
    ServiceUnavailable,
    Timeout,
    Cancelled,
    ReconnectRequired,
    Unauthorized,
    Conflict,
    TooLarge,
    UnknownWrite {
        operation_id: String,
    },
    LocalPublicationPending {
        operation_id: String,
    },
}
impl std::fmt::Display for NativeProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Storage => "Native provider state could not be opened",
            Self::Invalid => "Native provider owner configuration is invalid",
            _ => "Native provider operation failed",
        })
    }
}
impl From<ProviderError> for NativeProviderError {
    fn from(error: ProviderError) -> Self {
        match error {
            ProviderError::Unsupported => Self::Unsupported,
            ProviderError::Invalid | ProviderError::InvalidUtf8 => Self::Invalid,
            ProviderError::Unauthorized | ProviderError::IdentityMismatch => Self::Unauthorized,
            ProviderError::ReconnectRequired | ProviderError::CredentialUnavailable => {
                Self::ReconnectRequired
            }
            ProviderError::Conflict => Self::Conflict,
            ProviderError::TooLarge => Self::TooLarge,
            ProviderError::Timeout => Self::Timeout,
            ProviderError::Cancelled => Self::Cancelled,
            ProviderError::UnknownWrite { operation_id } => Self::UnknownWrite {
                operation_id: operation_id.to_string(),
            },
            ProviderError::Persistence => Self::Storage,
            ProviderError::Protocol => Self::InvalidResponse,
            ProviderError::Unavailable
            | ProviderError::NotFound
            | ProviderError::RateLimited { .. }
            | ProviderError::ResetCursor => Self::ServiceUnavailable,
            ProviderError::LocalPublicationPending => Self::LocalPublicationPending {
                operation_id: String::new(),
            },
        }
    }
}
impl From<provider_oauth::AuthorizationFailure> for NativeProviderError {
    fn from(failure: provider_oauth::AuthorizationFailure) -> Self {
        Self::AuthorizationFailed {
            error: Box::new(failure.error.into()),
            diagnostic: failure.diagnostic,
        }
    }
}
type Result<T> = std::result::Result<T, NativeProviderError>;
#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeProviderKind {
    GoogleDrive,
    Dropbox,
}
impl From<ProviderKind> for NativeProviderKind {
    fn from(kind: ProviderKind) -> Self {
        match kind {
            ProviderKind::GoogleDrive => Self::GoogleDrive,
            ProviderKind::Dropbox => Self::Dropbox,
        }
    }
}
impl From<NativeProviderKind> for ProviderKind {
    fn from(kind: NativeProviderKind) -> Self {
        match kind {
            NativeProviderKind::GoogleDrive => Self::GoogleDrive,
            NativeProviderKind::Dropbox => Self::Dropbox,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DropboxSelection {
    pub path: String,
    pub recursive: bool,
}
#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationAvailability {
    pub provider: NativeProviderKind,
    pub configured: bool,
    pub supported: bool,
    pub diagnostic: Option<String>,
}
#[derive(Debug, Serialize, specta::Type)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ScopeDto {
    ExactDocuments {
        count: u32,
    },
    ScopedRoot {
        recursive: bool,
        path: Option<String>,
    },
    #[serde(rename = "drive_readonly_folder")]
    DriveReadOnlyFolder {
        account_wide: bool,
        folder_id: String,
    },
    DriveFolder {
        folder_id: String,
        recursive: bool,
    },
    ImportedLegacyBroad,
}
#[derive(Debug, Serialize, specta::Type)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum CoverageDto {
    NeverIndexed,
    Partial { reason: String },
    MetadataCompleteSelected { observed_at_ms: f64 },
    CompleteSelected { observed_at_ms: f64 },
}
#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitiesDto {
    pub read: bool,
    pub list: bool,
    pub changes: bool,
    pub assets: bool,
    pub guarded_write: bool,
    pub browse_folders: bool,
}
#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSourceDto {
    pub source_id: String,
    pub reader_source_id: Option<String>,
    pub name: String,
    pub kind: NativeProviderKind,
    pub enabled: bool,
    pub selected_scope: ScopeDto,
    pub capabilities: CapabilitiesDto,
    pub health: String,
    pub coverage: CoverageDto,
    pub metadata_listing: Option<MetadataListingDto>,
}
#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ListingCompletenessDto {
    NeverObserved,
    Partial,
    CompleteSelected,
}
#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MetadataListingDto {
    pub known_documents: u32,
    pub completeness: ListingCompletenessDto,
    pub observed_at_ms: Option<f64>,
    pub refresh_pending: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionDto {
    pub job_id: String,
    pub reader_source_id: String,
    pub path: String,
    pub revision: String,
}
#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDocumentDto {
    pub document_id: String,
    pub name: String,
    pub display_path: String,
    pub modified_at_ms: Option<f64>,
    pub size_bytes: Option<f64>,
    pub cached: bool,
    pub cached_observed_at_ms: Option<f64>,
    pub admitted: bool,
    pub projection: Option<ProjectionDto>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionPhase {
    VaultPending,
    AdmissionPending,
}
#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PendingAdmissionDto {
    pub attempt_id: String,
    pub provider: NativeProviderKind,
    pub name: String,
    pub source_id: Option<String>,
    pub phase: AdmissionPhase,
}
#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSnapshot {
    pub availability: Vec<RegistrationAvailability>,
    pub sources: Vec<ProviderSourceDto>,
    pub pending_admissions: Vec<PendingAdmissionDto>,
    pub error: Option<NativeProviderError>,
}
#[derive(Debug, Serialize, specta::Type)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AdmissionOutcome {
    Admitted {
        source_id: String,
        enabled: bool,
        replayed: bool,
    },
    Pending {
        attempt_id: String,
        reason: NativeProviderError,
    },
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum NativeLinkKind {
    Relative,
    Wikilink,
}
#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct NativeLinkCandidate {
    pub source_id: String,
    pub document_id: String,
    pub title: String,
}
#[derive(Debug, Serialize, specta::Type)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum NativeLinkResolution {
    Resolved {
        candidate: NativeLinkCandidate,
        projection: ProjectionDto,
        line: u32,
        heading: Option<String>,
    },
    SelectedUncached {
        candidate: NativeLinkCandidate,
        heading: Option<String>,
    },
    Ambiguous {
        candidates: Vec<NativeLinkCandidate>,
        heading: Option<String>,
    },
    Missing,
    HeadingMissing {
        candidate: NativeLinkCandidate,
        heading: String,
    },
}
#[derive(Debug, Deserialize, specta::Type)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum NativeProviderRequest {
    RecoveredDrafts {
        offset: u32,
        limit: u32,
    },
    ReadRecoveredDraft {
        draft_id: String,
        revision: String,
    },
    BrowseFolders {
        source_id: String,
        folder_id: String,
        cursor: Option<String>,
    },
    ResolveLink {
        source_id: String,
        path: String,
        revision: String,
        kind: NativeLinkKind,
        target: String,
    },
    Documents {
        source_id: String,
        offset: u32,
        limit: u32,
    },
    Refresh {
        source_id: String,
    },
    FetchCurrent {
        source_id: String,
        document_id: String,
    },
    ReadCached {
        source_id: String,
        document_id: String,
    },
    ReadDocument {
        source_id: String,
        path: String,
        revision: String,
    },
    ReadCurrentDocument {
        source_id: String,
        path: String,
        revision: String,
    },
    Disable {
        source_id: String,
    },
    Forget {
        source_id: String,
    },
    /// Remove the source, its downloaded copies and its saved sign-in.
    Disconnect {
        source_id: String,
    },
    BeginEditExact {
        source_id: String,
        path: String,
        revision: String,
    },
    CreateDocument {
        source_id: String,
        operation_id: String,
        filename: String,
        markdown: String,
    },
    InspectCreate {
        source_id: String,
        operation_id: String,
    },
    InspectSave {
        source_id: String,
        operation_id: String,
    },
    BeginEdit {
        source_id: String,
        document_id: String,
    },
    PrepareSave {
        source_id: String,
        edit_id: String,
        markdown: String,
    },
    DispatchSave {
        source_id: String,
        operation_id: String,
    },
    Mutations {
        source_id: String,
    },
    Repair {
        source_id: String,
    },
}
#[derive(Debug, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum MutationStateDto {
    Absent,
    Prepared,
    Dispatched,
    Unknown,
    Accepted,
    Published,
    Conflict,
}
#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MutationDto {
    pub operation_id: String,
    pub state: MutationStateDto,
}
#[derive(Debug, Serialize, specta::Type)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum NativeProviderResult {
    RecoveredDrafts {
        drafts: Vec<RecoveredDraftDto>,
        offset: u32,
        next_offset: Option<u32>,
    },
    ReadRecoveredDraft {
        draft: RecoveredDraftDto,
        markdown: String,
    },
    BrowseFolders {
        folders: Vec<drive::FolderDto>,
        next_cursor: Option<String>,
    },
    ResolveLink {
        resolution: NativeLinkResolution,
    },
    Documents {
        documents: Vec<ProviderDocumentDto>,
        offset: u32,
        next_offset: Option<u32>,
    },
    Refresh {
        pages: u32,
        metadata_complete: bool,
    },
    FetchCurrent {
        projection: ProjectionDto,
    },
    ReadCached {
        projection: ProjectionDto,
    },
    ReadDocument {
        source_id: String,
        path: String,
        revision: String,
        markdown: String,
        line_start: u32,
        line_end: u32,
    },
    ReadCurrentDocument {
        source_id: String,
        path: String,
        revision: String,
        markdown: String,
        line_start: u32,
        line_end: u32,
    },
    Disable {
        disabled: bool,
        forgotten: bool,
    },
    Forget {
        disabled: bool,
        forgotten: bool,
    },
    Disconnect {
        credential: SignInDisposalDto,
        revocation: RevocationDto,
    },
    BeginEditExact {
        write_source_id: String,
        edit_id: String,
        markdown: String,
        expires_at_ms: f64,
        projection: ProjectionDto,
    },
    CreateDocument {
        operation_id: String,
        state: MutationStateDto,
        projection: Option<ProjectionDto>,
        diagnostic: Option<String>,
    },
    InspectCreate {
        operation_id: String,
        state: MutationStateDto,
        projection: Option<ProjectionDto>,
        diagnostic: Option<String>,
    },
    InspectSave {
        operation_id: String,
        state: MutationStateDto,
        projection: Option<ProjectionDto>,
    },
    BeginEdit {
        edit_id: String,
        markdown: String,
        expires_at_ms: f64,
    },
    PrepareSave {
        operation_id: String,
        state: MutationStateDto,
    },
    DispatchSave {
        projection: ProjectionDto,
    },
    Mutations {
        mutations: Vec<MutationDto>,
    },
    Repair {
        recovered_unknown: u32,
        published: u32,
        conflicts: u32,
        pending: u32,
    },
}

fn profile_namespace(profile: &Path, identifier: &str) -> Result<String> {
    if identifier.is_empty()
        || identifier.len() > 128
        || !identifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
    {
        return Err(NativeProviderError::Invalid);
    }
    let canonical = profile
        .canonicalize()
        .map_err(|_| NativeProviderError::Storage)?;
    Ok(format!(
        "{identifier}.{}",
        mdreader_provider::hex(&Sha256::digest(canonical.as_os_str().as_encoded_bytes()))
    ))
}
struct UnconfiguredFactory;
#[async_trait::async_trait]
impl ProviderFactory for UnconfiguredFactory {
    async fn settle(&self, context: &IoContext) -> ProviderResult<()> {
        // This exact compiled factory owns no vault or provider kernels.
        context.check()
    }
    async fn describe(
        &self,
        _: LibraryId,
        _: SourceId,
        _: DisplayText,
        _: CredentialReference,
        _: &IoContext,
    ) -> ProviderResult<SourceConfiguration> {
        Err(ProviderError::Unsupported)
    }
    async fn open(
        &self,
        _: SourceConfiguration,
        _: Arc<dyn ProviderAuthority>,
        _: Vec<mdreader_provider::Metadata>,
        _: &IoContext,
    ) -> ProviderResult<Arc<dyn DocumentProvider>> {
        Err(ProviderError::Unsupported)
    }
}
/// Public lib shim and GUI use the exact same compiled configuration/namespace.
#[cfg(not(mobile))]
pub(crate) fn serve(
    directory: &Path,
    options: mdreader_service::ServiceOptions,
) -> mdreader_core::Result<()> {
    fs::create_dir_all(directory)?;
    let config = compiled_config();
    let factory: Arc<dyn ProviderFactory> = if config.registrations.is_empty() {
        Arc::new(UnconfiguredFactory)
    } else {
        let factory = (|| {
            let namespace = profile_namespace(directory, env!("MDREADER_NATIVE_APP_ID"))
                .map_err(|_| ProviderError::Invalid)?;
            let vault = VaultClient::new(Arc::new(KeyringVault::new(&namespace)?))?;
            NativeFactory::new(
                vault,
                config.registrations,
                Arc::new(ReqwestTransport::new()?),
            )
        })();
        match factory {
            Ok(factory) => Arc::new(factory),
            Err(_) => Arc::new(UnconfiguredFactory),
        }
    };
    mdreader_service::serve_with_native_accounts(
        directory,
        options,
        Some(factory),
        crate::native_sync::setup(directory),
        crate::native_sync::roots(directory),
    )
}

#[async_trait::async_trait]
trait Rpc: Send + Sync {
    async fn provider(&self, request: ProviderRequest) -> Result<Value>;
    async fn read(&self, projection: &ProjectionDto) -> Result<mdreader_core::Document>;
    async fn read_current(&self, _projection: &ProjectionDto) -> Result<mdreader_core::Document> {
        Err(NativeProviderError::Unsupported)
    }
}
#[cfg(not(mobile))]
struct DesktopRpc {
    backend: ReaderBackend,
    slots: Arc<Semaphore>,
}
#[cfg(not(mobile))]
impl DesktopRpc {
    async fn invoke<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        request: mdreader_service::Request,
    ) -> Result<T> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| NativeProviderError::Busy)?;
        let backend = self.backend.clone();
        // The blocking transport owns its permit even if the caller times out.
        // At most four abandoned calls can exist; none is retried automatically.
        let task = tauri::async_runtime::spawn_blocking(move || {
            let _permit = permit;
            backend.call_native(request)
        });
        let value = tokio::time::timeout(RPC_BUDGET, task)
            .await
            .map_err(|_| NativeProviderError::Timeout)?
            .map_err(|_| NativeProviderError::ServiceUnavailable)?
            .map_err(core_error)?;
        decode(value)
    }
}
fn core_error(error: mdreader_core::ReaderError) -> NativeProviderError {
    use mdreader_core::ReaderError as E;
    match error {
        E::Conflict => NativeProviderError::Conflict,
        E::Cancelled => NativeProviderError::Cancelled,
        E::ModelUnavailable => NativeProviderError::Unsupported,
        E::Invalid(_) => NativeProviderError::InvalidResponse,
        E::Sqlite(_) | E::NewerDatabase(_) => NativeProviderError::Storage,
        E::Unavailable | E::Source(_) | E::Io(_) => NativeProviderError::ServiceUnavailable,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderReply {
    result: Value,
    error: Option<ProviderError>,
}
#[async_trait::async_trait]
#[cfg(not(mobile))]
impl Rpc for DesktopRpc {
    async fn provider(&self, request: ProviderRequest) -> Result<Value> {
        let reply: ProviderReply = self
            .invoke(mdreader_service::Request::Provider { request })
            .await?;
        match reply.error {
            Some(error) if reply.result.is_null() => Err(error.into()),
            None => Ok(reply.result),
            _ => Err(NativeProviderError::InvalidResponse),
        }
    }
    async fn read(&self, p: &ProjectionDto) -> Result<mdreader_core::Document> {
        self.invoke(mdreader_service::Request::Read {
            source_id: p.reader_source_id.clone(),
            path: p.path.clone(),
            revision: Some(p.revision.clone()),
        })
        .await
    }
    async fn read_current(&self, p: &ProjectionDto) -> Result<mdreader_core::Document> {
        self.invoke(mdreader_service::Request::Read {
            source_id: p.reader_source_id.clone(),
            path: p.path.clone(),
            revision: None,
        })
        .await
    }
}
#[async_trait::async_trait]
trait Authorizer: Send + Sync {
    async fn authorize(
        &self,
        registration: Registration,
        selection: Option<NativeSelection>,
        allow_editing: bool,
    ) -> Result<CompletedAuthorization>;
}
#[cfg(not(mobile))]
struct Browser;
#[async_trait::async_trait]
#[cfg(not(mobile))]
impl SystemBrowser for Browser {
    async fn open(&self, url: &url::Url) -> ProviderResult<()> {
        crate::native_browser::open(url)
            .await
            .map_err(|error| match error {
                crate::native_browser::Error::Invalid => ProviderError::Invalid,
                crate::native_browser::Error::Busy | crate::native_browser::Error::Unavailable => {
                    ProviderError::Unavailable
                }
            })
    }
}

#[cfg(not(mobile))]
struct NativeAuthorizer {
    config: OAuthConfig,
    transport: Arc<dyn Transport>,
}
#[async_trait::async_trait]
#[cfg(not(mobile))]
impl Authorizer for NativeAuthorizer {
    async fn authorize(
        &self,
        registration: Registration,
        selection: Option<NativeSelection>,
        allow_editing: bool,
    ) -> Result<CompletedAuthorization> {
        let context = IoContext::new(
            Uuid::new_v4(),
            Instant::now() + Duration::from_secs(300),
            1,
            64 * 1024,
            Default::default(),
        )
        .map_err(NativeProviderError::from)?;
        provider_oauth::authorize_detailed(
            registration,
            selection,
            allow_editing,
            &self.config,
            &Browser,
            &*self.transport,
            &context,
        )
        .await
        .map_err(Into::into)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingAdmission {
    attempt_id: Uuid,
    provider: NativeProviderKind,
    name: DisplayText,
    source_id: Option<SourceId>,
    phase: AdmissionPhase,
    credential_reference: String,
    configuration_digest: [u8; 32],
    registration_digest: [u8; 32],
}
impl PendingAdmission {
    fn dto(&self) -> PendingAdmissionDto {
        PendingAdmissionDto {
            attempt_id: self.attempt_id.to_string(),
            provider: self.provider,
            name: self.name.as_str().into(),
            source_id: self.source_id.as_ref().map(|s| s.as_str().into()),
            phase: self.phase,
        }
    }
    fn request(&self) -> ProviderRequest {
        match &self.source_id {
            Some(source_id) => ProviderRequest::Reconnect {
                source_id: source_id.clone(),
                credential_reference: self.credential_reference.clone(),
                name: self.name.clone(),
            },
            None => ProviderRequest::Admit {
                credential_reference: self.credential_reference.clone(),
                name: self.name.clone(),
            },
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LedgerEnvelope {
    version: u8,
    namespace: String,
    attempts: Vec<PendingAdmission>,
}
struct CustodyLedger {
    root: mdreader_core::SourceRoot,
    namespace: String,
    // Keep the OS lease until this GUI owner exits. The headless service never
    // edits this ledger, and a second GUI must not overwrite pending custody.
    lease: File,
}
impl CustodyLedger {
    fn open(directory: &Path, identifier: &str) -> Result<Self> {
        fs::create_dir_all(directory).map_err(|_| NativeProviderError::Storage)?;
        let canonical = directory
            .canonicalize()
            .map_err(|_| NativeProviderError::Storage)?;
        Self::open_in_namespace(directory, profile_namespace(&canonical, identifier)?)
    }
    // Scope is supplied before any ledger load, including on mobile sandbox relocation.
    fn open_in_namespace(directory: &Path, namespace: String) -> Result<Self> {
        if namespace.is_empty() || namespace.len() > 200 || namespace.chars().any(char::is_control)
        {
            return Err(NativeProviderError::Storage);
        }
        fs::create_dir_all(directory).map_err(|_| NativeProviderError::Storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .map_err(|_| NativeProviderError::Storage)?;
        }
        let canonical = directory
            .canonicalize()
            .map_err(|_| NativeProviderError::Storage)?;
        let root = mdreader_core::SourceRoot::capture(canonical)
            .map_err(|_| NativeProviderError::Storage)?;
        let path = root.path.join("native-provider-admissions-v1.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lease = match options.open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata =
                    fs::symlink_metadata(&path).map_err(|_| NativeProviderError::Storage)?;
                if !metadata.is_file() {
                    return Err(NativeProviderError::Storage);
                }
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&path)
                    .map_err(|_| NativeProviderError::Storage)?
            }
            Err(_) => return Err(NativeProviderError::Storage),
        };
        let ledger = Self {
            root,
            namespace,
            lease,
        };
        ledger.verify()?;
        fs2::FileExt::try_lock_exclusive(&ledger.lease).map_err(|_| NativeProviderError::Busy)?;
        ledger.load()?;
        Ok(ledger)
    }
    fn verify(&self) -> Result<()> {
        self.root
            .verify()
            .map_err(|_| NativeProviderError::Storage)?;
        let path = self.root.path.join("native-provider-admissions-v1.lock");
        let path_metadata = fs::symlink_metadata(path).map_err(|_| NativeProviderError::Storage)?;
        let held_metadata = self
            .lease
            .metadata()
            .map_err(|_| NativeProviderError::Storage)?;
        if !path_metadata.is_file() || !held_metadata.is_file() {
            return Err(NativeProviderError::Storage);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if path_metadata.dev() != held_metadata.dev()
                || path_metadata.ino() != held_metadata.ino()
                || held_metadata.permissions().mode() & 0o077 != 0
            {
                return Err(NativeProviderError::Storage);
            }
        }
        Ok(())
    }
    fn load(&self) -> Result<Vec<PendingAdmission>> {
        self.verify()?;
        let path = self.root.path.join("native-provider-admissions-v1.json");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(_) => return Err(NativeProviderError::Storage),
        };
        if !metadata.is_file() || metadata.len() > MAX_LEDGER_BYTES {
            return Err(NativeProviderError::Storage);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(NativeProviderError::Storage);
            }
        }
        let mut bytes = Vec::new();
        File::open(&path)
            .map_err(|_| NativeProviderError::Storage)?
            .take(MAX_LEDGER_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| NativeProviderError::Storage)?;
        if bytes.len() as u64 > MAX_LEDGER_BYTES {
            return Err(NativeProviderError::Storage);
        }
        let envelope: LedgerEnvelope =
            serde_json::from_slice(&bytes).map_err(|_| NativeProviderError::Storage)?;
        if envelope.version != 1
            || envelope.namespace != self.namespace
            || envelope.attempts.len() > MAX_PENDING
        {
            return Err(NativeProviderError::Storage);
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut references = std::collections::BTreeSet::new();
        for p in &envelope.attempts {
            if p.attempt_id.get_version_num() != 4
                || !ids.insert(p.attempt_id)
                || !references.insert(&p.credential_reference)
                || p.configuration_digest == [0; 32]
                || p.registration_digest == [0; 32]
                || CredentialReference::new(p.credential_reference.clone()).is_err()
                || p.source_id
                    .as_ref()
                    .is_some_and(|id| valid_source(id.as_str()).is_err())
            {
                return Err(NativeProviderError::Storage);
            }
        }
        self.verify()?;
        Ok(envelope.attempts)
    }
    fn save(&self, attempts: Vec<PendingAdmission>) -> Result<()> {
        self.verify()?;
        if attempts.len() > MAX_PENDING {
            return Err(NativeProviderError::TooLarge);
        }
        let bytes = serde_json::to_vec(&LedgerEnvelope {
            version: 1,
            namespace: self.namespace.clone(),
            attempts,
        })
        .map_err(|_| NativeProviderError::Storage)?;
        if bytes.len() as u64 > MAX_LEDGER_BYTES {
            return Err(NativeProviderError::TooLarge);
        }
        let mut temporary = tempfile::Builder::new()
            .prefix(".native-provider-admission-")
            .tempfile_in(&self.root.path)
            .map_err(|_| NativeProviderError::Storage)?;
        temporary
            .write_all(&bytes)
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|_| NativeProviderError::Storage)?;
        self.verify()?;
        temporary
            .persist(self.root.path.join("native-provider-admissions-v1.json"))
            .map_err(|_| NativeProviderError::Storage)?;
        File::open(&self.root.path)
            .and_then(|f| f.sync_all())
            .map_err(|_| NativeProviderError::Storage)?;
        self.verify()
    }
    fn insert(&self, p: PendingAdmission) -> Result<()> {
        let mut entries = self.load()?;
        if entries.len() >= MAX_PENDING {
            return Err(NativeProviderError::TooLarge);
        }
        entries.push(p);
        self.save(entries)
    }
    fn phase(&self, id: Uuid, phase: AdmissionPhase) -> Result<()> {
        let mut entries = self.load()?;
        entries
            .iter_mut()
            .find(|p| p.attempt_id == id)
            .ok_or(NativeProviderError::Invalid)?
            .phase = phase;
        self.save(entries)
    }
    fn remove(&self, id: Uuid) -> Result<()> {
        let mut entries = self.load()?;
        entries.retain(|p| p.attempt_id != id);
        self.save(entries)
    }
}

pub(crate) struct NativeProviders {
    config: NativeOwnerConfig,
    rpc: Arc<dyn Rpc>,
    vault: Option<VaultClient>,
    authorizer: Option<Arc<dyn Authorizer>>,
    ledger: Mutex<Result<CustodyLedger>>,
    admission: Semaphore,
}
impl NativeProviders {
    #[cfg(not(mobile))]
    pub(crate) fn new(app: &tauri::AppHandle, backend: ReaderBackend) -> Result<Self> {
        if app.config().identifier != env!("MDREADER_NATIVE_APP_ID") {
            return Err(NativeProviderError::Invalid);
        }
        let directory = crate::reader_profile::directory(app)
            .map_err(|_| NativeProviderError::Storage)?
            .join("reader-v1");
        let ledger = CustodyLedger::open(&directory, env!("MDREADER_NATIVE_APP_ID"));
        let config = compiled_config();
        let (vault, authorizer) = if config.registrations.is_empty()
            || !cfg!(all(unix, not(mobile)))
            || ledger.is_err()
        {
            (None, None)
        } else {
            let resources = (|| {
                let vault = VaultClient::new(Arc::new(
                    KeyringVault::new(&ledger.as_ref().map_err(|e| e.clone())?.namespace)
                        .map_err(NativeProviderError::from)?,
                ))
                .map_err(NativeProviderError::from)?;
                let authorizer = Arc::new(NativeAuthorizer {
                    config: OAuthConfig {
                        dropbox_registered_redirect: config
                            .oauth
                            .dropbox_registered_redirect
                            .clone(),
                    },
                    transport: Arc::new(
                        ReqwestTransport::new().map_err(NativeProviderError::from)?,
                    ),
                });
                Ok::<_, NativeProviderError>((vault, authorizer as Arc<dyn Authorizer>))
            })();
            match resources {
                Ok((v, a)) => (Some(v), Some(a)),
                Err(_) => (None, None),
            }
        };
        Ok(Self {
            config,
            rpc: Arc::new(DesktopRpc {
                backend,
                slots: Arc::new(Semaphore::new(4)),
            }),
            vault,
            authorizer,
            ledger: Mutex::new(ledger),
            admission: Semaphore::new(1),
        })
    }
    fn with_ledger<T>(&self, operation: impl FnOnce(&CustodyLedger) -> Result<T>) -> Result<T> {
        let guard = self
            .ledger
            .lock()
            .map_err(|_| NativeProviderError::Storage)?;
        operation(guard.as_ref().map_err(Clone::clone)?)
    }
    fn pending(&self) -> Result<Vec<PendingAdmission>> {
        self.with_ledger(CustodyLedger::load)
    }
    async fn status(&self) -> Result<ProviderSnapshot> {
        let mut availability = self.config.availability();
        if self.vault.is_none() {
            for item in &mut availability {
                if item.configured {
                    item.supported = false;
                    item.diagnostic = Some("Native credential custody is unavailable".into())
                }
            }
        }
        let (sources, error) = match self
            .rpc
            .provider(ProviderRequest::List)
            .await
            .and_then(decode::<Vec<SourceSummary>>)
            .and_then(|rows| rows.into_iter().map(source_dto).collect())
        {
            Ok(sources) => (sources, None),
            Err(error) => (vec![], Some(error)),
        };
        let (pending_admissions, error) = match self.pending() {
            Ok(pending) => (pending.iter().map(PendingAdmission::dto).collect(), error),
            Err(storage) => (vec![], Some(storage)),
        };
        Ok(ProviderSnapshot {
            availability,
            sources,
            pending_admissions,
            error,
        })
    }
    #[cfg(test)]
    async fn connect(
        &self,
        kind: NativeProviderKind,
        selection: Option<DropboxSelection>,
        source: Option<String>,
    ) -> Result<AdmissionOutcome> {
        self.connect_with_permissions(kind, selection, source, false)
            .await
    }
    #[cfg(test)]
    async fn connect_with_permissions(
        &self,
        kind: NativeProviderKind,
        selection: Option<DropboxSelection>,
        source: Option<String>,
        allow_editing: bool,
    ) -> Result<AdmissionOutcome> {
        self.connect_with_drive(kind, selection, source, allow_editing, None)
            .await
    }
    async fn connect_with_drive(
        &self,
        kind: NativeProviderKind,
        selection: Option<DropboxSelection>,
        source: Option<String>,
        allow_editing: bool,
        drive_folder: Option<drive::DriveFolder>,
    ) -> Result<AdmissionOutcome> {
        #[cfg(mobile)]
        if allow_editing {
            return Err(NativeProviderError::Unsupported);
        }
        if allow_editing
            && kind == NativeProviderKind::GoogleDrive
            && drive_folder
                .as_ref()
                .is_none_or(|folder| folder.recursive.is_none())
        {
            return Err(NativeProviderError::Invalid);
        }
        if !self
            .config
            .availability()
            .iter()
            .any(|entry| entry.provider == kind && entry.supported)
        {
            return Err(NativeProviderError::Unsupported);
        }
        let _permit = self
            .admission
            .try_acquire()
            .map_err(|_| NativeProviderError::Busy)?;
        if let Some(existing) = self.pending()?.into_iter().find(|p| p.provider == kind) {
            return Ok(AdmissionOutcome::Pending {
                attempt_id: existing.attempt_id.to_string(),
                reason: NativeProviderError::Busy,
            });
        }
        let registration = self
            .config
            .registration(kind)
            .ok_or(NativeProviderError::Unconfigured)?;
        let selected = if let Some(folder) = drive_folder {
            if kind != NativeProviderKind::GoogleDrive || selection.is_some() {
                return Err(NativeProviderError::Invalid);
            }
            let root = mdreader_provider::RecordKey::new(folder.id)?;
            let selected = match folder.recursive {
                Some(recursive) => NativeSelection::DriveFolder {
                    folder: root,
                    recursive,
                },
                None => NativeSelection::DriveReadOnlyFolder { folder: root },
            };
            selected.validate()?;
            Some(selected)
        } else {
            match (kind, selection) {
                (NativeProviderKind::GoogleDrive, None) => None,
                (NativeProviderKind::Dropbox, Some(s)) => {
                    let selected = NativeSelection::DropboxRoot {
                        path: mdreader_provider::RecordKey::new(s.path)
                            .map_err(NativeProviderError::from)?,
                        recursive: s.recursive,
                    };
                    selected.validate().map_err(NativeProviderError::from)?;
                    Some(selected)
                }
                _ => return Err(NativeProviderError::Invalid),
            }
        };
        if let Some(source) = &source {
            let id = valid_source(source)?;
            let sources: Vec<SourceSummary> =
                decode(self.rpc.provider(ProviderRequest::List).await?)?;
            let previous = sources
                .iter()
                .find(|s| s.source_id == id)
                .ok_or(NativeProviderError::Invalid)?;
            if NativeProviderKind::from(previous.kind) != kind {
                return Err(NativeProviderError::Invalid);
            }
            if previous.enabled {
                let value = self
                    .rpc
                    .provider(ProviderRequest::Disable { source_id: id })
                    .await?;
                if value.get("disabled") != Some(&Value::Bool(true)) {
                    return Err(NativeProviderError::InvalidResponse);
                }
            }
        }
        let vault = self
            .vault
            .as_ref()
            .ok_or(NativeProviderError::Unconfigured)?;
        let completed = self
            .authorizer
            .as_ref()
            .ok_or(NativeProviderError::Unconfigured)?
            .authorize(registration.clone(), selected, allow_editing)
            .await?;
        let reference = CredentialReference::fresh();
        let pending = PendingAdmission {
            attempt_id: Uuid::new_v4(),
            provider: kind,
            name: completed.name,
            source_id: source.map(|s| valid_source(&s)).transpose()?,
            phase: AdmissionPhase::VaultPending,
            credential_reference: reference.as_str().into(),
            configuration_digest: completed
                .grant
                .configuration_digest()
                .map_err(NativeProviderError::from)?,
            registration_digest: registration.digest(),
        };
        self.with_ledger(|ledger| ledger.insert(pending.clone()))?;
        // Persist intent BEFORE dispatch. Timeout may mean the worker did write.
        let context = vault_context()?;
        if let Err(error) = vault.create(&reference, &completed.grant, &context).await {
            return Ok(AdmissionOutcome::Pending {
                attempt_id: pending.attempt_id.to_string(),
                reason: error.into(),
            });
        }
        self.publish(pending).await
    }
    async fn publish(&self, pending: PendingAdmission) -> Result<AdmissionOutcome> {
        let result = self.with_ledger(|ledger| {
            ledger.phase(pending.attempt_id, AdmissionPhase::AdmissionPending)
        });
        if let Err(reason) = result {
            return Ok(AdmissionOutcome::Pending {
                attempt_id: pending.attempt_id.to_string(),
                reason,
            });
        }
        let outcome = self
            .rpc
            .provider(pending.request())
            .await
            .and_then(decode::<AdmissionReply>);
        match outcome {
            Ok(reply) => {
                valid_source(&reply.source_id)?;
                if pending
                    .source_id
                    .as_ref()
                    .is_some_and(|id| id.as_str() != reply.source_id)
                {
                    return Err(NativeProviderError::InvalidResponse);
                }
                // If this fsync fails, replay remains safe and the custody reference
                // stays retained: the service may already have committed admission.
                if let Err(reason) = self.with_ledger(|ledger| ledger.remove(pending.attempt_id)) {
                    return Ok(AdmissionOutcome::Pending {
                        attempt_id: pending.attempt_id.to_string(),
                        reason,
                    });
                }
                Ok(AdmissionOutcome::Admitted {
                    source_id: reply.source_id,
                    enabled: reply.enabled,
                    replayed: reply.replayed,
                })
            }
            Err(reason) => Ok(AdmissionOutcome::Pending {
                attempt_id: pending.attempt_id.to_string(),
                reason,
            }),
        }
    }
    async fn reconcile(&self, attempt: &str) -> Result<AdmissionOutcome> {
        let _permit = self
            .admission
            .try_acquire()
            .map_err(|_| NativeProviderError::Busy)?;
        let id = valid_uuid(attempt)?;
        let pending = self
            .pending()?
            .into_iter()
            .find(|p| p.attempt_id == id)
            .ok_or(NativeProviderError::Invalid)?;
        let result = async {
            let registration = self
                .config
                .registration(pending.provider)
                .ok_or(NativeProviderError::Unconfigured)?;
            if registration.digest() != pending.registration_digest {
                return Err(NativeProviderError::Unconfigured);
            }
            let reference = CredentialReference::new(pending.credential_reference.clone())
                .map_err(NativeProviderError::from)?;
            let grant = self
                .vault
                .as_ref()
                .ok_or(NativeProviderError::Unconfigured)?
                .get(&reference, &vault_context()?)
                .await
                .map_err(NativeProviderError::from)?
                .ok_or(NativeProviderError::ReconnectRequired)?;
            if grant
                .configuration_digest()
                .map_err(NativeProviderError::from)?
                != pending.configuration_digest
                || grant.registration().digest() != pending.registration_digest
            {
                return Err(NativeProviderError::Unauthorized);
            }
            Ok(())
        }
        .await;
        if let Err(reason) = result {
            return Ok(AdmissionOutcome::Pending {
                attempt_id: attempt.into(),
                reason,
            });
        }
        // Reuse the exact native reference/request; no OAuth/create/overwrite.
        self.publish(pending).await
    }
    #[cfg(test)]
    async fn reconnect(
        &self,
        source: &str,
        selection: Option<DropboxSelection>,
    ) -> Result<AdmissionOutcome> {
        self.reconnect_with_permissions(source, selection, false)
            .await
    }
    #[cfg(test)]
    async fn reconnect_with_permissions(
        &self,
        source: &str,
        selection: Option<DropboxSelection>,
        allow_editing: bool,
    ) -> Result<AdmissionOutcome> {
        self.reconnect_with_drive(source, selection, allow_editing, None)
            .await
    }
    async fn reconnect_with_drive(
        &self,
        source: &str,
        selection: Option<DropboxSelection>,
        allow_editing: bool,
        drive_folder: Option<drive::DriveFolder>,
    ) -> Result<AdmissionOutcome> {
        let id = valid_source(source)?;
        let sources: Vec<SourceSummary> = decode(self.rpc.provider(ProviderRequest::List).await?)?;
        let kind = sources
            .iter()
            .find(|s| s.source_id == id)
            .map(|s| NativeProviderKind::from(s.kind))
            .ok_or(NativeProviderError::Invalid)?;
        self.connect_with_drive(
            kind,
            selection,
            Some(source.into()),
            allow_editing,
            drive_folder,
        )
        .await
    }
}
fn vault_context() -> Result<IoContext> {
    IoContext::new(
        Uuid::new_v4(),
        Instant::now() + Duration::from_secs(10),
        1,
        65536,
        Default::default(),
    )
    .map_err(Into::into)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmissionReply {
    source_id: String,
    enabled: bool,
    replayed: bool,
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|_| NativeProviderError::InvalidResponse)
}
fn valid_uuid(value: &str) -> Result<Uuid> {
    let id = Uuid::parse_str(value).map_err(|_| NativeProviderError::Invalid)?;
    if id.get_version_num() != 4 || id.to_string() != value {
        return Err(NativeProviderError::Invalid);
    }
    Ok(id)
}
fn valid_source(value: &str) -> Result<SourceId> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(NativeProviderError::Invalid);
    }
    SourceId::new(value).map_err(Into::into)
}
fn safe_number(value: i128) -> Result<f64> {
    if !(0..=9_007_199_254_740_991).contains(&value) {
        return Err(NativeProviderError::InvalidResponse);
    }
    Ok(value as f64)
}
fn source_dto(source: SourceSummary) -> Result<ProviderSourceDto> {
    use mdreader_provider::Coverage;
    valid_source(source.source_id.as_str())?;
    let coverage = match source.coverage {
        Coverage::NeverIndexed => CoverageDto::NeverIndexed,
        Coverage::Partial { reason } => CoverageDto::Partial {
            reason: reason.as_str().into(),
        },
        Coverage::MetadataCompleteSelected { observed_at_ms } => {
            CoverageDto::MetadataCompleteSelected {
                observed_at_ms: safe_number(observed_at_ms.into())?,
            }
        }
        Coverage::CompleteSelected { observed_at_ms } => CoverageDto::CompleteSelected {
            observed_at_ms: safe_number(observed_at_ms.into())?,
        },
    };
    let c = source.capabilities;
    let selected_scope = match source.selected_scope {
        SelectionSummary::ExactDocuments { count } => ScopeDto::ExactDocuments {
            count: count
                .try_into()
                .map_err(|_| NativeProviderError::InvalidResponse)?,
        },
        SelectionSummary::ScopedRoot { recursive, root }
            if source.kind == ProviderKind::GoogleDrive =>
        {
            let folder_id = root
                .ok_or(NativeProviderError::InvalidResponse)?
                .as_str()
                .to_owned();
            drive::valid_folder(&folder_id)?;
            if recursive != (folder_id == "root") {
                return Err(NativeProviderError::InvalidResponse);
            }
            ScopeDto::DriveReadOnlyFolder {
                account_wide: recursive,
                folder_id,
            }
        }
        SelectionSummary::ScopedRoot { recursive, root } => ScopeDto::ScopedRoot {
            recursive,
            path: root.map(|path| path.as_str().to_owned()),
        },
        SelectionSummary::DriveFolder { root, recursive } => {
            if source.kind != ProviderKind::GoogleDrive {
                return Err(NativeProviderError::InvalidResponse);
            }
            drive::valid_folder(root.as_str())?;
            ScopeDto::DriveFolder {
                folder_id: root.as_str().into(),
                recursive,
            }
        }
        SelectionSummary::ImportedLegacyBroad => ScopeDto::ImportedLegacyBroad,
    };
    if let Some(id) = &source.reader_source_id {
        validate_reader_id(id.as_str())?;
    }
    let browse_folders = matches!(
        selected_scope,
        ScopeDto::DriveReadOnlyFolder { .. } | ScopeDto::DriveFolder { .. }
    );
    Ok(ProviderSourceDto {
        source_id: source.source_id.as_str().into(),
        reader_source_id: source.reader_source_id.map(|s| s.as_str().into()),
        name: source.name.as_str().into(),
        kind: source.kind.into(),
        enabled: source.enabled,
        selected_scope,
        capabilities: CapabilitiesDto {
            read: c.read,
            list: c.list,
            changes: c.changes,
            assets: c.assets,
            guarded_write: c.guarded_write && !cfg!(mobile),
            browse_folders,
        },
        health: serde_json::to_value(source.health)
            .map_err(|_| NativeProviderError::InvalidResponse)?
            .as_str()
            .ok_or(NativeProviderError::InvalidResponse)?
            .into(),
        coverage,
        metadata_listing: source
            .metadata_listing
            .map(|listing| {
                use mdreader_service::provider::ListingCompleteness as L;
                if listing.known_documents > 16_384 {
                    return Err(NativeProviderError::InvalidResponse);
                }
                Ok(MetadataListingDto {
                    known_documents: listing.known_documents,
                    completeness: match listing.completeness {
                        L::NeverObserved => ListingCompletenessDto::NeverObserved,
                        L::Partial => ListingCompletenessDto::Partial,
                        L::CompleteSelected => ListingCompletenessDto::CompleteSelected,
                    },
                    observed_at_ms: listing
                        .observed_at_ms
                        .map(|v| safe_number(i128::from(v)))
                        .transpose()?,
                    refresh_pending: listing.refresh_pending,
                })
            })
            .transpose()?,
    })
}
fn validate_reader_id(value: &str) -> Result<()> {
    let digest = value
        .strip_prefix("provider:")
        .ok_or(NativeProviderError::Invalid)?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(NativeProviderError::Invalid);
    }
    Ok(())
}
fn projection_dto(receipt: ProjectionReceipt) -> Result<ProjectionDto> {
    let dto = ProjectionDto {
        job_id: receipt.job_id.to_string(),
        reader_source_id: receipt.reader_source_id.as_str().into(),
        path: receipt.path.as_str().into(),
        revision: receipt.revision.as_str().into(),
    };
    validate_projection(&dto)?;
    Ok(dto)
}
fn validate_projection(p: &ProjectionDto) -> Result<()> {
    validate_reader_id(&p.reader_source_id)?;
    let hash = p
        .path
        .strip_prefix("records/")
        .and_then(|p| p.strip_suffix(".md"))
        .ok_or(NativeProviderError::Invalid)?;
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || p.revision.len() != 64
        || !p
            .revision
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(NativeProviderError::Invalid);
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentPageWire {
    documents: Vec<DocumentWire>,
    offset: u32,
    next_offset: Option<u32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentWire {
    document_id: String,
    name: DisplayText,
    display_path: DisplayText,
    modified_at_ms: Option<i64>,
    size_bytes: Option<u64>,
    cached: bool,
    cached_observed_at_ms: Option<i64>,
    admitted: bool,
    projection: Option<ProjectionReceipt>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RefreshWire {
    pages: u32,
    metadata_complete: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BeginEditWire {
    edit_id: String,
    markdown: String,
    expires_at_ms: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MutationWire {
    operation_id: String,
    state: mdreader_provider::journal::MutationState,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairWire {
    recovered_unknown: u32,
    published: u32,
    conflicts: u32,
    pending: u32,
}
fn mutation_state(state: mdreader_provider::journal::MutationState) -> MutationStateDto {
    use mdreader_provider::journal::MutationState as S;
    match state {
        S::Prepared => MutationStateDto::Prepared,
        S::Dispatched => MutationStateDto::Dispatched,
        S::Unknown => MutationStateDto::Unknown,
        S::Accepted => MutationStateDto::Accepted,
        S::Published => MutationStateDto::Published,
        S::Conflict => MutationStateDto::Conflict,
    }
}
fn link_candidate(
    value: mdreader_service::provider::ProviderLinkCandidate,
    expected_source: &str,
) -> Result<NativeLinkCandidate> {
    valid_source(value.source_id.as_str())?;
    valid_uuid(&value.document_id.to_string())?;
    if value.source_id.as_str() != expected_source {
        return Err(NativeProviderError::InvalidResponse);
    }
    Ok(NativeLinkCandidate {
        source_id: value.source_id.as_str().into(),
        document_id: value.document_id.to_string(),
        title: value.title.as_str().into(),
    })
}
fn link_resolution(
    value: ProviderLinkResolution,
    reader_source: &str,
    expected_source: &str,
) -> Result<NativeLinkResolution> {
    use NativeLinkResolution as O;
    use ProviderLinkResolution as I;
    let heading = match &value {
        I::Resolved { heading, .. }
        | I::SelectedUncached { heading, .. }
        | I::Ambiguous { heading, .. } => heading.as_deref(),
        I::HeadingMissing { heading, .. } => Some(heading.as_str()),
        I::Missing => None,
    };
    if heading.is_some_and(|h| h.len() > 4096 || h.chars().any(char::is_control)) {
        return Err(NativeProviderError::InvalidResponse);
    }
    Ok(match value {
        I::Resolved {
            candidate,
            projection,
            line,
            heading,
        } => {
            let projection = projection_dto(projection)?;
            if projection.reader_source_id != reader_source || line == 0 {
                return Err(NativeProviderError::InvalidResponse);
            }
            O::Resolved {
                candidate: link_candidate(candidate, expected_source)?,
                projection,
                line,
                heading,
            }
        }
        I::SelectedUncached { candidate, heading } => O::SelectedUncached {
            candidate: link_candidate(candidate, expected_source)?,
            heading,
        },
        I::Ambiguous {
            candidates,
            heading,
        } => {
            if candidates.len() < 2 || candidates.len() > 16 {
                return Err(NativeProviderError::InvalidResponse);
            }
            let mut ids = std::collections::BTreeSet::new();
            let candidates = candidates
                .into_iter()
                .map(|candidate| {
                    if !ids.insert(candidate.document_id) {
                        return Err(NativeProviderError::InvalidResponse);
                    }
                    link_candidate(candidate, expected_source)
                })
                .collect::<Result<_>>()?;
            O::Ambiguous {
                candidates,
                heading,
            }
        }
        I::Missing => O::Missing,
        I::HeadingMissing { candidate, heading } => O::HeadingMissing {
            candidate: link_candidate(candidate, expected_source)?,
            heading,
        },
    })
}
impl NativeProviders {
    async fn source_projection(&self, source: &SourceId, projection: &ProjectionDto) -> Result<()> {
        let rows: Vec<SourceSummary> = decode(self.rpc.provider(ProviderRequest::List).await?)?;
        if !rows.iter().any(|row| {
            row.enabled
                && row.source_id == *source
                && row
                    .reader_source_id
                    .as_ref()
                    .is_some_and(|id| id.as_str() == projection.reader_source_id)
        }) {
            return Err(NativeProviderError::InvalidResponse);
        }
        Ok(())
    }
    async fn request(&self, request: NativeProviderRequest) -> Result<NativeProviderResult> {
        #[cfg(mobile)]
        if matches!(
            request,
            NativeProviderRequest::CreateDocument { .. }
                | NativeProviderRequest::InspectCreate { .. }
                | NativeProviderRequest::BeginEditExact { .. }
                | NativeProviderRequest::BeginEdit { .. }
                | NativeProviderRequest::PrepareSave { .. }
                | NativeProviderRequest::DispatchSave { .. }
                | NativeProviderRequest::Repair { .. }
        ) {
            return Err(NativeProviderError::Unsupported);
        }

        use NativeProviderRequest as R;
        use NativeProviderResult as O;
        let cached_request = matches!(&request, R::ReadCached { .. });
        let forget_request = matches!(&request, R::Forget { .. });
        match request {
            R::RecoveredDrafts { offset, limit } => self.recovered_drafts(offset, limit).await,
            R::ReadRecoveredDraft { draft_id, revision } => {
                self.read_recovered_draft(draft_id, revision).await
            }
            R::BrowseFolders {
                source_id,
                folder_id,
                cursor,
            } => self.browse_drive(source_id, folder_id, cursor).await,
            R::ResolveLink {
                source_id,
                path,
                revision,
                kind,
                target,
            } => {
                validate_projection(&ProjectionDto {
                    job_id: String::new(),
                    reader_source_id: source_id.clone(),
                    path: path.clone(),
                    revision: revision.clone(),
                })?;
                if target.len() > 4096 {
                    return Err(NativeProviderError::TooLarge);
                }
                let kind = match kind {
                    NativeLinkKind::Relative => ProviderLinkKind::Relative,
                    NativeLinkKind::Wikilink => ProviderLinkKind::Wikilink,
                };
                let resolution: ProviderLinkResolution = decode(
                    self.rpc
                        .provider(ProviderRequest::ResolveLink {
                            reader_source_id: SourceId::new(source_id.clone())
                                .map_err(NativeProviderError::from)?,
                            path: mdreader_provider::RecordKey::new(path)
                                .map_err(NativeProviderError::from)?,
                            revision: mdreader_provider::OpaqueValue::new(revision)
                                .map_err(NativeProviderError::from)?,
                            kind,
                            target,
                        })
                        .await?,
                )?;
                let sources: Vec<SourceSummary> =
                    decode(self.rpc.provider(ProviderRequest::List).await?)?;
                let expected = sources
                    .iter()
                    .find(|row| {
                        row.enabled
                            && row
                                .reader_source_id
                                .as_ref()
                                .is_some_and(|id| id.as_str() == source_id)
                    })
                    .ok_or(NativeProviderError::Unauthorized)?;
                Ok(O::ResolveLink {
                    resolution: link_resolution(
                        resolution,
                        &source_id,
                        expected.source_id.as_str(),
                    )?,
                })
            }
            R::Documents {
                source_id,
                offset,
                limit,
            } => {
                if limit == 0 || limit > 256 || offset > 16384 {
                    return Err(NativeProviderError::Invalid);
                }
                let source = valid_source(&source_id)?;
                let page: DocumentPageWire = decode(
                    self.rpc
                        .provider(ProviderRequest::Documents {
                            source_id: source.clone(),
                            offset: offset as usize,
                            limit: limit as usize,
                        })
                        .await?,
                )?;
                if page.documents.len() > limit as usize
                    || page.offset != offset
                    || page.next_offset.is_some_and(|n| n <= offset || n > 16384)
                {
                    return Err(NativeProviderError::InvalidResponse);
                }
                let sources: Vec<SourceSummary> =
                    decode(self.rpc.provider(ProviderRequest::List).await?)?;
                let expected = sources
                    .iter()
                    .find(|row| row.enabled && row.source_id == source)
                    .ok_or(NativeProviderError::Unauthorized)?
                    .reader_source_id
                    .as_ref()
                    .map(|id| id.as_str());
                let mut documents = Vec::new();
                let mut ids = std::collections::BTreeSet::new();
                for document in page.documents {
                    valid_uuid(&document.document_id)?;
                    if !ids.insert(document.document_id.clone()) {
                        return Err(NativeProviderError::InvalidResponse);
                    }
                    let projection = document.projection.map(projection_dto).transpose()?;
                    if document.admitted != projection.is_some()
                        || document.admitted && !document.cached
                    {
                        return Err(NativeProviderError::InvalidResponse);
                    }
                    if projection
                        .as_ref()
                        .is_some_and(|p| Some(p.reader_source_id.as_str()) != expected)
                    {
                        return Err(NativeProviderError::InvalidResponse);
                    }
                    documents.push(ProviderDocumentDto {
                        document_id: document.document_id,
                        name: document.name.as_str().into(),
                        display_path: document.display_path.as_str().into(),
                        modified_at_ms: document
                            .modified_at_ms
                            .map(|v| safe_number(v.into()))
                            .transpose()?,
                        size_bytes: document
                            .size_bytes
                            .map(|v| safe_number(v.into()))
                            .transpose()?,
                        cached: document.cached,
                        cached_observed_at_ms: document
                            .cached_observed_at_ms
                            .map(|v| safe_number(v.into()))
                            .transpose()?,
                        admitted: document.admitted,
                        projection,
                    });
                }
                Ok(O::Documents {
                    documents,
                    offset,
                    next_offset: page.next_offset,
                })
            }
            R::Refresh { source_id } => {
                let value: RefreshWire = decode(
                    self.rpc
                        .provider(ProviderRequest::Refresh {
                            source_id: valid_source(&source_id)?,
                        })
                        .await?,
                )?;
                Ok(O::Refresh {
                    pages: value.pages,
                    metadata_complete: value.metadata_complete,
                })
            }
            R::FetchCurrent {
                source_id,
                document_id,
            }
            | R::ReadCached {
                source_id,
                document_id,
            } => {
                let cached = cached_request;
                let source = valid_source(&source_id)?;
                let document = valid_uuid(&document_id)?;
                let operation = if cached {
                    ProviderRequest::CachedOpen {
                        source_id: source.clone(),
                        document_id: document,
                    }
                } else {
                    ProviderRequest::Open {
                        source_id: source.clone(),
                        document_id: document,
                    }
                };
                let projection = projection_dto(decode(self.rpc.provider(operation).await?)?)?;
                self.source_projection(&source, &projection).await?;
                Ok(if cached {
                    O::ReadCached { projection }
                } else {
                    O::FetchCurrent { projection }
                })
            }
            R::ReadCurrentDocument {
                source_id,
                path,
                revision,
            } => reading::current(&*self.rpc, source_id, path, revision).await,
            R::ReadDocument {
                source_id,
                path,
                revision,
            } => {
                let projection = ProjectionDto {
                    job_id: String::new(),
                    reader_source_id: source_id.clone(),
                    path: path.clone(),
                    revision: revision.clone(),
                };
                validate_projection(&projection)?;
                let document = self.rpc.read(&projection).await?;
                if document.source_id != source_id
                    || document.path != path
                    || document.revision != revision
                    || document.markdown.len() > MAX_MARKDOWN
                {
                    return Err(NativeProviderError::InvalidResponse);
                }
                let line_end = document
                    .markdown
                    .lines()
                    .count()
                    .max(1)
                    .try_into()
                    .map_err(|_| NativeProviderError::TooLarge)?;
                Ok(O::ReadDocument {
                    source_id,
                    path,
                    revision,
                    markdown: document.markdown,
                    line_start: 1,
                    line_end,
                })
            }
            R::Disable { source_id } | R::Forget { source_id } => {
                let forget = forget_request;
                let source = valid_source(&source_id)?;
                let value = self
                    .rpc
                    .provider(if forget {
                        ProviderRequest::Forget { source_id: source }
                    } else {
                        ProviderRequest::Disable { source_id: source }
                    })
                    .await?;
                if value.get("disabled") != Some(&Value::Bool(true))
                    || forget && value.get("forgotten") != Some(&Value::Bool(true))
                {
                    return Err(NativeProviderError::InvalidResponse);
                }
                Ok(if forget {
                    O::Forget {
                        disabled: true,
                        forgotten: true,
                    }
                } else {
                    O::Disable {
                        disabled: true,
                        forgotten: false,
                    }
                })
            }
            R::Disconnect { source_id } => disconnect::disconnect(&*self.rpc, source_id).await,
            R::CreateDocument {
                source_id,
                operation_id,
                filename,
                markdown,
            } => {
                self.create_document(source_id, operation_id, filename, markdown)
                    .await
            }
            R::InspectCreate {
                source_id,
                operation_id,
            } => self.inspect_create(source_id, operation_id).await,
            R::BeginEditExact {
                source_id,
                path,
                revision,
            } => self.begin_edit_exact(source_id, path, revision).await,
            R::InspectSave {
                source_id,
                operation_id,
            } => self.inspect_save(source_id, operation_id).await,
            R::BeginEdit {
                source_id,
                document_id,
            } => {
                let value: BeginEditWire = decode(
                    self.rpc
                        .provider(ProviderRequest::BeginEdit {
                            source_id: valid_source(&source_id)?,
                            document_id: valid_uuid(&document_id)?,
                        })
                        .await?,
                )?;
                valid_uuid(&value.edit_id)?;
                if value.markdown.len() > MAX_MARKDOWN {
                    return Err(NativeProviderError::InvalidResponse);
                }
                Ok(O::BeginEdit {
                    edit_id: value.edit_id,
                    markdown: value.markdown,
                    expires_at_ms: safe_number(value.expires_at_ms.into())?,
                })
            }
            R::PrepareSave {
                source_id,
                edit_id,
                markdown,
            } => {
                if markdown.len() > MAX_MARKDOWN {
                    return Err(NativeProviderError::TooLarge);
                }
                let edit = valid_uuid(&edit_id)?;
                let value: MutationWire = decode(
                    self.rpc
                        .provider(ProviderRequest::PrepareSave {
                            source_id: valid_source(&source_id)?,
                            edit_id: edit,
                            markdown,
                        })
                        .await?,
                )?;
                if value.operation_id != edit_id
                    || value.state != mdreader_provider::journal::MutationState::Prepared
                {
                    return Err(NativeProviderError::InvalidResponse);
                }
                Ok(O::PrepareSave {
                    operation_id: value.operation_id,
                    state: mutation_state(value.state),
                })
            }
            R::DispatchSave {
                source_id,
                operation_id,
            } => {
                let id = valid_uuid(&operation_id)?;
                let source = valid_source(&source_id)?;
                let result = self
                    .rpc
                    .provider(ProviderRequest::DispatchSave {
                        source_id: source.clone(),
                        operation_id: id,
                    })
                    .await;
                let value = match result {
                    Ok(value) => value,
                    Err(NativeProviderError::Timeout | NativeProviderError::ServiceUnavailable) => {
                        return Err(NativeProviderError::UnknownWrite { operation_id });
                    }
                    Err(NativeProviderError::LocalPublicationPending { .. }) => {
                        return Err(NativeProviderError::LocalPublicationPending { operation_id });
                    }
                    Err(error) => return Err(error),
                };
                let projection = projection_dto(decode(value)?)?;
                self.source_projection(&source, &projection).await?;
                Ok(O::DispatchSave { projection })
            }
            R::Mutations { source_id } => {
                let values: Vec<MutationWire> = decode(
                    self.rpc
                        .provider(ProviderRequest::Mutations {
                            source_id: valid_source(&source_id)?,
                        })
                        .await?,
                )?;
                if values.len() > 256 {
                    return Err(NativeProviderError::InvalidResponse);
                }
                let mutations = values
                    .into_iter()
                    .map(|value| {
                        valid_uuid(&value.operation_id)?;
                        Ok(MutationDto {
                            operation_id: value.operation_id,
                            state: mutation_state(value.state),
                        })
                    })
                    .collect::<Result<_>>()?;
                Ok(O::Mutations { mutations })
            }
            R::Repair { source_id } => {
                let value: RepairWire = decode(
                    self.rpc
                        .provider(ProviderRequest::Repair {
                            source_id: valid_source(&source_id)?,
                        })
                        .await?,
                )?;
                Ok(O::Repair {
                    recovered_unknown: value.recovered_unknown,
                    published: value.published,
                    conflicts: value.conflicts,
                    pending: value.pending,
                })
            }
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn native_provider_status(
    app: tauri::AppHandle,
    owner: tauri::State<'_, Arc<NativeProviders>>,
) -> Result<ProviderSnapshot> {
    let owner = owner.inner().clone();
    preservation::run(&app, async move { owner.status().await }).await
}
#[tauri::command]
#[specta::specta]
pub async fn native_provider_connect(
    app: tauri::AppHandle,
    owner: tauri::State<'_, Arc<NativeProviders>>,
    provider: NativeProviderKind,
    selection: Option<DropboxSelection>,
    allow_editing: Option<bool>,
    drive_folder: Option<drive::DriveFolder>,
) -> Result<AdmissionOutcome> {
    let owner = owner.inner().clone();
    preservation::run(&app, async move {
        owner
            .connect_with_drive(
                provider,
                selection,
                None,
                allow_editing.unwrap_or(false),
                drive_folder,
            )
            .await
    })
    .await
}
#[tauri::command]
#[specta::specta]
pub async fn native_provider_reconnect(
    app: tauri::AppHandle,
    owner: tauri::State<'_, Arc<NativeProviders>>,
    source_id: String,
    selection: Option<DropboxSelection>,
    allow_editing: Option<bool>,
    drive_folder: Option<drive::DriveFolder>,
) -> Result<AdmissionOutcome> {
    let owner = owner.inner().clone();
    preservation::run(&app, async move {
        owner
            .reconnect_with_drive(
                &source_id,
                selection,
                allow_editing.unwrap_or(false),
                drive_folder,
            )
            .await
    })
    .await
}
#[tauri::command]
#[specta::specta]
pub async fn native_provider_reconcile(
    app: tauri::AppHandle,
    owner: tauri::State<'_, Arc<NativeProviders>>,
    attempt_id: String,
) -> Result<AdmissionOutcome> {
    let owner = owner.inner().clone();
    preservation::run(&app, async move { owner.reconcile(&attempt_id).await }).await
}
#[tauri::command]
#[specta::specta]
pub async fn native_provider_request(
    app: tauri::AppHandle,
    owner: tauri::State<'_, Arc<NativeProviders>>,
    request: NativeProviderRequest,
) -> Result<NativeProviderResult> {
    let owner = owner.inner().clone();
    preservation::run(&app, async move { owner.request(request).await }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdreader_credentials::{ProviderGrant, SecretBundle, Vault};
    use mdreader_provider::AccountBinding;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    const SOURCE: &str = "remote-6a7dcc31-15b8-47df-8319-9def79c2cbab";
    const DOC: &str = "5aa571e6-3457-46ad-9a77-ce26f4e5b3b0";
    const OP: &str = "9e88a2ca-e79d-4cd1-b5f2-6b6c7ed8e539";
    fn config() -> NativeOwnerConfig {
        config_from(
            Some("synthetic.apps.googleusercontent.com"),
            None,
            None,
            None,
        )
    }
    fn grant() -> ProviderGrant {
        ProviderGrant::new(
            &config().registrations[0],
            AccountBinding::new("synthetic-private-account").unwrap(),
            NativeSelection::DriveFiles {
                files: vec![mdreader_credentials::DriveFile {
                    id: mdreader_provider::RecordKey::new("synthetic-private-record").unwrap(),
                    resource_key: None,
                }],
            },
            SecretBundle::new(
                "synthetic-private-access".into(),
                "synthetic-private-refresh".into(),
                4_000_000_000,
                [7; 32],
            )
            .unwrap(),
        )
        .unwrap()
    }
    #[derive(Default)]
    struct MemoryVault {
        grants: Mutex<BTreeMap<String, ProviderGrant>>,
        creates: AtomicUsize,
        lost: AtomicBool,
    }
    impl Vault for MemoryVault {
        fn get(&self, r: &CredentialReference) -> ProviderResult<Option<ProviderGrant>> {
            Ok(self.grants.lock().unwrap().get(r.as_str()).cloned())
        }
        fn create(&self, r: &CredentialReference, g: &ProviderGrant) -> ProviderResult<()> {
            self.creates.fetch_add(1, Ordering::SeqCst);
            let mut grants = self.grants.lock().unwrap();
            if grants.contains_key(r.as_str()) {
                return Err(ProviderError::Conflict);
            }
            grants.insert(r.as_str().into(), g.clone());
            if self.lost.swap(false, Ordering::SeqCst) {
                Err(ProviderError::Timeout)
            } else {
                Ok(())
            }
        }
        fn compare_exchange(
            &self,
            _: &CredentialReference,
            _: u64,
            _: &ProviderGrant,
        ) -> ProviderResult<()> {
            panic!("GUI must not refresh existing service-owned references")
        }
        fn delete(&self, _: &CredentialReference) -> ProviderResult<()> {
            panic!("uncertain admission custody must never be deleted")
        }
    }
    struct SyntheticAuth {
        calls: AtomicUsize,
        events: Arc<Mutex<Vec<String>>>,
    }
    #[async_trait::async_trait]
    impl Authorizer for SyntheticAuth {
        async fn authorize(
            &self,
            _: Registration,
            selection: Option<NativeSelection>,
            _allow_editing: bool,
        ) -> Result<CompletedAuthorization> {
            assert!(selection.is_none());
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.events.lock().unwrap().push("oauth".into());
            Ok(CompletedAuthorization {
                grant: grant(),
                name: DisplayText::new("Synthetic account").unwrap(),
            })
        }
    }
    struct SyntheticRpc {
        calls: Mutex<Vec<ProviderRequest>>,
        lost: AtomicBool,
        disabled: AtomicBool,
        wrong_projection: AtomicBool,
        read_calls: AtomicUsize,
        events: Arc<Mutex<Vec<String>>>,
        disconnect_reply: Mutex<Value>,
    }
    impl SyntheticRpc {
        fn new(events: Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                calls: Mutex::new(vec![]),
                lost: AtomicBool::new(false),
                disabled: AtomicBool::new(false),
                wrong_projection: AtomicBool::new(false),
                read_calls: AtomicUsize::new(0),
                events,
                disconnect_reply: Mutex::new(
                    json!({"disconnected":true,"credential":"deleted","revocation":"revoked"}),
                ),
            }
        }
        fn projection(&self) -> Value {
            json!({"job_id":OP,"reader_source_id":format!("provider:{}",if self.wrong_projection.load(Ordering::SeqCst){"b"}else{"a"}.repeat(64)),"path":format!("records/{}.md","c".repeat(64)),"revision":"d".repeat(64)})
        }
        fn summaries(&self) -> Value {
            json!([{"source_id":SOURCE,"reader_source_id":format!("provider:{}","a".repeat(64)),"name":"Synthetic source","kind":"google_drive","enabled":!self.disabled.load(Ordering::SeqCst),"selected_scope":{"kind":"exact_documents","count":1},"capabilities":{"read":true,"list":true,"changes":true,"assets":false,"guarded_write":true},"health":"ready","coverage":{"kind":"never_indexed"}}])
        }
    }
    #[async_trait::async_trait]
    impl Rpc for SyntheticRpc {
        async fn provider(&self, request: ProviderRequest) -> Result<Value> {
            self.calls.lock().unwrap().push(request.clone());
            match request {
                ProviderRequest::List => Ok(self.summaries()),
                ProviderRequest::Admit { .. } | ProviderRequest::Reconnect { .. } => {
                    self.events.lock().unwrap().push("admit".into());
                    if self.lost.swap(false, Ordering::SeqCst) {
                        return Err(NativeProviderError::Timeout);
                    }
                    Ok(json!({"source_id":SOURCE,"enabled":true,"replayed":true}))
                }
                ProviderRequest::Disable { .. } => {
                    self.events.lock().unwrap().push("disable".into());
                    self.disabled.store(true, Ordering::SeqCst);
                    Ok(json!({"disabled":true}))
                }
                ProviderRequest::Disconnect { .. } => {
                    self.events.lock().unwrap().push("disconnect".into());
                    Ok(self.disconnect_reply.lock().unwrap().clone())
                }
                ProviderRequest::ResolveLink { .. } => Ok(
                    json!({"state":"resolved","candidate":{"source_id":SOURCE,"document_id":DOC,"title":"Synthetic target"},"projection":self.projection(),"line":3,"heading":"decision"}),
                ),
                ProviderRequest::CachedOpen { .. } | ProviderRequest::Open { .. } => {
                    Ok(self.projection())
                }
                ProviderRequest::DispatchSave { .. } => Err(NativeProviderError::Timeout),
                ProviderRequest::PrepareSave { edit_id, .. } => {
                    Ok(json!({"operation_id":edit_id,"state":"prepared"}))
                }
                ProviderRequest::Mutations { .. } => {
                    Ok(json!([{"operation_id":OP,"state":"unknown"}]))
                }
                _ => Err(NativeProviderError::Unsupported),
            }
        }
        async fn read(&self, p: &ProjectionDto) -> Result<mdreader_core::Document> {
            self.read_calls.fetch_add(1, Ordering::SeqCst);
            Ok(mdreader_core::Document {
                source_id: p.reader_source_id.clone(),
                path: p.path.clone(),
                revision: p.revision.clone(),
                markdown: "# Synthetic\nExact cached evidence.\n".into(),
            })
        }
    }
    fn owner(
        directory: &Path,
        rpc: Arc<SyntheticRpc>,
        vault: VaultClient,
        auth: Arc<SyntheticAuth>,
    ) -> NativeProviders {
        NativeProviders {
            config: config(),
            rpc,
            vault: Some(vault),
            authorizer: Some(auth),
            ledger: Mutex::new(Ok(
                CustodyLedger::open(directory, "dev.mdreader.synthetic").unwrap()
            )),
            admission: Semaphore::new(1),
        }
    }
    #[tokio::test]
    async fn lost_vault_or_admit_reply_retains_durable_reference_and_replays_without_oauth() {
        for vault_lost in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let events = Arc::new(Mutex::new(vec![]));
            let storage = Arc::new(MemoryVault::default());
            storage.lost.store(vault_lost, Ordering::SeqCst);
            let vault = VaultClient::new(storage.clone()).unwrap();
            let rpc = Arc::new(SyntheticRpc::new(events.clone()));
            rpc.lost.store(!vault_lost, Ordering::SeqCst);
            let auth = Arc::new(SyntheticAuth {
                calls: AtomicUsize::new(0),
                events,
            });
            let first = owner(directory.path(), rpc.clone(), vault.clone(), auth.clone());
            let outcome = first
                .connect(NativeProviderKind::GoogleDrive, None, None)
                .await
                .unwrap();
            let attempt = match outcome {
                AdmissionOutcome::Pending {
                    attempt_id,
                    reason: NativeProviderError::Timeout,
                } => attempt_id,
                other => panic!("{other:?}"),
            };
            let pending = first.pending().unwrap();
            assert_eq!(pending.len(), 1);
            let reference = pending[0].credential_reference.clone();
            assert!(storage.grants.lock().unwrap().contains_key(&reference));
            let snapshot = serde_json::to_string(&first.status().await.unwrap()).unwrap();
            for secret in [
                &reference,
                "synthetic-private-access",
                "synthetic-private-refresh",
                "synthetic-private-account",
                "synthetic-private-record",
            ] {
                assert!(!snapshot.contains(secret))
            }
            let blocked = first.reconnect(SOURCE, None).await.unwrap();
            assert!(
                matches!(blocked,AdmissionOutcome::Pending{attempt_id,..}if attempt_id==attempt),
                "an unresolved attempt must prevent fresh consent even through reconnect"
            );
            assert_eq!(auth.calls.load(Ordering::SeqCst), 1);
            drop(first);
            let reopened = owner(directory.path(), rpc.clone(), vault, auth.clone());
            assert!(matches!(
                reopened.reconcile(&attempt).await.unwrap(),
                AdmissionOutcome::Admitted { replayed: true, .. }
            ));
            assert!(reopened.pending().unwrap().is_empty());
            assert!(storage.grants.lock().unwrap().contains_key(&reference));
            assert_eq!(storage.creates.load(Ordering::SeqCst), 1);
            assert_eq!(auth.calls.load(Ordering::SeqCst), 1);
            let calls = rpc.calls.lock().unwrap();
            let refs = calls
                .iter()
                .filter_map(|r| match r {
                    ProviderRequest::Admit {
                        credential_reference,
                        ..
                    } => Some(credential_reference),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(refs.len(), if vault_lost { 1 } else { 2 });
            assert!(refs.iter().all(|r| *r == &reference));
        }
    }
    #[tokio::test]
    async fn reconnect_revokes_before_fresh_consent_and_preserves_source_identity() {
        let directory = tempfile::tempdir().unwrap();
        let events = Arc::new(Mutex::new(vec![]));
        let rpc = Arc::new(SyntheticRpc::new(events.clone()));
        let auth = Arc::new(SyntheticAuth {
            calls: AtomicUsize::new(0),
            events: events.clone(),
        });
        let vault = VaultClient::new(Arc::new(MemoryVault::default())).unwrap();
        let owner = owner(directory.path(), rpc.clone(), vault, auth);
        assert!(
            matches!(owner.reconnect(SOURCE,None).await.unwrap(),AdmissionOutcome::Admitted{source_id,..}if source_id==SOURCE)
        );
        assert_eq!(*events.lock().unwrap(), ["disable", "oauth", "admit"]);
        assert!(rpc.calls.lock().unwrap().iter().any(
            |r| matches!(r,ProviderRequest::Reconnect{source_id,..}if source_id.as_str()==SOURCE)
        ));
    }
    #[tokio::test]
    async fn disconnect_forwards_a_validated_source_and_rejects_malformed_outcomes() {
        let directory = tempfile::tempdir().unwrap();
        let events = Arc::new(Mutex::new(vec![]));
        let rpc = Arc::new(SyntheticRpc::new(events.clone()));
        let auth = Arc::new(SyntheticAuth {
            calls: AtomicUsize::new(0),
            events: events.clone(),
        });
        let vault = VaultClient::new(Arc::new(MemoryVault::default())).unwrap();
        let owner = owner(directory.path(), rpc.clone(), vault, auth.clone());
        let disconnect = |source: &str| NativeProviderRequest::Disconnect {
            source_id: source.into(),
        };
        assert!(matches!(
            owner.request(disconnect(SOURCE)).await.unwrap(),
            NativeProviderResult::Disconnect {
                credential: SignInDisposalDto::Deleted,
                revocation: RevocationDto::Revoked
            }
        ));
        assert!(rpc.calls.lock().unwrap().iter().any(
            |r| matches!(r,ProviderRequest::Disconnect{source_id}if source_id.as_str()==SOURCE)
        ));
        assert_eq!(*events.lock().unwrap(), ["disconnect"]);
        assert_eq!(auth.calls.load(Ordering::SeqCst), 0);
        assert!(matches!(
            owner.request(disconnect("../outside")).await,
            Err(NativeProviderError::Invalid)
        ));
        for malformed in [
            json!({"disconnected":true,"credential":"shared","revocation":"revoked"}),
            json!({"disconnected":false,"credential":"deleted","revocation":"failed"}),
            json!({"disconnected":true,"credential":"deleted","revocation":"failed","token":"x"}),
        ] {
            *rpc.disconnect_reply.lock().unwrap() = malformed;
            assert!(matches!(
                owner.request(disconnect(SOURCE)).await,
                Err(NativeProviderError::InvalidResponse)
            ));
        }
    }
    #[tokio::test]
    async fn output_scope_path_and_revision_evidence_are_guarded_without_file_access() {
        let directory = tempfile::tempdir().unwrap();
        let events = Arc::new(Mutex::new(vec![]));
        let rpc = Arc::new(SyntheticRpc::new(events.clone()));
        let auth = Arc::new(SyntheticAuth {
            calls: AtomicUsize::new(0),
            events,
        });
        let owner = owner(
            directory.path(),
            rpc.clone(),
            VaultClient::new(Arc::new(MemoryVault::default())).unwrap(),
            auth,
        );
        rpc.wrong_projection.store(true, Ordering::SeqCst);
        assert!(matches!(
            owner
                .request(NativeProviderRequest::ReadCached {
                    source_id: SOURCE.into(),
                    document_id: DOC.into()
                })
                .await,
            Err(NativeProviderError::InvalidResponse)
        ));
        for path in ["../../private.md", "/absolute.md", "records/short.md"] {
            assert!(owner
                .request(NativeProviderRequest::ReadDocument {
                    source_id: format!("provider:{}", "a".repeat(64)),
                    path: path.into(),
                    revision: "d".repeat(64)
                })
                .await
                .is_err());
        }
        assert_eq!(rpc.read_calls.load(Ordering::SeqCst), 0);
        let output = owner
            .request(NativeProviderRequest::ReadDocument {
                source_id: format!("provider:{}", "a".repeat(64)),
                path: format!("records/{}.md", "c".repeat(64)),
                revision: "d".repeat(64),
            })
            .await
            .unwrap();
        let value = serde_json::to_value(output).unwrap();
        assert_eq!(value["lineStart"], 1);
        assert_eq!(value["lineEnd"], 2);
        assert_eq!(value["revision"], "d".repeat(64));
        assert_eq!(value["markdown"], "# Synthetic\nExact cached evidence.\n");
    }
    #[tokio::test]
    async fn linked_resolution_preserves_exact_evidence_and_rejects_wrong_binding_receipts() {
        let directory = tempfile::tempdir().unwrap();
        let events = Arc::new(Mutex::new(vec![]));
        let rpc = Arc::new(SyntheticRpc::new(events.clone()));
        let owner = owner(
            directory.path(),
            rpc.clone(),
            VaultClient::new(Arc::new(MemoryVault::default())).unwrap(),
            Arc::new(SyntheticAuth {
                calls: AtomicUsize::new(0),
                events,
            }),
        );
        let request = || NativeProviderRequest::ResolveLink {
            source_id: format!("provider:{}", "a".repeat(64)),
            path: format!("records/{}.md", "b".repeat(64)),
            revision: "c".repeat(64),
            kind: NativeLinkKind::Wikilink,
            target: "Plan#decision".into(),
        };
        let result = owner.request(request()).await.unwrap();
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["resolution"]["state"], "resolved");
        assert_eq!(value["resolution"]["line"], 3);
        assert_eq!(value["resolution"]["candidate"]["documentId"], DOC);
        assert_eq!(
            value["resolution"]["projection"]["revision"],
            "d".repeat(64)
        );
        rpc.wrong_projection.store(true, Ordering::SeqCst);
        assert!(matches!(
            owner.request(request()).await,
            Err(NativeProviderError::InvalidResponse)
        ));
        let before = rpc.calls.lock().unwrap().len();
        let mut too_large = request();
        if let NativeProviderRequest::ResolveLink { target, .. } = &mut too_large {
            *target = "x".repeat(4097)
        };
        assert!(matches!(
            owner.request(too_large).await,
            Err(NativeProviderError::TooLarge)
        ));
        assert_eq!(rpc.calls.lock().unwrap().len(), before);
    }
    #[tokio::test]
    async fn save_timeout_is_unknown_and_prepare_body_is_bounded_without_hidden_dispatch() {
        let directory = tempfile::tempdir().unwrap();
        let events = Arc::new(Mutex::new(vec![]));
        let rpc = Arc::new(SyntheticRpc::new(events.clone()));
        let auth = Arc::new(SyntheticAuth {
            calls: AtomicUsize::new(0),
            events,
        });
        let owner = owner(
            directory.path(),
            rpc.clone(),
            VaultClient::new(Arc::new(MemoryVault::default())).unwrap(),
            auth,
        );
        assert!(matches!(
            owner
                .request(NativeProviderRequest::PrepareSave {
                    source_id: SOURCE.into(),
                    edit_id: OP.into(),
                    markdown: "x".repeat(MAX_MARKDOWN + 1)
                })
                .await,
            Err(NativeProviderError::TooLarge)
        ));
        assert!(rpc.calls.lock().unwrap().is_empty());
        let prepared = owner
            .request(NativeProviderRequest::PrepareSave {
                source_id: SOURCE.into(),
                edit_id: OP.into(),
                markdown: "# Draft".into(),
            })
            .await
            .unwrap();
        assert!(matches!(
            prepared,
            NativeProviderResult::PrepareSave {
                state: MutationStateDto::Prepared,
                ..
            }
        ));
        assert_eq!(rpc.calls.lock().unwrap().len(), 1);
        let error = owner
            .request(NativeProviderRequest::DispatchSave {
                source_id: SOURCE.into(),
                operation_id: OP.into(),
            })
            .await
            .unwrap_err();
        assert_eq!(
            error,
            NativeProviderError::UnknownWrite {
                operation_id: OP.into()
            }
        );
        let mutations = serde_json::to_value(
            owner
                .request(NativeProviderRequest::Mutations {
                    source_id: SOURCE.into(),
                })
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(mutations["mutations"][0]["state"], "unknown");
        assert_eq!(
            rpc.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|r| matches!(r, ProviderRequest::DispatchSave { .. }))
                .count(),
            1
        );
    }
    #[test]
    fn compiled_registration_profile_identity_and_private_ledger_are_closed() {
        let empty = config_from(None, None, None, None);
        assert!(empty.registrations.is_empty());
        let invalid = config_from(
            None,
            Some("syntheticdropbox"),
            None,
            Some("http://127.0.0.1:34001/oauth/callback"),
        );
        assert!(invalid.registrations.is_empty());
        assert!(invalid.diagnostics[1]
            .as_ref()
            .unwrap()
            .contains("incomplete"));
        let valid = config_from(
            None,
            Some("syntheticdropbox"),
            Some(&"7".repeat(64)),
            Some("http://127.0.0.1:34001/oauth/callback"),
        );
        assert_eq!(valid.registrations.len(), 1);
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        assert_ne!(
            profile_namespace(a.path(), "dev.mdreader.app").unwrap(),
            profile_namespace(b.path(), "dev.mdreader.app").unwrap()
        );
        assert_ne!(
            profile_namespace(a.path(), "dev.mdreader.app").unwrap(),
            profile_namespace(a.path(), "dev.mdreader.probe").unwrap()
        );
        let ledger = CustodyLedger::open(a.path(), "dev.mdreader.synthetic").unwrap();
        assert!(
            CustodyLedger::open(a.path(), "dev.mdreader.synthetic").is_err(),
            "only one GUI custody writer may own a profile"
        );
        let path = a.path().join("native-provider-admissions-v1.json");
        fs::write(&path, vec![b'x'; MAX_LEDGER_BYTES as usize + 1]).unwrap();
        assert!(ledger.load().is_err());
        fs::remove_file(&path).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(b.path(), &path).unwrap();
            assert!(ledger.load().is_err());
        }
        assert!(serde_json::from_value::<NativeProviderRequest>(
            json!({"operation":"refresh","sourceId":SOURCE,"credentialReference":"forged"})
        )
        .is_err());
        assert!(serde_json::from_value::<DropboxSelection>(
            json!({"path":"/","recursive":false,"registrationAttestation":"rendererclaim"})
        )
        .is_err());
    }
}
