//! Explicit review/apply transport. The service's persisted grant is authority.
//!
//! Prepare changes only private preparation/installer state, never user configuration.
//! The opaque UUID is not application authority: apply is a separate explicit request
//! and revalidates the complete DB grant, service artifact, selected sources,
//! executable and directory identities, and exact reviewed configuration bytes.
//! Current-executable selection prefers the packaged service binary; capabilities
//! are caller claims and are always reported as unverified. No client is executed.
//!
//! SQLite records use the existing owner's borrowed connection and separate v1
//! migration guard. Intent is committed before filesystem publication/removal;
//! installer receipts recover interrupted publication, then DB phase is committed.
//! At most 64 live preparations and 32 MiB are admitted. The eight newest closed
//! preparations by creation order are retained for short-term inspection; older
//! closed snapshots are pruned transactionally. Storage pressure may prune more
//! closed rows, never live ownership. Pruned IDs cannot reactivate. Each record
//! is at most 4 MiB and each
//! configuration at most 512 KiB. Records contain private configuration snapshots;
//! never log, sync, or expose them through ownership listing.
//!
//! Uninstall deliberately survives capture revocation and verifies only recorded
//! filesystem/receipt ownership. Windows uses canonical SQLite phase receipts
//! and retained local NTFS identity. The underlying configuration publisher still has the documented noncooperating editor
//! compare/rename race; this transport does not claim to eliminate that gate.
use mdreader_capture::ProjectPolicy;
use mdreader_core::{ReaderError, ReaderStore, Result, SourceRoot, SourceRootIdentity};
use mdreader_integrations::{
    Capabilities, InstallRequest, Installer, IntegrationError, IntegrationKind,
};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    ClaudeHooks,
    ClaudeMcp,
    CodexHooks,
    CodexMcp,
    CodexNotify,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientCapabilities {
    pub local_commands: bool,
    pub claude_exec_hooks: bool,
    pub codex_lifecycle_hooks: bool,
    pub codex_notify: bool,
    pub client_version: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub kind: Kind,
    pub project_id: String,
    pub worktree_root: PathBuf,
    /// None chooses the current packaged service executable.
    pub executable: Option<PathBuf>,
    pub source_ids: Vec<String>,
    /// Explicit compatibility claims; these are not runtime probe evidence.
    pub capabilities: ClientCapabilities,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewDiff {
    pub offset_bytes: usize,
    pub remove_bytes: usize,
    pub insert_text: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Review {
    pub preparation_id: String,
    pub config_path: PathBuf,
    pub policy_path: PathBuf,
    pub before_text: Option<String>,
    pub after_text: String,
    pub diff: ReviewDiff,
    pub capabilities_verified: bool,
}
const MAX_RECORD: usize = 4 * 1024 * 1024;
const MAX_CONFIG: usize = 512 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RootGrant {
    path: PathBuf,
    identity: SourceRootIdentity,
}
impl RootGrant {
    fn from_root(root: &SourceRoot) -> Self {
        Self {
            path: root.path.clone(),
            identity: root.identity.clone(),
        }
    }
    fn restore(&self) -> Result<SourceRoot> {
        Ok(SourceRoot::restore(
            self.path.clone(),
            self.identity.clone(),
        )?)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredGrant {
    policy: ProjectPolicy,
    roots: Vec<RootGrant>,
    source_id: Option<String>,
    projection_root: Option<RootGrant>,
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SelectedRoot {
    id: String,
    root: RootGrant,
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ExecutableGrant {
    directory: RootGrant,
    name: PathBuf,
    length: u64,
    modified_ns: String,
    device: u64,
    inode: u64,
    mode: u32,
    #[cfg(windows)]
    identity: mdreader_capture::WindowsFileIdentity,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preparation {
    v: u8,
    id: String,
    selection: Selection,
    profile: RootGrant,
    worktree: RootGrant,
    authority: String,
    sources: Vec<SelectedRoot>,
    executable: ExecutableGrant,
    config_path: PathBuf,
    policy_path: PathBuf,
    before: Option<String>,
    after: String,
}
fn invalid() -> ReaderError {
    super::protocol_error("Integration review, policy or capability is unavailable")
}
fn integration_error(error: IntegrationError) -> ReaderError {
    match error {
        IntegrationError::Conflict => ReaderError::Conflict,
        IntegrationError::Io(error) => ReaderError::Io(error),
        _ => invalid(),
    }
}
fn encode<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|_| invalid())
}
fn host() -> Result<()> {
    if cfg!(any(unix, windows)) {
        Ok(())
    } else {
        Err(invalid())
    }
}
impl Kind {
    fn native(self) -> IntegrationKind {
        match self {
            Self::ClaudeHooks => IntegrationKind::ClaudeHooks,
            Self::ClaudeMcp => IntegrationKind::ClaudeMcp,
            Self::CodexHooks => IntegrationKind::CodexHooks,
            Self::CodexMcp => IntegrationKind::CodexMcp,
            Self::CodexNotify => IntegrationKind::CodexNotify,
        }
    }
    fn capture(self) -> bool {
        matches!(
            self,
            Self::ClaudeHooks | Self::CodexHooks | Self::CodexNotify
        )
    }
}
fn grant(store: &ReaderStore, project: &str) -> Result<Option<StoredGrant>> {
    if project.trim().is_empty() || project.len() > 256 {
        return Err(invalid());
    }
    let Some((_, json)) = store
        .capture_policies()?
        .into_iter()
        .find(|(id, _)| id == project)
    else {
        return Ok(None);
    };
    let grant: StoredGrant = serde_json::from_str(&json).map_err(|_| invalid())?;
    grant.policy.validate().map_err(|_| invalid())?;
    if grant.policy.project_id != project {
        return Err(invalid());
    }
    Ok(Some(grant))
}
/// Settings inspection is read-only and never creates folders or repairs an artifact.
pub fn capture_policy(store: &ReaderStore, project_id: &str) -> Result<Option<ProjectPolicy>> {
    Ok(grant(store, project_id)?.map(|grant| grant.policy))
}
#[cfg(any(unix, windows))]
fn source_roots(store: &mut ReaderStore, ids: &[String]) -> Result<Vec<SelectedRoot>> {
    if ids.len() > 128 || ids.iter().collect::<std::collections::BTreeSet<_>>().len() != ids.len() {
        return Err(invalid());
    }
    let mut result = Vec::new();
    for id in ids {
        // scope() verifies enabled admission and persisted filesystem identity.
        store.index_status(std::slice::from_ref(id))?;
        let rows: Vec<(Vec<u8>, String)> =
            store.with_library_connection(|connection| -> Result<_> {
                Ok(connection
                    .prepare("SELECT path,identity FROM sources WHERE id=?1 AND enabled=1")?
                    .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<std::result::Result<Vec<_>, _>>()?)
            })?;
        let [(path, identity)] = rows.as_slice() else {
            return Err(invalid());
        };
        #[cfg(unix)]
        let path = {
            use std::os::unix::ffi::OsStringExt;
            PathBuf::from(std::ffi::OsString::from_vec(path.clone()))
        };
        #[cfg(windows)]
        let path = {
            use std::os::windows::ffi::OsStringExt;
            if path.len() % 2 != 0 {
                return Err(invalid());
            }
            let wide: Vec<u16> = path
                .chunks_exact(2)
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .collect();
            PathBuf::from(std::ffi::OsString::from_wide(&wide))
        };
        let identity: SourceRootIdentity = serde_json::from_str(identity).map_err(|_| invalid())?;
        let root = SourceRoot::restore(path, identity)?;
        root.verify()?;
        result.push(SelectedRoot {
            id: id.clone(),
            root: RootGrant::from_root(&root),
        });
    }
    Ok(result)
}
#[cfg(not(any(unix, windows)))]
fn source_roots(_: &mut ReaderStore, _: &[String]) -> Result<Vec<SelectedRoot>> {
    Err(invalid())
}
fn authority(
    store: &mut ReaderStore,
    selection: &Selection,
) -> Result<(StoredGrant, Vec<SelectedRoot>, SourceRoot)> {
    let grant = grant(store, &selection.project_id)?.ok_or_else(invalid)?;
    if selection.kind.capture() && !grant.policy.enabled {
        return Err(invalid());
    }
    if !selection.worktree_root.is_absolute()
        || std::fs::symlink_metadata(&selection.worktree_root)?
            .file_type()
            .is_symlink()
    {
        return Err(invalid());
    }
    #[cfg(windows)]
    let selected = mdreader_capture::SelectedDirectory::open(&selection.worktree_root)
        .map_err(|_| invalid())?;
    let worktree = SourceRoot::capture(selection.worktree_root.canonicalize()?)?;
    #[cfg(windows)]
    selected.verify().map_err(|_| invalid())?;
    if !grant
        .policy
        .worktree_roots
        .iter()
        .any(|path| path.canonicalize().is_ok_and(|p| p == worktree.path))
    {
        return Err(invalid());
    }
    for root in &grant.roots {
        root.restore()?.verify()?;
    }
    let sources = source_roots(store, &selection.source_ids)?;
    if !selection.kind.capture() && sources.is_empty() {
        return Err(invalid());
    }
    let admitted = grant
        .roots
        .iter()
        .chain(sources.iter().map(|source| &source.root))
        // Path-based: a renumbered device/inode is still the admitted folder.
        .any(|root| root.path == worktree.path);
    if !admitted {
        return Err(invalid());
    }
    if selection.kind.capture() {
        store.index_status(&[grant.source_id.clone().ok_or_else(invalid)?])?;
    }
    worktree.verify()?;
    Ok((grant, sources, worktree))
}
#[cfg(unix)]
fn executable(path: &Path) -> Result<ExecutableGrant> {
    use std::os::unix::fs::MetadataExt;
    if !path.is_absolute() || std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(invalid());
    }
    let name = PathBuf::from(path.file_name().ok_or_else(invalid)?);
    let root = SourceRoot::capture(path.parent().ok_or_else(invalid)?.canonicalize()?)?;
    let file = root
        .open_optional_file_relative(&name)?
        .ok_or_else(invalid)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.mode() & 0o111 == 0
        || metadata.len() == 0
    {
        return Err(invalid());
    }
    root.verify()?;
    Ok(ExecutableGrant {
        directory: RootGrant::from_root(&root),
        name,
        length: metadata.len(),
        modified_ns: metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| invalid())?
            .as_nanos()
            .to_string(),
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
    })
}
#[cfg(windows)]
fn executable(path: &Path) -> Result<ExecutableGrant> {
    if !path.is_absolute()
        || !path
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("exe"))
    {
        return Err(invalid());
    }
    let leaf = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid)?;
    if leaf.contains(':') || leaf.ends_with(['.', ' ']) || leaf.chars().any(char::is_control) {
        return Err(invalid());
    }
    let parent = path.parent().ok_or_else(invalid)?;
    let selected = mdreader_capture::SelectedDirectory::open(parent).map_err(|_| invalid())?;
    let root = SourceRoot::capture(parent.to_owned())?;
    let name = PathBuf::from(path.file_name().ok_or_else(invalid)?);
    let mut file = root
        .open_optional_file_relative(&name)?
        .ok_or_else(invalid)?;
    let identity = mdreader_capture::selected_file_identity(&file).map_err(|_| invalid())?;
    let metadata = file.metadata()?;
    let mut magic = [0u8; 2];
    file.read_exact(&mut magic)?;
    if metadata.len() == 0 || magic != *b"MZ" {
        return Err(invalid());
    }
    let named = root
        .open_optional_file_relative(&name)?
        .ok_or_else(invalid)?;
    if mdreader_capture::selected_file_identity(&named).map_err(|_| invalid())? != identity {
        return Err(ReaderError::Conflict);
    }
    selected.verify().map_err(|_| invalid())?;
    root.verify()?;
    Ok(ExecutableGrant {
        directory: RootGrant::from_root(&root),
        name,
        length: metadata.len(),
        modified_ns: metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| invalid())?
            .as_nanos()
            .to_string(),
        device: 0,
        inode: 0,
        mode: 0,
        identity,
    })
}
#[cfg(not(any(unix, windows)))]
fn executable(_: &Path) -> Result<ExecutableGrant> {
    Err(invalid())
}
fn request(directory: &Path, selection: &Selection) -> Result<InstallRequest> {
    Ok(InstallRequest {
        kind: selection.kind.native(),
        worktree_root: selection.worktree_root.clone(),
        executable: selection.executable.clone().ok_or_else(invalid)?,
        data_dir: directory.canonicalize()?,
        policy_path: super::capture::policy_path(&directory.canonicalize()?, &selection.project_id),
        spool_root: directory.canonicalize()?.join("captures"),
        source_ids: selection.source_ids.clone(),
        mcp_grant_id: None,
        capabilities: Capabilities {
            local_commands: selection.capabilities.local_commands,
            claude_exec_hooks: selection.capabilities.claude_exec_hooks,
            codex_lifecycle_hooks: selection.capabilities.codex_lifecycle_hooks,
            codex_notify: selection.capabilities.codex_notify,
            client_version: selection.capabilities.client_version.clone(),
        },
    })
}
#[cfg(windows)]
fn installer(directory: &Path) -> Result<Installer> {
    let profile = mdreader_capture::PrivateDirectory::open(directory).map_err(|_| invalid())?;
    let state = directory.join("integrations");
    let state_guard = mdreader_capture::PrivateDirectory::create(&state).map_err(|_| invalid())?;
    let installer = Installer::open(&state.join("settings")).map_err(integration_error)?;
    profile.verify().map_err(|_| invalid())?;
    state_guard.verify().map_err(|_| invalid())?;
    Ok(installer)
}
#[cfg(not(windows))]
fn installer(directory: &Path) -> Result<Installer> {
    let profile = SourceRoot::capture(directory.canonicalize()?)?;
    let state = profile.create_or_capture_directory_relative(Path::new("integrations"))?;
    profile.try_clone_directory_handle()?.sync_all()?;
    state.try_clone_directory_handle()?.sync_all()?;
    Installer::open(&state.path.join("settings")).map_err(integration_error)
}
fn initialize(store: &mut ReaderStore) -> Result<()> {
    store.with_library_connection(|connection|->Result<()>{
        let transaction=connection.transaction()?;
        let tables:u64=transaction.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('mdreader_service_integrations_meta','mdreader_service_integrations_preparations')",[],|row|row.get(0))?;
        if tables==0 {
            transaction.execute_batch("CREATE TABLE mdreader_service_integrations_meta(id INTEGER PRIMARY KEY CHECK(id=1),version INTEGER NOT NULL); INSERT INTO mdreader_service_integrations_meta VALUES(1,1); CREATE TABLE mdreader_service_integrations_preparations(id TEXT PRIMARY KEY,record TEXT NOT NULL,digest TEXT NOT NULL,phase TEXT NOT NULL CHECK(phase IN ('prepared','applying','applied','removing','removed')),bytes INTEGER NOT NULL CHECK(bytes>0));")?;
        }else{
            if tables!=2||transaction.query_row("SELECT version FROM mdreader_service_integrations_meta WHERE id=1",[],|row|row.get::<_,u64>(0))?!=1{return Err(invalid());}
            transaction.prepare("SELECT id,record,digest,phase,bytes FROM mdreader_service_integrations_preparations LIMIT 0")?;
        }
        transaction.commit()?;Ok(())
    })
}
fn save(store: &mut ReaderStore, preparation: &Preparation) -> Result<()> {
    initialize(store)?;
    let json = encode(preparation)?;
    if json.len() > MAX_RECORD {
        return Err(invalid());
    }
    let digest = mdreader_sync::blob_digest(json.as_bytes());
    store.with_library_connection(|connection|->Result<()>{
        let transaction=connection.transaction()?;
        transaction.execute("DELETE FROM mdreader_service_integrations_preparations WHERE phase='removed' AND id NOT IN (SELECT id FROM mdreader_service_integrations_preparations WHERE phase='removed' ORDER BY rowid DESC LIMIT 8)",[])?;
        let (count,mut bytes):(u64,u64)=transaction.query_row("SELECT count(*),coalesce(sum(bytes),0) FROM mdreader_service_integrations_preparations WHERE phase!='removed'",[],|row|Ok((row.get(0)?,row.get(1)?)))?;
        if count>=64{return Err(invalid());}
        bytes+=transaction.query_row("SELECT coalesce(sum(bytes),0) FROM mdreader_service_integrations_preparations WHERE phase='removed'",[],|row|row.get::<_,u64>(0))?;
        while bytes.saturating_add(json.len()as u64)>32*1024*1024 {
            if transaction.execute("DELETE FROM mdreader_service_integrations_preparations WHERE rowid=(SELECT rowid FROM mdreader_service_integrations_preparations WHERE phase='removed' ORDER BY rowid LIMIT 1)",[])?==0{return Err(invalid());}
            bytes=transaction.query_row("SELECT coalesce(sum(bytes),0) FROM mdreader_service_integrations_preparations",[],|row|row.get(0))?;
        }
        transaction.execute("INSERT INTO mdreader_service_integrations_preparations VALUES(?1,?2,?3,'prepared',?4)",(&preparation.id,&json,&digest,json.len()as u64))?;
        transaction.commit()?;Ok(())
    })
}
fn load(store: &mut ReaderStore, id: &str) -> Result<(Preparation, String)> {
    let uuid = uuid::Uuid::parse_str(id).map_err(|_| invalid())?;
    if uuid.get_version_num() != 4 || uuid.to_string() != id {
        return Err(invalid());
    }
    initialize(store)?;
    let (json,digest,phase):(String,String,String)=store.with_library_connection(|connection|->Result<_>{Ok(connection.query_row("SELECT record,digest,phase FROM mdreader_service_integrations_preparations WHERE id=?1",[id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?)})?;
    if json.len() > MAX_RECORD || mdreader_sync::blob_digest(json.as_bytes()) != digest {
        return Err(invalid());
    }
    let record: Preparation = serde_json::from_str(&json).map_err(|_| invalid())?;
    if record.v != 1
        || record.id != id
        || record.after.len() > MAX_CONFIG
        || record.before.as_ref().is_some_and(|s| s.len() > MAX_CONFIG)
    {
        return Err(invalid());
    }
    Ok((record, phase))
}
fn phase(store: &mut ReaderStore, id: &str, expected: &str, next: &str) -> Result<()> {
    store.with_library_connection(|connection|->Result<()>{
        let transaction=connection.transaction()?;
        if transaction.execute("UPDATE mdreader_service_integrations_preparations SET phase=?1 WHERE id=?2 AND phase=?3",[next,id,expected])?!=1{return Err(ReaderError::Conflict);}
        if next=="removed" {transaction.execute("DELETE FROM mdreader_service_integrations_preparations WHERE phase='removed' AND id NOT IN (SELECT id FROM mdreader_service_integrations_preparations WHERE phase='removed' ORDER BY rowid DESC LIMIT 8)",[])?;}
        transaction.commit()?;Ok(())
    })
}
fn diff(before: &str, after: &str) -> ReviewDiff {
    let mut offset = before
        .bytes()
        .zip(after.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(offset) || !after.is_char_boundary(offset) {
        offset -= 1;
    }
    let mut suffix = before[offset..]
        .bytes()
        .rev()
        .zip(after[offset..].bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(before.len() - suffix)
        || !after.is_char_boundary(after.len() - suffix)
    {
        suffix -= 1;
    }
    ReviewDiff {
        offset_bytes: offset,
        remove_bytes: before.len() - offset - suffix,
        insert_text: after[offset..after.len() - suffix].into(),
    }
}
pub fn prepare(
    store: &mut ReaderStore,
    directory: &Path,
    mut selection: Selection,
) -> Result<Review> {
    host()?;
    selection.source_ids.sort();
    let (grant, sources, worktree) = authority(store, &selection)?;
    selection.worktree_root = worktree.path.clone();
    let chosen = selection
        .executable
        .clone()
        .map(Ok)
        .unwrap_or_else(std::env::current_exe)?;
    let executable = executable(&chosen)?;
    selection.executable = Some(executable.directory.path.join(&executable.name));
    let request = request(directory, &selection)?;
    let installer = installer(directory)?;
    let plan = installer
        .prepare(&request, &grant.policy)
        .map_err(integration_error)?;
    let before = plan
        .before_bytes()
        .map(|bytes| String::from_utf8(bytes.to_vec()).map_err(|_| invalid()))
        .transpose()?;
    let after = String::from_utf8(plan.after_bytes().to_vec()).map_err(|_| invalid())?;
    let preparation_id = uuid::Uuid::new_v4().to_string();
    let review = Review {
        preparation_id: preparation_id.clone(),
        config_path: plan.config_path().to_owned(),
        policy_path: plan.policy_path().to_owned(),
        before_text: before.clone(),
        after_text: after.clone(),
        diff: diff(before.as_deref().unwrap_or_default(), &after),
        capabilities_verified: false,
    };
    let record = Preparation {
        v: 1,
        id: preparation_id,
        selection,
        profile: RootGrant::from_root(&SourceRoot::capture(directory.canonicalize()?)?),
        worktree: RootGrant::from_root(&worktree),
        authority: encode(&grant)?,
        sources,
        executable,
        config_path: review.config_path.clone(),
        policy_path: review.policy_path.clone(),
        before,
        after,
    };
    save(store, &record)?;
    Ok(review)
}
pub fn apply(
    store: &mut ReaderStore,
    directory: &Path,
    preparation_id: &str,
) -> Result<serde_json::Value> {
    host()?;
    let (record, current) = load(store, preparation_id)?;
    if !matches!(current.as_str(), "prepared" | "applying" | "applied") {
        return Err(ReaderError::Conflict);
    }
    let profile = record.profile.restore()?;
    if profile.path != directory.canonicalize()? {
        return Err(ReaderError::Conflict);
    }
    let (grant, sources, worktree) = authority(store, &record.selection)?;
    if encode(&grant)? != record.authority
        || sources != record.sources
        || RootGrant::from_root(&worktree) != record.worktree
        || executable(record.selection.executable.as_deref().ok_or_else(invalid)?)?
            != record.executable
    {
        return Err(ReaderError::Conflict);
    }
    let request = request(directory, &record.selection)?;
    let mut installer = installer(directory)?;
    let plan = installer
        .prepare(&request, &grant.policy)
        .map_err(integration_error)?;
    if plan.config_path() != record.config_path || plan.policy_path() != record.policy_path {
        return Err(ReaderError::Conflict);
    }
    let before = plan.before_bytes();
    let after = plan.after_bytes();
    let same_review =
        before == record.before.as_deref().map(str::as_bytes) && after == record.after.as_bytes();
    let completed = before == Some(record.after.as_bytes()) && after == record.after.as_bytes();
    if match current.as_str() {
        "prepared" => !same_review,
        "applying" => !same_review && !completed,
        "applied" => before != Some(after),
        _ => true,
    } {
        return Err(ReaderError::Conflict);
    }
    if current == "prepared" {
        phase(store, preparation_id, "prepared", "applying")?;
    }
    installer
        .apply(plan, &grant.policy)
        .map_err(integration_error)?;
    if current != "applied" {
        phase(store, preparation_id, "applying", "applied")?;
    }
    Ok(
        serde_json::json!({"applied":true,"preparationId":preparation_id,"configPath":record.config_path,"capabilitiesVerified":false}),
    )
}
fn config_text(record: &Preparation) -> Result<Option<String>> {
    let root = record.worktree.restore()?;
    let relative = root.relative_path(&record.config_path)?;
    let Some(file) = root.open_optional_file_relative(&relative)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_CONFIG {
        return Err(invalid());
    }
    root.verify()?;
    String::from_utf8(bytes).map(Some).map_err(|_| invalid())
}
pub fn uninstall(
    store: &mut ReaderStore,
    directory: &Path,
    preparation_id: &str,
) -> Result<serde_json::Value> {
    host()?;
    let (record, current) = load(store, preparation_id)?;
    let profile = record.profile.restore()?;
    if profile.path != directory.canonicalize()? {
        return Err(ReaderError::Conflict);
    }
    if current == "removed" {
        return Ok(serde_json::json!({"removed":true,"preparationId":preparation_id}));
    }
    if current == "prepared" {
        phase(store, preparation_id, "prepared", "removed")?;
        return Ok(
            serde_json::json!({"removed":true,"preparationId":preparation_id,"configurationChanged":false}),
        );
    }
    if !matches!(current.as_str(), "applying" | "applied" | "removing") {
        return Err(invalid());
    }
    record.worktree.restore()?.verify()?;
    if current != "removing" {
        phase(store, preparation_id, &current, "removing")?;
    }
    let request = request(directory, &record.selection)?;
    let mut installer = installer(directory)?;
    if let Err(error) = installer.uninstall(&request) {
        // Interrupted apply may have committed its DB intent before creating an installer receipt.
        // Canceling an unchanged baseline cannot remove an unowned entry.
        if !(current == "applying"
            && matches!(error, IntegrationError::Conflict)
            && config_text(&record)? == record.before)
        {
            return Err(integration_error(error));
        }
    }
    phase(store, preparation_id, "removing", "removed")?;
    Ok(
        serde_json::json!({"removed":true,"preparationId":preparation_id,"configPath":record.config_path}),
    )
}

/// Only sanitized ownership metadata is returned; saved client settings remain private.
pub fn list(store: &mut ReaderStore, project_id: &str) -> Result<serde_json::Value> {
    if project_id.trim().is_empty() || project_id.len() > 256 {
        return Err(invalid());
    }
    store.with_library_connection(|connection|->Result<_>{
        let exists:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='mdreader_service_integrations_preparations')",[],|row|row.get(0))?;
        if !exists{return Ok(serde_json::json!([]));}
        let version:u64=connection.query_row("SELECT version FROM mdreader_service_integrations_meta WHERE id=1",[],|row|row.get(0))?;
        if version!=1{return Err(invalid());}
        let mut statement=connection.prepare("SELECT record,digest,phase FROM mdreader_service_integrations_preparations ORDER BY rowid DESC LIMIT 73")?;
        let rows=statement.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        if rows.len()>72{return Err(invalid());}
        let mut entries=Vec::new();
        for (json,digest,state) in rows {
            if json.len()>MAX_RECORD||mdreader_sync::blob_digest(json.as_bytes())!=digest{return Err(invalid());}
            let record:Preparation=serde_json::from_str(&json).map_err(|_|invalid())?;
            if record.v!=1{return Err(invalid());}
            if record.selection.project_id==project_id {
                entries.push(serde_json::json!({"preparationId":record.id,"kind":record.selection.kind,"configPath":record.config_path,"state":state,"capabilitiesVerified":false}));
            }
        }
        Ok(serde_json::Value::Array(entries))
    })
}
pub(super) fn execute(
    store: &mut ReaderStore,
    directory: &Path,
    request: super::Request,
) -> Result<serde_json::Value> {
    match request {
        super::Request::PrepareIntegration { selection } => {
            serde_json::to_value(prepare(store, directory, selection)?).map_err(|_| invalid())
        }
        super::Request::ApplyIntegration { preparation_id } => {
            apply(store, directory, &preparation_id)
        }
        super::Request::UninstallIntegration { preparation_id } => {
            uninstall(store, directory, &preparation_id)
        }
        _ => Err(invalid()),
    }
}
