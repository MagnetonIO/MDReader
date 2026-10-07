#[cfg(unix)]
use crate::unixfs::{private_dir, read_optional, replace};
use crate::{
    config::{self, Owned},
    InstallRequest, IntegrationError as Error, IntegrationKind, ProjectPolicy, Result,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::{
    fs::{self, File},
    path::{Component, Path, PathBuf},
};
const CONFIG_BYTES: usize = 512 * 1024;
const POLICY_BYTES: usize = 128 * 1024;
const RECEIPT_BYTES: usize = 4 * 1024 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    Installing,
    Installed,
    Removing,
    Removed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Removal {
    before: Option<String>,
    after: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    v: u8,
    kind: IntegrationKind,
    target: PathBuf,
    worktree: PathBuf,
    worktree_identity: (u64, u64),
    policy_path: PathBuf,
    client_version: String,
    owned: Vec<Owned>,
    baseline: Option<String>,
    installed: String,
    phase: Phase,
    removal: Option<Removal>,
}
/// Reviewable bytes plus an expected baseline; fields cannot be changed by callers.
pub struct PreparedChange {
    request: InstallRequest,
    target: PathBuf,
    policy_path: PathBuf,
    policy: Vec<u8>,
    before: Option<String>,
    after: String,
    owned: Vec<Owned>,
    previous: Option<Receipt>,
    id: String,
    worktree_identity: (u64, u64),
}
impl PreparedChange {
    pub fn config_path(&self) -> &Path {
        &self.target
    }
    pub fn policy_path(&self) -> &Path {
        &self.policy_path
    }
    pub fn before_bytes(&self) -> Option<&[u8]> {
        self.before.as_deref().map(str::as_bytes)
    }
    pub fn after_bytes(&self) -> &[u8] {
        self.after.as_bytes()
    }
}
pub struct Installer {
    root: PathBuf,
    #[cfg(unix)]
    identity: (u64, u64),
}
struct Guard(File);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}
impl Installer {
    pub fn open(root: &Path) -> Result<Self> {
        if !cfg!(unix) {
            return Err(Error::Unsupported);
        }
        config::path_string(root)?;
        if fs::symlink_metadata(root).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(Error::Invalid);
        }
        private_dir(root)?;
        let root = fs::canonicalize(root)?;
        private_dir(&root.join("receipts"))?;
        #[cfg(unix)]
        let metadata = fs::metadata(&root)?;
        Ok(Self {
            root,
            #[cfg(unix)]
            identity: (metadata.dev(), metadata.ino()),
        })
    }
    fn checked(&self) -> Result<()> {
        let metadata = fs::symlink_metadata(&self.root)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(Error::Conflict);
        }
        #[cfg(unix)]
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::Conflict);
        }
        #[cfg(unix)]
        if (metadata.dev(), metadata.ino()) != self.identity {
            return Err(Error::Conflict);
        }
        {
            let path = self.root.join("receipts");
            safe_existing(&path)?;
            let metadata = fs::metadata(path)?;
            if !metadata.is_dir() {
                return Err(Error::Invalid);
            }
            #[cfg(unix)]
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(Error::Invalid);
            }
        }
        Ok(())
    }
    fn lock(&self) -> Result<Guard> {
        self.checked()?;
        let path = self.root.join("installer.lock");
        if path.exists() {
            safe_existing(&path)?;
        }
        #[cfg(unix)]
        let file = crate::unixfs::lock_file(&path)?;
        #[cfg(not(unix))]
        let file = File::open(path)?;
        file.try_lock_exclusive().map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                Error::Busy
            } else {
                Error::Io(error)
            }
        })?;
        Ok(Guard(file))
    }
    fn receipt_path(&self, id: &str) -> PathBuf {
        self.root.join("receipts").join(format!("{id}.json"))
    }
    fn receipt(&self, id: &str) -> Result<Option<Receipt>> {
        let Some(text) = read_optional(&self.receipt_path(id), RECEIPT_BYTES)? else {
            return Ok(None);
        };
        let receipt: Receipt = serde_json::from_str(&text).map_err(|_| Error::Invalid)?;
        if receipt.v != 1
            || receipt.owned.is_empty()
            || receipt.owned.len() > 2
            || !receipt.target.is_absolute()
            || id != identity(receipt.kind, &receipt.target)
        {
            return Err(Error::Invalid);
        }
        if receipt.kind.config_path(&receipt.worktree) != receipt.target
            || !receipt.policy_path.is_absolute()
        {
            return Err(Error::Invalid);
        }
        if receipt
            .baseline
            .as_ref()
            .is_some_and(|v| v.len() > CONFIG_BYTES)
            || receipt.installed.len() > CONFIG_BYTES
        {
            return Err(Error::Bounds);
        }
        Ok(Some(receipt))
    }
    fn save_receipt(&self, id: &str, receipt: &Receipt) -> Result<()> {
        self.checked()?;
        let bytes = serde_json::to_vec(receipt).map_err(|_| Error::Invalid)?;
        if bytes.len() > RECEIPT_BYTES {
            return Err(Error::Bounds);
        }
        let path = self.receipt_path(id);
        let before = read_optional(&path, RECEIPT_BYTES)?;
        let mut count = 0;
        let mut total = bytes.len() as u64;
        for entry in fs::read_dir(self.root.join("receipts"))? {
            let entry = entry?;
            if entry.path() != path {
                let metadata = entry.metadata()?;
                if !entry.file_type()?.is_file() {
                    return Err(Error::Invalid);
                }
                count += 1;
                total += metadata.len();
            }
        }
        if count >= 256 || total > 32 * 1024 * 1024 {
            return Err(Error::Bounds);
        }
        replace(
            &path,
            before.as_deref(),
            Some(std::str::from_utf8(&bytes).map_err(|_| Error::Invalid)?),
        )
    }
    pub fn prepare(
        &self,
        request: &InstallRequest,
        policy: &ProjectPolicy,
    ) -> Result<PreparedChange> {
        let _guard = self.lock()?;
        let worktree = authorize(request, policy)?;
        let worktree_identity = path_identity(&worktree)?;
        let target = request.kind.config_path(&worktree);
        safe_target(&worktree, &target)?;
        let id = identity(request.kind, &target);
        let policy_path = service_path(request, &request.policy_path, true)?;
        let policy_bytes = checked_service_policy(request, policy)?;
        let owned = config::spec(request, &policy_path, policy.capture_subagents)?;
        let before = read_optional(&target, CONFIG_BYTES)?;
        let previous = self.receipt(&id)?;
        let mut previous = previous.filter(|receipt| receipt.phase != Phase::Removed);
        let after = if let Some(receipt) = &mut previous {
            if receipt.target != target
                || receipt.kind != request.kind
                || receipt.owned != owned
                || receipt.policy_path != policy_path
                || receipt.worktree_identity != worktree_identity
            {
                return Err(Error::Conflict);
            }
            match receipt.phase {
                Phase::Installed => {
                    config::verify(before.as_deref().ok_or(Error::Conflict)?, &owned)?;
                    before.clone().ok_or(Error::Conflict)?
                }
                Phase::Installing => {
                    if before.as_deref() != receipt.baseline.as_deref()
                        && before.as_deref() != Some(receipt.installed.as_str())
                    {
                        return Err(Error::Conflict);
                    }
                    receipt.installed.clone()
                }
                _ => return Err(Error::NotReady),
            }
        } else {
            let empty = match request.kind {
                IntegrationKind::ClaudeHooks
                | IntegrationKind::ClaudeMcp
                | IntegrationKind::CodexHooks => "{}",
                _ => "",
            };
            config::install(before.clone().unwrap_or_else(|| empty.into()), &owned)?
        };
        if after.len() > CONFIG_BYTES {
            return Err(Error::Bounds);
        }
        Ok(PreparedChange {
            request: request.clone(),
            target,
            policy_path,
            policy: policy_bytes,
            before,
            after,
            owned,
            previous,
            id,
            worktree_identity,
        })
    }
    /// Requires a matching durable service policy. No policy or client trust is modified.
    pub fn apply(
        &mut self,
        change: PreparedChange,
        current_policy: &ProjectPolicy,
    ) -> Result<PathBuf> {
        self.apply_observed(change, current_policy, |_| {})
    }
    fn apply_observed(
        &mut self,
        change: PreparedChange,
        current_policy: &ProjectPolicy,
        mut observe: impl FnMut(Phase),
    ) -> Result<PathBuf> {
        let _guard = self.lock()?;
        let worktree = authorize(&change.request, current_policy)?;
        if path_identity(&worktree)? != change.worktree_identity {
            return Err(Error::Conflict);
        }
        if checked_service_policy(&change.request, current_policy)? != change.policy
            || change.request.kind.config_path(&worktree) != change.target
        {
            return Err(Error::Conflict);
        }
        safe_target(&worktree, &change.target)?;
        if read_optional(&change.target, CONFIG_BYTES)? != change.before {
            return Err(Error::Conflict);
        }
        if let Some(old) = &change.previous {
            if self
                .receipt(&change.id)?
                .as_ref()
                .map(|r| (&r.installed, r.phase))
                != Some((&old.installed, old.phase))
            {
                return Err(Error::Conflict);
            }
        } else if self
            .receipt(&change.id)?
            .is_some_and(|r| r.phase != Phase::Removed)
        {
            return Err(Error::Conflict);
        }
        let mut receipt = change.previous.unwrap_or(Receipt {
            v: 1,
            kind: change.request.kind,
            target: change.target.clone(),
            worktree: worktree.clone(),
            worktree_identity: change.worktree_identity,
            policy_path: change.policy_path.clone(),
            client_version: change.request.capabilities.client_version.clone(),
            owned: change.owned,
            baseline: change.before.clone(),
            installed: change.after.clone(),
            phase: Phase::Installing,
            removal: None,
        });
        // Check receipt capacity before changing configuration.
        if serde_json::to_vec(&receipt)
            .map_err(|_| Error::Invalid)?
            .len()
            > RECEIPT_BYTES
        {
            return Err(Error::Bounds);
        }
        if receipt.phase == Phase::Installed {
            return Ok(change.target);
        }
        self.save_receipt(&change.id, &receipt)?;
        observe(Phase::Installing);
        create_config_parent(&worktree, &change.target)?;
        replace_config(
            &worktree,
            change.worktree_identity,
            &change.target,
            change.before.as_deref(),
            Some(&change.after),
        )?;
        observe(Phase::Installed);
        receipt.phase = Phase::Installed;
        self.save_receipt(&change.id, &receipt)?;
        Ok(change.target)
    }
    /// Rollback and uninstall use the same ownership checks; they never restore a stale snapshot over user edits.
    pub fn rollback(&mut self, request: &InstallRequest) -> Result<()> {
        self.uninstall(request)
    }
    pub fn uninstall(&mut self, request: &InstallRequest) -> Result<()> {
        self.uninstall_observed(request, |_| {})
    }
    fn uninstall_observed(
        &mut self,
        request: &InstallRequest,
        mut observe: impl FnMut(Phase),
    ) -> Result<()> {
        let _guard = self.lock()?;
        let worktree = canonical_root(&request.worktree_root)?;
        let target = request.kind.config_path(&worktree);
        safe_target(&worktree, &target)?;
        let id = identity(request.kind, &target);
        let Some(mut receipt) = self.receipt(&id)? else {
            return Err(Error::Conflict);
        };
        if receipt.phase == Phase::Removed {
            return Ok(());
        }
        if path_identity(&worktree)? != receipt.worktree_identity {
            return Err(Error::Conflict);
        }
        let current = read_optional(&target, CONFIG_BYTES)?;
        let removal = if receipt.phase == Phase::Removing {
            receipt.removal.clone().ok_or(Error::Invalid)?
        } else {
            let after = if current.as_deref() == Some(receipt.installed.as_str()) {
                receipt.baseline.clone()
            } else if receipt.phase == Phase::Installing && current == receipt.baseline {
                current.clone()
            } else {
                Some(config::remove(
                    current.clone().ok_or(Error::Conflict)?,
                    &receipt.owned,
                )?)
            };
            let removal = Removal {
                before: current.clone(),
                after,
            };
            receipt.phase = Phase::Removing;
            receipt.removal = Some(removal.clone());
            self.save_receipt(&id, &receipt)?;
            observe(Phase::Removing);
            removal
        };
        if current != removal.after {
            if current != removal.before {
                return Err(Error::Conflict);
            }
            replace_config(
                &worktree,
                receipt.worktree_identity,
                &target,
                current.as_deref(),
                removal.after.as_deref(),
            )?;
        }
        observe(Phase::Removed);
        receipt.phase = Phase::Removed;
        self.save_receipt(&id, &receipt)?;
        Ok(())
    }
}
fn policy_bytes(policy: &ProjectPolicy) -> Result<Vec<u8>> {
    policy.validate().map_err(|_| Error::Invalid)?;
    let bytes = serde_json::to_vec(policy).map_err(|_| Error::Invalid)?;
    if bytes.len() > POLICY_BYTES {
        return Err(Error::Bounds);
    }
    Ok(bytes)
}
fn authorize(request: &InstallRequest, policy: &ProjectPolicy) -> Result<PathBuf> {
    if !cfg!(unix) {
        return Err(Error::Unsupported);
    }
    policy.validate().map_err(|_| Error::Invalid)?;
    if !policy.enabled
        && matches!(
            request.kind,
            IntegrationKind::ClaudeHooks
                | IntegrationKind::CodexHooks
                | IntegrationKind::CodexNotify
        )
    {
        return Err(Error::OptOut);
    }
    config::validate_capability(request.kind, &request.capabilities)?;
    let worktree = canonical_root(&request.worktree_root)?;
    let admitted = policy
        .worktree_roots
        .iter()
        .map(|root| canonical_root(root))
        .collect::<Result<Vec<_>>>()?;
    if !admitted.contains(&worktree) {
        return Err(Error::OptOut);
    }
    canonical_root(&policy.project_root)?;
    canonical_root(&policy.repository_root)?;
    config::path_string(&request.executable)?;
    canonical_root(&request.data_dir)?;
    service_path(request, &request.policy_path, true)?;
    service_path(request, &request.spool_root, false)?;
    Ok(worktree)
}
fn checked_service_policy(request: &InstallRequest, current: &ProjectPolicy) -> Result<Vec<u8>> {
    let path = service_path(request, &request.policy_path, true)?;
    let text = read_optional(&path, POLICY_BYTES)?.ok_or(Error::NotReady)?;
    let artifact: ProjectPolicy = serde_json::from_str(&text).map_err(|_| Error::Invalid)?;
    let expected = policy_bytes(current)?;
    if policy_bytes(&artifact)? != expected {
        return Err(Error::Conflict);
    }
    Ok(expected)
}
fn service_path(request: &InstallRequest, path: &Path, file: bool) -> Result<PathBuf> {
    config::path_string(path)?;
    let root = canonical_root(&request.data_dir)?;
    let relative = path
        .strip_prefix(&request.data_dir)
        .or_else(|_| path.strip_prefix(&root))
        .map_err(|_| Error::Invalid)?;
    if relative.as_os_str().is_empty() {
        return Err(Error::Invalid);
    }
    let target = root.join(relative);
    let mut candidate = root;
    for component in relative.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(Error::Invalid);
        }
        candidate.push(component);
        let metadata = fs::symlink_metadata(&candidate)?;
        if metadata.file_type().is_symlink()
            || if candidate == target && file {
                !metadata.is_file()
            } else {
                !metadata.is_dir()
            }
        {
            return Err(Error::Invalid);
        }
    }
    Ok(candidate)
}
fn identity(kind: IntegrationKind, path: &Path) -> String {
    blake3::hash(format!("{kind:?}\0{}", path.display()).as_bytes())
        .to_hex()
        .to_string()
}
fn canonical_root(path: &Path) -> Result<PathBuf> {
    config::path_string(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Error::Invalid);
    }
    let canonical = fs::canonicalize(path)?;
    #[cfg(unix)]
    if path_identity(&canonical)? != (metadata.dev(), metadata.ino()) {
        return Err(Error::Conflict);
    }
    Ok(canonical)
}
#[cfg(unix)]
fn path_identity(path: &Path) -> Result<(u64, u64)> {
    let metadata = crate::unixfs::directory(path)?.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}
