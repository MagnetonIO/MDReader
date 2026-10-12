//! Receipt-backed launcher updates keep the existing library/grant binding.
use super::*;

fn same_binding(old: &[Owned], new: &[Owned]) -> Result<bool> {
    if old.len() != 1 || new.len() != 1 {
        return Ok(false);
    }
    match (&old[0], &new[0]) {
        (
            Owned::Json {
                path: a,
                array: false,
                raw: old,
            },
            Owned::Json {
                path: b,
                array: false,
                raw: new,
            },
        ) if a == b => {
            let mut old: serde_json::Value =
                serde_json::from_str(old).map_err(|_| Error::Invalid)?;
            let mut new: serde_json::Value =
                serde_json::from_str(new).map_err(|_| Error::Invalid)?;
            for value in [&mut old, &mut new] {
                value
                    .as_object_mut()
                    .ok_or(Error::Invalid)?
                    .remove("command")
                    .ok_or(Error::Invalid)?;
            }
            Ok(old == new)
        }
        (
            Owned::Toml {
                path: a,
                fragment: old,
            },
            Owned::Toml {
                path: b,
                fragment: new,
            },
        ) if a == b => {
            let strip = |text: &str| -> Result<String> {
                let mut doc: toml_edit::DocumentMut = text.parse().map_err(|_| Error::Invalid)?;
                let mut table: &mut dyn toml_edit::TableLike = doc.as_table_mut();
                for key in a {
                    table = table
                        .get_mut(key)
                        .and_then(toml_edit::Item::as_table_like_mut)
                        .ok_or(Error::Invalid)?;
                }
                table.remove("command").ok_or(Error::Invalid)?;
                Ok(doc.to_string())
            };
            Ok(strip(old)? == strip(new)?)
        }
        _ => Ok(false),
    }
}

impl Installer {
    /// Retry/update an owned MCP launcher, preserving unrelated settings. The
    /// existing removal/install journals also recover a stop between the steps.
    pub fn refresh_launcher(
        &mut self,
        request: &InstallRequest,
        policy: &ProjectPolicy,
    ) -> Result<PathBuf> {
        if !matches!(
            request.kind,
            IntegrationKind::ClaudeMcp | IntegrationKind::ClaudeUserMcp | IntegrationKind::CodexMcp
        ) {
            return Err(Error::Conflict);
        }
        {
            let _guard = self.lock()?;
            let root = authorize(request, policy)?;
            let target = request.kind.config_path(&root);
            safe_target(&root, &target)?;
            let id = identity(request.kind, &target);
            let old = self.receipt(&id)?.ok_or(Error::Conflict)?;
            let policy_path = service_path(request, &request.policy_path, true)?;
            let owned = config::spec(request, &policy_path, policy.capture_subagents)?;
            if old.phase != Phase::Installed
                || old.kind != request.kind
                || old.target != target
                || old.worktree_identity != path_identity(&root)?
                || old.policy_path != policy_path
                || !same_binding(&old.owned, &owned)?
            {
                return Err(Error::Conflict);
            }
            checked_service_policy(request, policy)?;
            let before =
                read_optional(&target, config_bytes(request.kind))?.ok_or(Error::Conflict)?;
            config::verify(&before, &old.owned)?;
            let baseline = config::remove(before, &old.owned)?;
            let installed = config::install(baseline.clone(), &owned)?;
            if installed.len() > config_bytes(request.kind) {
                return Err(Error::Bounds);
            }
            let candidate = Receipt {
                baseline: Some(baseline),
                installed,
                owned,
                phase: Phase::Installing,
                removal: None,
                ..old
            };
            if serde_json::to_vec(&candidate)
                .map_err(|_| Error::Invalid)?
                .len()
                > RECEIPT_BYTES
            {
                return Err(Error::Bounds);
            }
        }
        self.uninstall(request)?;
        let change = self.prepare(request, policy)?;
        self.apply(change, policy)
    }
}
