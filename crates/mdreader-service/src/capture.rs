//! Service-owned admission bridge. Hooks never open library.sqlite.
pub mod admission;
pub mod backfill;
pub mod reader;
use mdreader_capture::{Limits, ProjectPolicy, Spool};
use mdreader_core::{ReaderStore, Result, SourceRoot, SourceRootIdentity};
use serde::{Deserialize, Serialize};
use std::io::Read;
#[cfg(not(windows))]
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RootGrant {
    path: PathBuf,
    identity: SourceRootIdentity,
}
impl RootGrant {
    fn capture(path: &Path) -> Result<Self> {
        let root = SourceRoot::capture(path.to_path_buf())?;
        Ok(Self {
            path: root.path,
            identity: root.identity,
        })
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
struct Grant {
    policy: ProjectPolicy,
    roots: Vec<RootGrant>,
    source_id: Option<String>,
    projection_root: Option<RootGrant>,
}
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct DrainStatus {
    pub admitted: usize,
    pub rejected: usize,
    pub diagnostic: Option<String>,
}
fn invalid() -> mdreader_core::ReaderError {
    super::protocol_error("Capture configuration or source is unavailable")
}
fn policies(store: &ReaderStore) -> Result<Vec<Grant>> {
    store
        .capture_policies()?
        .into_iter()
        .map(|(_, json)| serde_json::from_str(&json).map_err(|_| invalid()))
        .collect()
}
fn artifact_name(project: &str) -> PathBuf {
    PathBuf::from(format!(
        "{}.json",
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, project.as_bytes())
    ))
}
pub fn policy_path(directory: &Path, project: &str) -> PathBuf {
    directory
        .join("captures")
        .join("policies")
        .join(artifact_name(project))
}
fn policy_directory(directory: &Path) -> Result<SourceRoot> {
    let root = SourceRoot::capture(directory.canonicalize()?)?;
    let captures = root.create_or_capture_directory_relative(Path::new("captures"))?;
    Ok(captures.create_or_capture_directory_relative(Path::new("policies"))?)
}
#[cfg(not(windows))]
fn write_policy(directory: &Path, policy: &ProjectPolicy) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(policy).map_err(|_| invalid())?;
    if bytes.len() > 128 * 1024 {
        return Err(invalid());
    }
    let root = policy_directory(directory)?;
    let temporary = PathBuf::from(format!(".policy-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = root.create_new_relative(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        root.rename_relative(&temporary, &artifact_name(&policy.project_id))?;
        root.try_clone_directory_handle()?.sync_all()?;
        root.verify()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = root.remove_file_relative(&temporary);
    }
    result
}
#[cfg(windows)]
fn write_policy(directory: &Path, policy: &ProjectPolicy) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(policy).map_err(|_| invalid())?;
    if bytes.len() > 128 * 1024 {
        return Err(invalid());
    }
    let captures = directory.join("captures");
    let _private_capture =
        mdreader_capture::PrivateDirectory::create(&captures).map_err(|_| invalid())?;
    mdreader_capture::PrivateDirectory::create(&captures.join("policies"))
        .and_then(|root| root.publish(&artifact_name(&policy.project_id), &bytes))
        .map_err(|_| invalid())
}
fn artifact_matches(directory: &Path, policy: &ProjectPolicy) -> Result<()> {
    let root = policy_directory(directory)?;
    let file = root
        .open_optional_file_relative(&artifact_name(&policy.project_id))?
        .ok_or_else(invalid)?;
    let mut bytes = Vec::new();
    file.take(128 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 128 * 1024 {
        return Err(invalid());
    }
    let actual: ProjectPolicy = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if serde_json::to_value(actual).map_err(|_| invalid())?
        != serde_json::to_value(policy).map_err(|_| invalid())?
    {
        return Err(invalid());
    }
    root.verify()?;
    Ok(())
}

/// Explicit user opt-in. Identity snapshots, rather than path strings alone,
/// remain authoritative across service restart and worktree replacement.
pub fn configure(store: &mut ReaderStore, directory: &Path, policy: ProjectPolicy) -> Result<()> {
    policy.validate().map_err(|_| invalid())?;
    let previous = policies(store)?
        .into_iter()
        .find(|grant| grant.policy.project_id == policy.project_id);
    // Settings callers carry the list they previously observed. Once a grant
    // exists, only the explicit backfill operation changes this list: a stale
    // settings save must not undo a later session deselection (or selection).
    // Reject before recapturing roots, opening the spool or publishing policy.
    if previous.as_ref().is_some_and(|grant| {
        grant.policy.selected_backfill_sessions != policy.selected_backfill_sessions
    }) {
        return Err(mdreader_core::ReaderError::Conflict);
    }
    let grant = if !policy.enabled {
        if let Some(mut previous) = previous {
            previous.policy = policy;
            previous
        } else {
            Grant {
                policy,
                roots: vec![],
                source_id: None,
                projection_root: None,
            }
        }
    } else {
        let mut paths = vec![policy.project_root.clone(), policy.repository_root.clone()];
        paths.extend(policy.worktree_roots.clone());
        paths.sort();
        paths.dedup();
        let roots = paths
            .iter()
            .map(|p| RootGrant::capture(p))
            .collect::<Result<Vec<_>>>()?;
        let spool =
            Spool::open(directory.join("captures"), Limits::default()).map_err(|_| invalid())?;
        let projection_root = RootGrant::capture(&spool.markdown_root())?;
        let source = store.add_owned_local_source("Agent captures", projection_root.restore()?)?;
        Grant {
            policy,
            roots,
            source_id: Some(source.id),
            projection_root: Some(projection_root),
        }
    };
    let json = serde_json::to_string(&grant).map_err(|_| invalid())?;
    // Publish the hook's current bounded policy first. If the SQLite commit
    // fails, a policy/artifact mismatch stops service admission until repaired.
    // Disabling therefore stops future hook enqueue before acknowledging it.
    write_policy(directory, &grant.policy)?;
    for root in &grant.roots {
        root.restore()?.verify()?;
    }
    store.save_capture_policy(&grant.policy.project_id, &json)
}
/// Upgrade only existing saved capture grants whose policy artifact, project
/// roots, exact source identity and private spool ownership all still verify.
/// Invalid/revoked grants remain excluded; a stored path/name is not authority.
pub(crate) fn adopt_verified_sources(store: &mut ReaderStore, directory: &Path) -> Result<()> {
    // Bind adoption to this owner's actual open database, before adding any
    // exemption. A different supplied profile cannot certify the namespace.
    store.store_identity()?;
    let profile = SourceRoot::capture(directory.canonicalize()?)?;
    store.library_identity()?.verify_at(&profile.path)?;
    let sources = reader::sources(store, directory)?;
    if sources.is_empty() {
        return Ok(());
    }
    let spool =
        Spool::open(directory.join("captures"), Limits::default()).map_err(|_| invalid())?;
    let root = SourceRoot::capture(spool.markdown_root())?;
    for source in sources {
        let observed = store
            .find_local_source(&root.path, &root.identity)?
            .filter(|s| s.enabled && s.id == source.id)
            .ok_or_else(invalid)?;
        profile.verify()?;
        root.verify()?;
        if !reader::sources(store, directory)?
            .iter()
            .any(|s| s.id == observed.id)
        {
            return Err(invalid());
        }
        store.store_identity()?;
        let admitted = store.add_owned_local_source(&observed.name, root.clone())?;
        if admitted.id != observed.id || !admitted.enabled {
            return Err(invalid());
        }
    }
    profile.verify()?;
    store.store_identity()?;
    Ok(())
}

/// Each pending occurrence is independently retried. Projection failure, changed
/// grants, or failed SQLite admission retain the event without a hook receipt.
pub fn drain(store: &mut ReaderStore, directory: &Path) -> Result<DrainStatus> {
    let grants = policies(store)?;
    if !grants.iter().any(|g| g.policy.enabled) {
        return Ok(DrainStatus::default());
    }
    let spool = match Spool::open(directory.join("captures"), Limits::default()) {
        Ok(spool) => spool,
        Err(_) => {
            return Ok(DrainStatus {
                diagnostic: Some("Capture spool unavailable on this platform or profile".into()),
                ..Default::default()
            });
        }
    };
    #[cfg(windows)]
    {
        // Restore only owned canonical capture output after verifying current
        // source/worktree grants; no transcript or arbitrary file discovery.
        let mut repair = Vec::new();
        for grant in grants.iter().filter(|grant| grant.policy.enabled) {
            artifact_matches(directory, &grant.policy)?;
            for root in &grant.roots {
                root.restore()?.verify()?;
            }
            grant
                .projection_root
                .as_ref()
                .ok_or_else(invalid)?
                .restore()?
                .verify()?;
            store.index_status(&[grant.source_id.clone().ok_or_else(invalid)?])?;
            repair.push(grant.policy.clone());
        }
        for project in spool
            .repair_projections(&repair, 2)
            .map_err(|_| invalid())?
        {
            let grant = grants
                .iter()
                .find(|grant| grant.policy.project_id == project)
                .ok_or_else(invalid)?;
            store.queue_index(grant.source_id.as_deref().ok_or_else(invalid)?)?;
        }
    }
    let mut status = DrainStatus::default();
    for id in spool.pending(32).map_err(|_| invalid())? {
        let admit = (|| -> Result<()> {
            let record = spool.read_pending(&id).map_err(|_| invalid())?;
            let grant = grants
                .iter()
                .find(|g| g.policy.enabled && g.policy.project_id == record.provenance.project_id)
                .ok_or_else(invalid)?;
            artifact_matches(directory, &grant.policy)?;
            for root in &grant.roots {
                root.restore()?.verify()?;
            }
            let projection_root = grant
                .projection_root
                .as_ref()
                .ok_or_else(invalid)?
                .restore()?;
            projection_root.verify()?;
            let source = grant.source_id.as_deref().ok_or_else(invalid)?;
            // Verify the durable reader grant BEFORE any projection mutation.
            store.index_status(&[source.into()])?;
            let projected = spool.project(&id, &grant.policy).map_err(|_| invalid())?;
            for root in &grant.roots {
                root.restore()?.verify()?;
            }
            let path = projection_root
                .relative_path(&projected.path)?
                .to_str()
                .ok_or_else(invalid)?
                .to_string();
            let provenance =
                serde_json::to_string(&projected.record.provenance).map_err(|_| invalid())?;
            store.commit_capture(&id, source, &path, &provenance)?;
            // Crash here is safe: replay confirms the same admission then issues
            // the receipt. Lexical/model completion is intentionally unnecessary.
            spool.acknowledge(&id).map_err(|_| invalid())?;
            Ok(())
        })();
        match admit {
            Ok(()) => status.admitted += 1,
            Err(_) => status.rejected += 1,
        }
    }
    if status.rejected > 0 {
        status.diagnostic =
            Some("Some captures await valid project/source grants or recovery".into());
    }
    Ok(status)
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct CaptureStatus {
    pub enabled_projects: usize,
    pub pending_events: usize,
    pub pending_bytes: u64,
    pub admitted_occurrences: u64,
    pub diagnostic: Option<String>,
}
/// Reading diagnostics does not project, admit, acknowledge or discard events.
pub fn status(store: &ReaderStore, directory: &Path) -> Result<CaptureStatus> {
    let mut result = CaptureStatus {
        enabled_projects: policies(store)?.iter().filter(|g| g.policy.enabled).count(),
        admitted_occurrences: store.capture_count()?,
        ..Default::default()
    };
    if directory.join("captures").exists() {
        match Spool::open(directory.join("captures"), Limits::default())
            .and_then(|spool| spool.status())
        {
            Ok(queue) => {
                result.pending_events = queue.pending_events;
                result.pending_bytes = queue.pending_bytes;
            }
            Err(_) => {
                result.diagnostic =
                    Some("Capture spool unavailable on this platform or profile".into())
            }
        }
    }
    Ok(result)
}

#[cfg(all(test, unix))]
mod adoption_tests;