#[cfg(not(unix))]
fn path_identity(_: &Path) -> Result<(u64, u64)> {
    Err(Error::Unsupported)
}
#[cfg(unix)]
fn replace_config(
    root: &Path,
    identity: (u64, u64),
    path: &Path,
    expected: Option<&str>,
    replacement: Option<&str>,
) -> Result<()> {
    crate::unixfs::replace_under(root, identity, path, expected, replacement)
}
#[cfg(not(unix))]
fn replace_config(
    _: &Path,
    _: (u64, u64),
    _: &Path,
    _: Option<&str>,
    _: Option<&str>,
) -> Result<()> {
    Err(Error::Unsupported)
}
fn safe_existing(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn safe_target(root: &Path, target: &Path) -> Result<()> {
    let relative = target.strip_prefix(root).map_err(|_| Error::Invalid)?;
    let mut path = root.to_path_buf();
    for component in relative.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(Error::Invalid);
        }
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink()
                    || (!metadata.is_dir() && path != target)
                    || (!metadata.is_file() && path == target)
                {
                    return Err(Error::Invalid);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
fn create_config_parent(root: &Path, target: &Path) -> Result<()> {
    safe_target(root, target)?;
    let parent = target.parent().ok_or(Error::Invalid)?;
    if !parent.exists() {
        private_dir(parent)?;
    }
    safe_target(root, target)
}
#[cfg(not(unix))]
fn private_dir(_: &Path) -> Result<()> {
    Err(Error::Unsupported)
}
#[cfg(not(unix))]
fn read_optional(_: &Path, _: usize) -> Result<Option<String>> {
    Err(Error::Unsupported)
}
#[cfg(not(unix))]
fn replace(_: &Path, _: Option<&str>, _: Option<&str>) -> Result<()> {
    Err(Error::Unsupported)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::Capabilities;
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
        time::Duration,
    };
    fn fixture(root: &Path) -> (Installer, InstallRequest, ProjectPolicy) {
        let worktree = root.join("project");
        let data = root.join("data");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(data.join("captures/policies")).unwrap();
        let policy = ProjectPolicy {
            enabled: true,
            project_id: "synthetic".into(),
            project_root: worktree.clone(),
            repository_root: worktree.clone(),
            worktree_roots: vec![worktree.clone()],
            capture_subagents: true,
            excluded_paths: vec![],
            excluded_sessions: vec![],
            selected_backfill_sessions: vec![],
        };
        let policy_path = data.join("captures/policies/policy.json");
        fs::write(&policy_path, serde_json::to_vec_pretty(&policy).unwrap()).unwrap();
        let request = InstallRequest {
            kind: IntegrationKind::ClaudeHooks,
            worktree_root: worktree,
            executable: "/Applications/MDReader.app/Contents/MacOS/mdreader-app".into(),
            spool_root: data.join("captures"),
            policy_path,
            data_dir: data,
            source_ids: vec!["selected-source".into()],
            capabilities: Capabilities {
                local_commands: true,
                claude_exec_hooks: true,
                client_version: "synthetic".into(),
                ..Capabilities::default()
            },
        };
        (
            Installer::open(&root.join("state")).unwrap(),
            request,
            policy,
        )
    }
    fn pause(phase: Phase, wanted: &str) {
        if format!("{phase:?}") == wanted {
            println!("ready");
            std::io::stdout().flush().unwrap();
            loop {
                std::thread::park();
            }
        }
    }
    #[test]
    fn crash_worker() {
        let Ok(root) = std::env::var("MDREADER_SYNTHETIC_INSTALL_CRASH") else {
            return;
        };
        let wanted = std::env::var("MDREADER_SYNTHETIC_INSTALL_PHASE").unwrap();
        let (mut installer, request, policy) = fixture(Path::new(&root));
        let plan = installer.prepare(&request, &policy).unwrap();
        if wanted == "Installing" || wanted == "Installed" {
            installer
                .apply_observed(plan, &policy, |phase| pause(phase, &wanted))
                .unwrap();
        } else {
            installer.apply(plan, &policy).unwrap();
            installer
                .uninstall_observed(&request, |phase| pause(phase, &wanted))
                .unwrap();
        }
        panic!("crash phase not reached");
    }
    fn killed(root: &Path, phase: &str) {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["durable::tests::crash_worker", "--exact", "--nocapture"])
            .env("MDREADER_SYNTHETIC_INSTALL_CRASH", root)
            .env("MDREADER_SYNTHETIC_INSTALL_PHASE", phase)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if line.unwrap() == "ready" {
                    let _ = sender.send(());
                    break;
                }
            }
        });
        let ready = receiver.recv_timeout(Duration::from_secs(10));
        let _ = child.kill();
        let status = child.wait().unwrap();
        assert!(
            ready.is_ok(),
            "worker did not reach durable {phase} checkpoint: {status}"
        );
        assert!(!status.success());
    }
    #[test]
    fn actual_process_kill_before_and_after_config_publication_recovers_install_and_removal() {
        for phase in ["Installing", "Installed", "Removing", "Removed"] {
            let temp = tempfile::TempDir::new().unwrap();
            let (_, request, _) = fixture(temp.path());
            let target = request.kind.config_path(&request.worktree_root);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            let original = "{\"user\":\"preserve exact baseline\"}\n";
            fs::write(&target, original).unwrap();
            killed(temp.path(), phase);
            let (mut installer, request, policy) = fixture(temp.path());
            if phase == "Installing" || phase == "Installed" {
                let plan = installer.prepare(&request, &policy).unwrap();
                installer.apply(plan, &policy).unwrap();
                let value: serde_json::Value =
                    serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
                assert_eq!(value["hooks"]["Stop"].as_array().unwrap().len(), 1);
                installer.rollback(&request).unwrap();
            } else {
                installer.uninstall(&request).unwrap();
            }
            assert_eq!(fs::read_to_string(&target).unwrap(), original);
        }
    }
    #[test]
    fn edit_after_interrupted_publication_is_preserved_and_requires_review() {
        let temp = tempfile::TempDir::new().unwrap();
        fixture(temp.path());
        killed(temp.path(), "Installed");
        let (mut installer, request, policy) = fixture(temp.path());
        let target = request.kind.config_path(&request.worktree_root);
        let edited =
            fs::read_to_string(&target)
                .unwrap()
                .replacen("{", "{\"userAfterCrash\":true,", 1);
        fs::write(&target, &edited).unwrap();
        assert!(matches!(
            installer.prepare(&request, &policy),
            Err(Error::Conflict)
        ));
        // Explicit rollback can remove precisely the recorded entries and preserve that edit.
        installer.rollback(&request).unwrap();
        let remaining = fs::read_to_string(&target).unwrap();
        assert!(remaining.contains("\"userAfterCrash\":true"));
        assert!(!remaining.contains("--mdreader-capture"));
    }
}
