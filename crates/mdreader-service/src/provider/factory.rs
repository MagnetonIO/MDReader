//! Native credential custody and standard provider kernels. No renderer claims
//! can establish account, selection, registration or provider capabilities.
use super::*;
use mdreader_connector_dropbox::native as dropbox;
use mdreader_connector_google_drive::native as drive;
use mdreader_credentials::*;
use mdreader_provider::transport::{self, Method, Request, Transport};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;
#[async_trait::async_trait]
pub trait ProviderFactory: Send + Sync {
    /// Called only after real request admission/drain. Custom factories must
    /// explicitly supply their custody barrier; absence is not readiness.
    async fn settle(&self, _context: &IoContext) -> Result<()> {
        Err(ProviderError::Unsupported)
    }
    async fn describe(
        &self,
        library: LibraryId,
        source: SourceId,
        name: DisplayText,
        reference: CredentialReference,
        context: &IoContext,
    ) -> Result<SourceConfiguration>;
    async fn open(
        &self,
        configuration: SourceConfiguration,
        authority: Arc<dyn ProviderAuthority>,
        known: Vec<Metadata>,
        context: &IoContext,
    ) -> Result<Arc<dyn DocumentProvider>>;
    /// Delete a stored grant from custody. `None` means none was stored, which
    /// lets an interrupted disconnect be retried. Factories without custody
    /// must not claim a deletion they cannot perform.
    async fn delete_credential(
        &self,
        _reference: &CredentialReference,
        _context: &IoContext,
    ) -> Result<Option<RetiredCredential>> {
        Err(ProviderError::Unsupported)
    }
    /// Best-effort provider revocation after local deletion; never fails the caller.
    async fn revoke_credential(
        &self,
        _retired: RetiredCredential,
        _context: &IoContext,
    ) -> Revocation {
        Revocation::NotAttempted
    }
}
/// Registrations are native owner configuration. Fresh OAuth grants must match
/// one exactly; an imported legacy grant is never accepted or upgraded.
pub struct NativeFactory {
    vault: VaultClient,
    registrations: Vec<Registration>,
    transport: Arc<dyn Transport>,
}
impl NativeFactory {
    pub fn new(
        vault: VaultClient,
        registrations: Vec<Registration>,
        transport: Arc<dyn Transport>,
    ) -> Result<Self> {
        if registrations.len() > 2
            || registrations.len() == 2
                && registrations[0].provider() == registrations[1].provider()
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self {
            vault,
            registrations,
            transport,
        })
    }
    async fn load(
        &self,
        reference: &CredentialReference,
        context: &IoContext,
    ) -> Result<ProviderGrant> {
        let grant = self
            .vault
            .get(reference, context)
            .await?
            .ok_or(ProviderError::ReconnectRequired)?;
        if !self
            .registrations
            .iter()
            .any(|r| r.digest() == grant.registration().digest())
        {
            return Err(ProviderError::Unauthorized);
        }
        if matches!(
            grant.selection(),
            NativeSelection::DriveReadOnlyFolder { .. }
        ) && grant.verified_scopes() != Some(mdreader_credentials::DRIVE_READONLY_SCOPE)
        {
            return Err(ProviderError::Unauthorized);
        }
        if matches!(grant.selection(), NativeSelection::DriveFolder { .. })
            && !matches!(
                grant.verified_scopes(),
                Some(DRIVE_READONLY_SCOPE | DRIVE_EDIT_SCOPE)
            )
        {
            return Err(ProviderError::Unauthorized);
        }
        Ok(grant)
    }
}
#[async_trait::async_trait]
impl ProviderFactory for NativeFactory {
    async fn settle(&self, context: &IoContext) -> Result<()> {
        self.vault.settle(context).await
    }
    async fn delete_credential(
        &self,
        reference: &CredentialReference,
        context: &IoContext,
    ) -> Result<Option<RetiredCredential>> {
        // An unreadable entry is still deleted; it just cannot be revoked.
        let grant = self.vault.get(reference, context).await.ok().flatten();
        self.vault.delete(reference, context).await?;
        Ok(grant.map(RetiredCredential::new))
    }
    async fn revoke_credential(
        &self,
        retired: RetiredCredential,
        context: &IoContext,
    ) -> Revocation {
        super::credential_retirement::revoke(
            retired,
            &self.registrations,
            &*self.transport,
            context,
        )
        .await
    }
    async fn describe(
        &self,
        library: LibraryId,
        source: SourceId,
        name: DisplayText,
        reference: CredentialReference,
        context: &IoContext,
    ) -> Result<SourceConfiguration> {
        let grant = self.load(&reference, context).await?;
        let scope = grant.selection().scope()?;
        let binding = SourceBinding::new(
            library,
            source,
            grant.account().clone(),
            Uuid::new_v4(),
            scope.digest(),
        )?;
        let capabilities = ProviderCapabilities {
            read: true,
            list: true,
            changes: matches!(
                grant.selection(),
                NativeSelection::DriveFiles { .. }
                    | NativeSelection::DriveFolder { .. }
                    | NativeSelection::DriveReadOnlyFolder { .. }
                    | NativeSelection::DropboxRoot { .. }
            ),
            assets: false,
            guarded_write: grant.permits_guarded_write(),
        };
        Ok(SourceConfiguration {
            descriptor: SourceDescriptor::new(
                binding,
                grant.registration().provider(),
                name,
                scope,
                capabilities,
                SourceHealth::Ready,
                Coverage::NeverIndexed,
            )?,
            credential_reference: OpaqueValue::new(reference.as_str())?,
            configuration_digest: grant.configuration_digest()?,
        })
    }
    async fn open(
        &self,
        mut configuration: SourceConfiguration,
        authority: Arc<dyn ProviderAuthority>,
        known: Vec<Metadata>,
        context: &IoContext,
    ) -> Result<Arc<dyn DocumentProvider>> {
        let reference =
            CredentialReference::new(configuration.credential_reference.as_str().into())?;
        authority
            .revalidate(configuration.descriptor.binding())
            .await?;
        let grant = self.load(&reference, context).await?;
        validate(&grant, &configuration)?;
        // A retained verified folder grant already permits changes.list. Enable
        // metadata deltas without replacing its binding, keys or OAuth scope.
        if matches!(
            grant.selection(),
            NativeSelection::DriveFolder { .. } | NativeSelection::DriveReadOnlyFolder { .. }
        ) {
            let d = &configuration.descriptor;
            let mut capabilities = d.capabilities();
            capabilities.changes = true;
            configuration.descriptor = SourceDescriptor::new(
                d.binding().clone(),
                d.kind(),
                d.name().clone(),
                d.scope().clone(),
                capabilities,
                d.health(),
                d.coverage().clone(),
            )?;
        }
        let resolver = Arc::new(Resolver {
            vault: self.vault.clone(),
            reference,
            binding: configuration.descriptor.binding().clone(),
            digest: configuration.configuration_digest,
            // Custody deliberately excludes desktop token-endpoint parameters.
            // Recover them only from this owner's matching registration.
            registration: self
                .registrations
                .iter()
                .find(|r| r.digest() == grant.registration().digest())
                .cloned()
                .ok_or(ProviderError::Unauthorized)?,
            authority: authority.clone(),
            transport: self.transport.clone(),
            refresh: tokio::sync::Mutex::new(()),
        });
        let key = grant.secret().cursor_key();
        let client: Arc<dyn DocumentProvider> = match grant.selection() {
            NativeSelection::DriveFiles { files } => Arc::new(drive::Client::new(
                configuration.descriptor,
                files
                    .iter()
                    .map(|f| drive::SelectedFile::new(f.id.clone(), f.resource_key.clone()))
                    .collect::<Result<_>>()?,
                resolver,
                authority.clone(),
                self.transport.clone(),
                key,
            )?),
            NativeSelection::DriveReadOnlyFolder { folder } => Arc::new(
                drive::Client::new_readonly_folder(
                    configuration.descriptor,
                    folder.clone(),
                    resolver,
                    authority.clone(),
                    self.transport.clone(),
                    key,
                )?
                .with_known_metadata(known)?,
            ),
            NativeSelection::DriveFolder { folder, recursive } => Arc::new(
                drive::Client::new_folder(
                    configuration.descriptor,
                    folder.clone(),
                    *recursive,
                    resolver,
                    authority.clone(),
                    self.transport.clone(),
                    key,
                )?
                .with_known_metadata(known)?,
            ),
            NativeSelection::DropboxExact { files } => Arc::new(dropbox::Client::new(
                configuration.descriptor,
                dropbox::Selection::ExactFiles(
                    files
                        .iter()
                        .map(|f| dropbox::SelectedFile::new(f.id.clone(), f.path.clone()))
                        .collect::<Result<_>>()?,
                ),
                resolver,
                authority.clone(),
                self.transport.clone(),
                key,
            )?),
            NativeSelection::DropboxRoot { path, recursive } => {
                let client = dropbox::Client::new(
                    configuration.descriptor,
                    dropbox::Selection::AppFolderRoot {
                        path: path.clone(),
                        recursive: *recursive,
                    },
                    resolver,
                    authority.clone(),
                    self.transport.clone(),
                    key,
                )?;
                let restored = known
                    .iter()
                    .map(|m| {
                        dropbox::SelectedFile::new(
                            m.key.clone(),
                            RecordKey::new(m.display_path.as_str())?,
                        )
                    })
                    .collect::<Result<Vec<_>>>()?;
                client.restore_membership(client.descriptor().binding(), &restored)?;
                Arc::new(client)
            }
        };
        authority.revalidate(client.descriptor().binding()).await?;
        Ok(client)
    }
}
fn validate(grant: &ProviderGrant, c: &SourceConfiguration) -> Result<()> {
    if grant.configuration_digest()? != c.configuration_digest
        || grant.account() != c.descriptor.binding().account()
        || grant.selection().scope()? != *c.descriptor.scope()
        || grant.registration().provider() != c.descriptor.kind()
        || grant.permits_guarded_write() != c.descriptor.capabilities().guarded_write
    {
        return Err(ProviderError::IdentityMismatch);
    }
    Ok(())
}
struct Resolver {
    vault: VaultClient,
    reference: CredentialReference,
    binding: SourceBinding,
    digest: [u8; 32],
    registration: Registration,
    authority: Arc<dyn ProviderAuthority>,
    transport: Arc<dyn Transport>,
    refresh: tokio::sync::Mutex<()>,
}
impl Resolver {
    async fn load(&self, b: &SourceBinding, c: &IoContext) -> Result<ProviderGrant> {
        if b != &self.binding {
            return Err(ProviderError::Unauthorized);
        }
        self.authority.revalidate(b).await?;
        let grant = self
            .vault
            .get(&self.reference, c)
            .await?
            .ok_or(ProviderError::ReconnectRequired)?;
        if grant.configuration_digest()? != self.digest
            || grant.account() != b.account()
            || grant.registration().digest() != self.registration.digest()
        {
            return Err(ProviderError::IdentityMismatch);
        }
        self.authority.revalidate(b).await?;
        Ok(grant)
    }
    async fn rotate(
        &self,
        b: &SourceBinding,
        previous: u64,
        c: &IoContext,
    ) -> Result<ProviderGrant> {
        let _guard = tokio::select! {_=c.cancellation().cancelled()=>return Err(ProviderError::Cancelled),g=tokio::time::timeout_at(tokio::time::Instant::from_std(c.deadline()),self.refresh.lock())=>g.map_err(|_|ProviderError::Timeout)?};
        let prior = self.load(b, c).await?;
        if prior.secret().generation() != previous {
            return Ok(prior);
        }
        let form = Zeroizing::new({
            let mut form = url::form_urlencoded::Serializer::new(String::new());
            form.append_pair("grant_type", "refresh_token")
                .append_pair("refresh_token", prior.secret().refresh_token())
                .append_pair("client_id", self.registration.client_id());
            if let Some(secret) = self.registration.google_desktop_client_secret() {
                form.append_pair("client_secret", secret);
            }
            form.finish()
        });
        let endpoint = match self.registration.provider() {
            ProviderKind::GoogleDrive => "https://oauth2.googleapis.com/token",
            ProviderKind::Dropbox => "https://api.dropboxapi.com/oauth2/token",
        };
        let response = transport::send(
            &*self.transport,
            Request::new(
                Method::Post,
                endpoint.into(),
                BTreeMap::from([(
                    "Content-Type".into(),
                    "application/x-www-form-urlencoded".into(),
                )]),
                form.as_bytes().to_vec(),
            )?,
            c,
            None,
        )
        .await?;
        if response.status() != 200 {
            return Err(ProviderError::ReconnectRequired);
        }
        #[derive(serde::Deserialize)]
        struct Token {
            access_token: String,
            refresh_token: Option<String>,
            expires_in: u64,
            token_type: String,
            scope: Option<String>,
        }
        let body = Zeroizing::new(response.into_body());
        let mut token: Token =
            serde_json::from_slice(&body).map_err(|_| ProviderError::Protocol)?;
        let access = Zeroizing::new(std::mem::take(&mut token.access_token));
        let refresh = token.refresh_token.take().map(Zeroizing::new);
        if !token.token_type.eq_ignore_ascii_case("bearer")
            || token.expires_in == 0
            || token.expires_in > 86400
        {
            return Err(ProviderError::Protocol);
        }
        if let Some(scope) = token.scope {
            let valid = prior.matches_verified_scopes(&scope);
            if !valid {
                return Err(ProviderError::Unauthorized);
            }
        }
        // Verify stable provider account under the refreshed access token before CAS.
        let (endpoint, method, payload) = match self.registration.provider() {
            ProviderKind::GoogleDrive => (
                "https://www.googleapis.com/drive/v3/about?fields=user(permissionId)",
                Method::Get,
                Vec::new(),
            ),
            ProviderKind::Dropbox => (
                "https://api.dropboxapi.com/2/users/get_current_account",
                Method::Post,
                b"null".to_vec(),
            ),
        };
        let account = transport::send(
            &*self.transport,
            Request::new(
                method,
                endpoint.into(),
                BTreeMap::from([
                    (
                        "Authorization".into(),
                        format!("Bearer {}", access.as_str()),
                    ),
                    ("Content-Type".into(), "application/json".into()),
                ]),
                payload,
            )?,
            c,
            None,
        )
        .await?;
        if account.status() != 200 {
            return Err(ProviderError::ReconnectRequired);
        }
        let body = Zeroizing::new(account.into_body());
        let identity: serde_json::Value =
            serde_json::from_slice(&body).map_err(|_| ProviderError::Protocol)?;
        let id = match self.registration.provider() {
            ProviderKind::GoogleDrive => identity.pointer("/user/permissionId"),
            ProviderKind::Dropbox => identity.get("account_id"),
        }
        .and_then(|v| v.as_str())
        .ok_or(ProviderError::Protocol)?;
        if id != b.account().as_str() {
            return Err(ProviderError::IdentityMismatch);
        }
        self.authority.revalidate(b).await?;
        let expires = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ProviderError::Protocol)?
            .as_secs()
            .checked_add(token.expires_in)
            .ok_or(ProviderError::Invalid)?;
        let updated =
            prior.refreshed(access.to_string(), refresh.map(|s| s.to_string()), expires)?;
        self.vault
            .compare_exchange(&self.reference, previous, &updated, c)
            .await?;
        self.authority.revalidate(b).await?;
        Ok(updated)
    }
    async fn access(&self, b: &SourceBinding, c: &IoContext) -> Result<ProviderGrant> {
        let grant = self.load(b, c).await?;
        if grant.secret().expires_at() <= SystemTime::now() + Duration::from_secs(30) {
            self.rotate(b, grant.secret().generation(), c).await
        } else {
            Ok(grant)
        }
    }
}
#[async_trait::async_trait]
impl drive::CredentialResolver for Resolver {
    async fn access(&self, b: &SourceBinding, c: &IoContext) -> Result<drive::AccessGrant> {
        let g = self.access(b, c).await?;
        drive::AccessGrant::new(
            g.secret().access_token().into(),
            g.secret().expires_at(),
            g.secret().generation(),
        )
    }
    async fn refresh(
        &self,
        b: &SourceBinding,
        n: u64,
        c: &IoContext,
    ) -> Result<drive::AccessGrant> {
        let g = self.rotate(b, n, c).await?;
        drive::AccessGrant::new(
            g.secret().access_token().into(),
            g.secret().expires_at(),
            g.secret().generation(),
        )
    }
}
#[async_trait::async_trait]
impl dropbox::CredentialResolver for Resolver {
    async fn access(&self, b: &SourceBinding, c: &IoContext) -> Result<dropbox::AccessGrant> {
        let g = self.access(b, c).await?;
        dropbox::AccessGrant::new(
            g.secret().access_token().into(),
            g.secret().expires_at(),
            g.secret().generation(),
        )
    }
    async fn refresh(
        &self,
        b: &SourceBinding,
        n: u64,
        c: &IoContext,
    ) -> Result<dropbox::AccessGrant> {
        let g = self.rotate(b, n, c).await?;
        dropbox::AccessGrant::new(
            g.secret().access_token().into(),
            g.secret().expires_at(),
            g.secret().generation(),
        )
    }
}
