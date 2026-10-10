//! Read-only original-operation proof. Filename alone never establishes creation.
use super::*;
use creation::{RecoveryDiagnostic as D, RecoveryObservation as O};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Match {
    #[serde(flatten)]
    file: File,
    #[serde(default)]
    app_properties: BTreeMap<String, String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Matches {
    files: Vec<Match>,
    next_page_token: Option<String>,
    #[serde(default)]
    incomplete_search: bool,
}
impl Client {
    async fn created_match(
        &self,
        id: uuid::Uuid,
        root: &RecordKey,
        context: &IoContext,
        spent: &mut usize,
    ) -> Result<std::result::Result<File, D>> {
        let query = format!(
            "trashed = false and '{}' in parents and appProperties has {{ key='mdreaderCreateOperation' and value='{}' }}",
            root.as_str(),
            id
        );
        let mut url = url::Url::parse("https://www.googleapis.com/drive/v3/files")
            .map_err(|_| ProviderError::Invalid)?;
        url.query_pairs_mut().append_pair("q",&query).append_pair("pageSize","2").append_pair("spaces","drive").append_pair("supportsAllDrives","true").append_pair("includeItemsFromAllDrives","true").append_pair("fields","nextPageToken,incompleteSearch,files(id,name,mimeType,version,md5Checksum,size,modifiedTime,trashed,parents,appProperties)");
        let response = self
            .send(
                Method::Get,
                url.into(),
                BTreeMap::new(),
                vec![],
                context,
                None,
                spent,
            )
            .await?;
        if response.status() != 200 {
            return Err(membership::metadata_failure(&response));
        }
        let mut matches: Matches =
            serde_json::from_slice(response.body()).map_err(|_| ProviderError::Protocol)?;
        if matches.files.len() > 2 {
            return Err(ProviderError::Protocol);
        }
        if matches.next_page_token.is_some() || matches.incomplete_search || matches.files.len() > 1
        {
            return Ok(Err(D::AmbiguousOperation));
        }
        let Some(matched) = matches.files.pop() else {
            return Ok(Err(D::NoMatchingOperation));
        };
        if matched.app_properties.get("mdreaderCreateOperation") != Some(&id.to_string())
            || matched.file.parents != vec![root.as_str().to_owned()]
            || matched.file.trashed
        {
            return Ok(Err(D::MetadataMismatch));
        }
        Ok(Ok(matched.file))
    }
    pub(super) async fn observe_created(
        &self,
        id: uuid::Uuid,
        filename: &str,
        body: &MarkdownBytes,
        context: &IoContext,
    ) -> Result<O> {
        creation::validate_filename(filename)?;
        if id.get_version_num() != 4 {
            return Err(ProviderError::Invalid);
        }
        if !self.descriptor.capabilities().guarded_write
            || !matches!(self.descriptor.scope(), SelectedScope::DriveFolder { .. })
        {
            return Err(ProviderError::Unsupported);
        }
        let mut spent = 0;
        self.observe_created_budget(id, filename, body, context, &mut spent, None)
            .await
    }
    /// Same original-operation proof for post-create reconciliation. The caller
    /// retains the dispatched key/root and the entire effect transfer budget.
    pub(super) async fn observe_created_budget(
        &self,
        id: uuid::Uuid,
        filename: &str,
        body: &MarkdownBytes,
        context: &IoContext,
        spent: &mut usize,
        expected: Option<(&RecordKey, &RecordKey)>,
    ) -> Result<O> {
        let root = self.root_id(context, spent).await?;
        if expected.is_some_and(|(_, selected)| selected != &root) {
            return Ok(O::Unresolved(D::MetadataMismatch));
        }
        let first = match self.created_match(id, &root, context, spent).await? {
            Ok(file) => file,
            Err(phase) => return Ok(O::Unresolved(phase)),
        };
        let key = RecordKey::new(first.id.clone())?;
        if expected.is_some_and(|(created, _)| created != &key) {
            return Ok(O::Unresolved(D::MetadataMismatch));
        }
        if first.validate(&key).is_err()
            || first.name != filename
            || first.size.as_deref().and_then(|n| n.parse::<u64>().ok())
                != Some(body.as_bytes().len() as u64)
        {
            return Ok(O::Unresolved(D::MetadataMismatch));
        }
        // Native read verifies UTF-8, exact revision, hash and fresh ancestry
        // on both sides of the body request. The body never leaves this adapter.
        let observed = self.read_document(&key, context, spent).await?;
        if observed.body != *body {
            return Ok(O::Unresolved(D::BodyMismatch));
        }
        let last = match self.created_match(id, &root, context, spent).await? {
            Ok(file) => file,
            Err(phase) => return Ok(O::Unresolved(phase)),
        };
        if last.validate(&key).is_err()
            || last.id != first.id
            || last.name != filename
            || last.size != first.size
            || last.version != observed.version.provider_revision.as_str()
        {
            return Ok(O::Unresolved(D::MetadataMismatch));
        }
        let version = last.version(None)?;
        if version.content_hash != observed.version.content_hash {
            return Ok(O::Unresolved(D::MetadataMismatch));
        }
        if self.root_id(context, spent).await? != root {
            return Ok(O::Unresolved(D::MetadataMismatch));
        }
        let mut metadata = last.metadata(observed.version)?;
        metadata.membership_ancestors = Some(vec![root]);
        let selected = self.folder.as_ref().ok_or(ProviderError::Unsupported)?;
        metadata.display_path =
            DisplayText::new(format!("/{}/{}", selected.root.as_str(), key.as_str()))?;
        context.check()?;
        revalidate_authority(&*self.authority, self.descriptor.binding(), context, None).await?;
        Ok(O::Confirmed(Box::new(metadata)))
    }
}
