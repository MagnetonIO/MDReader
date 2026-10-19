//! Receipt-backed launcher updates keep the existing library/grant binding.
use super::*;

fn same_binding(old: &[Owned], new: &[Owned]) -> Result<bool> {
    if old.len() != new.len() || old.is_empty() {
        return Ok(false);
    }
    if matches!(old[0], Owned::Json { array: true, .. }) {
        for (a, b) in old.iter().zip(new) {
            let (
                Owned::Json {
                    path: ap,
                    array: true,
                    raw: a,
                },
                Owned::Json {
                    path: bp,
                    array: true,
                    raw: b,
                },
            ) = (a, b)
            else {
                return Ok(false);
            };
            if ap != bp {
                return Ok(false);
            }
            let mut a: serde_json::Value = serde_json::from_str(a).map_err(|_| Error::Invalid)?;
            let mut b: serde_json::Value = serde_json::from_str(b).map_err(|_| Error::Invalid)?;
            for value in [&mut a, &mut b] {
                let hooks = value
                    .get_mut("hooks")
                    .and_then(serde_json::Value::as_array_mut)
                    .ok_or(Error::Invalid)?;
                if hooks.len() != 1 {
                    return Ok(false);
                }
                let command = hooks[0].get_mut("command").ok_or(Error::Invalid)?;
                *command = serde_json::Value::String(
                    command_tail(command.as_str().ok_or(Error::Invalid)?)?.into(),
                );
            }
            // A legacy Claude matcher is the same binding with fewer tools.
            if a.get("matcher")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|matcher| {
                    crate::session_hooks::LEGACY_CLAUDE_FILE_TOOLS.contains(&matcher)
                })
                && b.get("matcher").and_then(serde_json::Value::as_str)
                    == Some(crate::session_hooks::CLAUDE_FILE_TOOLS)
            {
                a["matcher"] = b["matcher"].clone();
            }
            if a != b {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    if old.len() != 1 {
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

/// Remove only the generated quoted executable argument. Everything after it,
/// including the profile, grant and hook mode, must remain byte-identical.
fn command_tail(command: &str) -> Result<&str> {
    let mut quote = None;
    let mut escape = false;
    for (i, c) in command.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escape = true;
            continue;
        }
        if matches!(c, '\'' | '"') {
            if quote == Some(c) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(c);
            }
        } else if c == ' ' && quote.is_none() {
            let tail = &command[i..];
            if tail.starts_with(" '--mdreader-agent' 'session-hook' ")
                || tail.starts_with(" \"--mdreader-agent\" \"session-hook\" ")
            {
                return Ok(tail);
            }
            return Err(Error::Conflict);
        }
    }
    Err(Error::Conflict)
}

impl Installer {
    /// Readiness is proven by a completed receipt and current owned entries;
    /// a valid install preflight is not evidence that publication happened.
    pub fn verify_installed(
        &self,
        request: &InstallRequest,
        policy: &ProjectPolicy,
    ) -> Result<bool> {
        let _guard = self.lock()?;
        let root = authorize(request, policy)?;
        let target = request.kind.config_path(&root);
        safe_target(&root, &target)?;
        let Some(receipt) = self.receipt(&identity(request.kind, &target))? else {
            return Ok(false);
        };
        if receipt.phase != Phase::Installed {
            return Ok(false);
        }
        let policy_path = service_path(request, &request.policy_path, true)?;
        let owned = config::spec(request, &policy_path, policy.capture_subagents)?;
        if receipt.target != target
            || receipt.kind != request.kind
            || receipt.owned != owned
            || receipt.policy_path != policy_path
            || receipt.worktree_identity != path_identity(&root)?
        {
            return Err(Error::Conflict);
        }
        checked_service_policy(request, policy)?;
        let Some(current) = read_optional(&target, config_bytes(request.kind))? else {
            return Ok(false);
        };
        config::verify(&current, &owned)?;
        Ok(true)
    }
    /// Retry/update an owned MCP launcher, preserving unrelated settings. The
    /// existing removal/install journals also recover a stop between the steps.
    pub fn refresh_launcher(
        &mut self,
        request: &InstallRequest,
        policy: &ProjectPolicy,
    ) -> Result<PathBuf> {
        if !matches!(
            request.kind,
            IntegrationKind::ClaudeMcp
                | IntegrationKind::ClaudeUserMcp
                | IntegrationKind::CodexMcp
                | IntegrationKind::ClaudeSessionHooks
                | IntegrationKind::ClaudeUserSessionHooks
                | IntegrationKind::CodexSessionHooks
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
