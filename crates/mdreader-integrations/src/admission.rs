//! Selected-root/policy admission. This module never writes an install receipt.
use crate::durable::{POLICY_BYTES, RootIdentity};
#[cfg(unix)]
use crate::unixfs::private_dir;
#[cfg(unix)]
use crate::unixfs::read_optional;
#[cfg(windows)]
use crate::windows::read_optional;
#[cfg(windows)]
pub(super) use crate::windows::{create_config_parent, path_identity, replace_config, safe_target};
use crate::{
    InstallRequest, IntegrationError as Error, IntegrationKind, ProjectPolicy, Result, config,
};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    fs,
    path::{Component, Path, PathBuf},
};
fn policy_bytes(policy: &ProjectPolicy) -> Result<Vec<u8>> {
    policy.validate().map_err(|_| Error::Invalid)?;
    let bytes = serde_json::to_vec(policy).map_err(|_| Error::Invalid)?;
    if bytes.len() > POLICY_BYTES {
        return Err(Error::Bounds);
    }
    Ok(bytes)
}
pub(super) fn authorize(request: &InstallRequest, policy: &ProjectPolicy) -> Result<PathBuf> {
    if !cfg!(any(unix, windows)) {
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
pub(super) fn checked_service_policy(
    request: &InstallRequest,
    current: &ProjectPolicy,
) -> Result<Vec<u8>> {
    let path = service_path(request, &request.policy_path, true)?;
    let text = read_optional(&path, POLICY_BYTES)?.ok_or(Error::NotReady)?;
    let artifact: ProjectPolicy = serde_json::from_str(&text).map_err(|_| Error::Invalid)?;
    let expected = policy_bytes(current)?;
    if policy_bytes(&artifact)? != expected {
        return Err(Error::Conflict);
    }
    Ok(expected)
}
pub(super) fn service_path(request: &InstallRequest, path: &Path, file: bool) -> Result<PathBuf> {
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
    #[cfg(windows)]
    {
        if file {
            read_optional(&candidate, POLICY_BYTES)?.ok_or(Error::NotReady)?;
        } else {
            crate::windows::directory(&candidate)?
                .verify()
                .map_err(crate::windows::capture)?;
        }
    }
    Ok(candidate)
}
pub(super) fn identity(kind: IntegrationKind, path: &Path) -> String {
    blake3::hash(format!("{kind:?}\0{}", path.display()).as_bytes())
        .to_hex()
        .to_string()
}
#[cfg(not(windows))]
pub(super) fn canonical_root(path: &Path) -> Result<PathBuf> {
    config::path_string(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Error::Invalid);
    }
    let canonical = fs::canonicalize(path)?;
    #[cfg(unix)]
    if path_identity(&canonical)? != RootIdentity::Unix((metadata.dev(), metadata.ino())) {
        return Err(Error::Conflict);
    }
    Ok(canonical)
}
#[cfg(windows)]
pub(super) fn canonical_root(path: &Path) -> Result<PathBuf> {
    config::path_string(path)?;
    crate::windows::directory(path)?
        .verify()
        .map_err(crate::windows::capture)?;
    Ok(path.to_owned())
}
#[cfg(unix)]
pub(super) fn path_identity(path: &Path) -> Result<RootIdentity> {
    let metadata = crate::unixfs::directory(path)?.metadata()?;
    Ok(RootIdentity::Unix((metadata.dev(), metadata.ino())))
}
#[cfg(not(any(unix, windows)))]
pub(super) fn path_identity(_: &Path) -> Result<RootIdentity> {
    Err(Error::Unsupported)
}
#[cfg(unix)]
pub(super) fn replace_config(
    root: &Path,
    identity: RootIdentity,
    path: &Path,
    expected: Option<&str>,
    replacement: Option<&str>,
) -> Result<()> {
    let RootIdentity::Unix(identity) = identity;
    crate::unixfs::replace_under(root, identity, path, expected, replacement)
}
#[cfg(not(any(unix, windows)))]
pub(super) fn replace_config(
    _: &Path,
    _: RootIdentity,
    _: &Path,
    _: Option<&str>,
    _: Option<&str>,
) -> Result<()> {
    Err(Error::Unsupported)
}
#[cfg(not(windows))]
pub(super) fn safe_existing(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
        return Err(Error::Invalid);
    }
    Ok(())
}
#[cfg(not(windows))]
pub(super) fn safe_target(root: &Path, target: &Path) -> Result<()> {
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
#[cfg(not(windows))]
pub(super) fn create_config_parent(root: &Path, target: &Path) -> Result<()> {
    safe_target(root, target)?;
    let parent = target.parent().ok_or(Error::Invalid)?;
    if !parent.exists() {
        private_dir(parent)?;
    }
    safe_target(root, target)
}
#[cfg(not(any(unix, windows)))]
fn private_dir(_: &Path) -> Result<()> {
    Err(Error::Unsupported)
}
#[cfg(not(any(unix, windows)))]
fn read_optional(_: &Path, _: usize) -> Result<Option<String>> {
    Err(Error::Unsupported)
}
