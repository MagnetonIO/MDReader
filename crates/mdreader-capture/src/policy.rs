use crate::*;

/// Explicit policy supplied by the owner of this library, never discovered in histories.
/// Roots are exact admitted worktrees. Exclusions are relative directory prefixes in
/// every worktree; there is no implicit admission of sibling or new worktrees.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectPolicy {
    #[serde(default)]
    pub enabled: bool,
    pub project_id: String,
    pub project_root: PathBuf,
    pub repository_root: PathBuf,
    pub worktree_roots: Vec<PathBuf>,
    #[serde(default)]
    pub capture_subagents: bool,
    #[serde(default)]
    pub excluded_paths: Vec<PathBuf>,
    #[serde(default)]
    pub excluded_sessions: Vec<String>,
    #[serde(default)]
    pub selected_backfill_sessions: Vec<String>,
}
impl ProjectPolicy {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = read_bounded(path, POLICY_BYTES)?;
        let policy: Self = serde_json::from_slice(&bytes).map_err(|_| CaptureError::Invalid)?;
        policy.validate()?;
        Ok(policy)
    }
    pub fn validate(&self) -> Result<()> {
        if self.project_id.trim().is_empty()
            || self.project_id.len() > 256
            || !self.project_root.is_absolute()
            || !self.repository_root.is_absolute()
            || self.worktree_roots.is_empty()
            || self.worktree_roots.len() > 128
            || self.excluded_paths.len() > 1024
            || self.excluded_sessions.len() > 4096
            || self.selected_backfill_sessions.len() > 4096
            || self.worktree_roots.iter().any(|p| !p.is_absolute())
            || self.excluded_paths.iter().any(|p| {
                p.as_os_str().is_empty()
                    || p.components().any(|c| !matches!(c, Component::Normal(_)))
            })
        {
            return Err(CaptureError::Invalid);
        }
        Ok(())
    }
    pub(crate) fn authorize(
        &self,
        cwd: &Path,
        session: &str,
        subagent: Option<&str>,
        origin: CaptureOrigin,
    ) -> Result<AuthorizedRoots> {
        self.validate()?;
        if !self.enabled {
            return Err(CaptureError::Excluded(SkipReason::ProjectDisabled));
        }
        if self.excluded_sessions.iter().any(|s| s == session) {
            return Err(CaptureError::Excluded(SkipReason::SessionExcluded));
        }
        if origin == CaptureOrigin::SelectedBackfill
            && !self.selected_backfill_sessions.iter().any(|s| s == session)
        {
            return Err(CaptureError::Excluded(SkipReason::BackfillNotSelected));
        }
        if subagent.is_some() && !self.capture_subagents {
            return Err(CaptureError::Excluded(SkipReason::SubagentExcluded));
        }
        if !cwd.is_absolute() {
            return Err(CaptureError::Invalid);
        }
        let cwd = fs::canonicalize(cwd)?;
        let mut admitted = Vec::new();
        for configured in &self.worktree_roots {
            let root = fs::canonicalize(configured)?;
            // A root itself cannot be a symlink. System ancestor aliases (such as
            // macOS /var) are normalized into the captured authority.
            if fs::symlink_metadata(configured)?.file_type().is_symlink() {
                return Err(CaptureError::UnsafePath);
            }
            if cwd.starts_with(&root) {
                admitted.push(root);
            }
        }
        let worktree = admitted
            .into_iter()
            .max_by_key(|p| p.components().count())
            .ok_or(CaptureError::Excluded(SkipReason::OutsideProject))?;
        if self
            .excluded_paths
            .iter()
            .any(|p| cwd.starts_with(worktree.join(p)))
        {
            return Err(CaptureError::Excluded(SkipReason::PathExcluded));
        }
        let project = fs::canonicalize(&self.project_root)?;
        let repository = fs::canonicalize(&self.repository_root)?;
        if fs::symlink_metadata(&self.project_root)?
            .file_type()
            .is_symlink()
            || fs::symlink_metadata(&self.repository_root)?
                .file_type()
                .is_symlink()
        {
            return Err(CaptureError::UnsafePath);
        }
        Ok(AuthorizedRoots {
            cwd,
            project,
            repository,
            worktree,
        })
    }
}
pub(crate) struct AuthorizedRoots {
    pub(crate) cwd: PathBuf,
    pub(crate) project: PathBuf,
    pub(crate) repository: PathBuf,
    pub(crate) worktree: PathBuf,
}
