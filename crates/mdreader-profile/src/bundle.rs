//! Output-only preservation envelope. The current snapshot protocol is
//! unchanged; exact preflush JSON is separate archival evidence, never replayed.
use crate::{
    Error, FreezeOwner, FrozenProfile, Inventory, Limits, Result, RootBinding, SnapshotIdentity,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

const MAX_PREFLUSH_BYTES: usize = 1024 * 1024;
const MAX_BUNDLE_MANIFEST: usize = 16 * 1024;
#[cfg(all(test, unix))]
mod tests;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflushKind {
    Session,
    Settings,
}
impl PreflushKind {
    fn member(self) -> &'static str {
        match self {
            Self::Session => "session.preflush",
            Self::Settings => "settings.preflush",
        }
    }
}
/// Owned private bytes issued by the actual frozen participant. Absence differs
/// from an existing empty file. No Debug or raw serialization implementation.
pub struct PreflushArtifact {
    pub kind: PreflushKind,
    pub bytes: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BundleIdentity {
    pub id: Uuid,
    pub manifest_sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BundleManifest {
    pub version: u8,
    pub id: Uuid,
    pub current: SnapshotIdentity,
    pub preflush: Vec<PreflushEntry>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreflushEntry {
    pub kind: PreflushKind,
    pub present: bool,
    pub member: Option<String>,
    pub bytes: u64,
    pub sha256: Option<String>,
}
pub struct VerifiedBundle {
    identity: BundleIdentity,
    manifest: BundleManifest,
    current: crate::VerifiedSnapshot,
    #[cfg(unix)]
    root: crate::platform::Root,
}
impl VerifiedBundle {
    pub fn identity(&self) -> &BundleIdentity {
        &self.identity
    }
    pub fn manifest(&self) -> &BundleManifest {
        &self.manifest
    }
    pub fn current(&self) -> &crate::VerifiedSnapshot {
        &self.current
    }
    pub fn revalidate(&self, limits: &Limits) -> Result<()> {
        #[cfg(unix)]
        {
            self.root.verify()?;
            verify_bundle(&self.root.path, &self.identity, limits).map(|_| ())
        }
        #[cfg(not(unix))]
        {
            let _ = limits;
            Err(Error::Unsupported)
        }
    }
}

/// Holds the same real freeze through raw preimage and current-state export,
/// component verification, and one no-replace outer publication. Destination
/// must be explicitly selected outside the original profile tree.
pub fn stage_bundle(
    owner: &mut impl FreezeOwner,
    inventory: Inventory,
    destination: &Path,
    limits: &Limits,
) -> Result<VerifiedBundle> {
    limits.validate()?;
    #[cfg(not(unix))]
    {
        let _ = (owner, inventory, destination);
        Err(Error::Unsupported)
    }
    #[cfg(unix)]
    {
        use crate::platform::{Root, Stage};
        let source = inventory.binding().clone();
        let parent = Root::private(destination.parent().ok_or(Error::Invalid)?)?;
        let original = source.locator.restore()?;
        if parent.path.starts_with(&original)
            || original.starts_with(
                &parent
                    .path
                    .join(destination.file_name().ok_or(Error::Invalid)?),
            )
        {
            return Err(Error::Invalid);
        }
        let identity = owner.with_frozen(&source, |frozen| {
            if frozen.binding() != &source {
                return Err(Error::Conflict);
            }
            frozen.revalidate()?;
            let mut artifacts = frozen.preflush_json()?;
            artifacts.sort_by_key(|a| a.kind);
            if artifacts.len() != 2
                || artifacts[0].kind != PreflushKind::Session
                || artifacts[1].kind != PreflushKind::Settings
            {
                return Err(Error::NotQuiescent);
            }
            let mut total = 0u64;
            for artifact in &artifacts {
                let bytes = artifact.bytes.as_ref().map_or(0, Vec::len);
                if bytes > MAX_PREFLUSH_BYTES {
                    return Err(Error::Bounds);
                }
                total = total.checked_add(bytes as u64).ok_or(Error::Bounds)?;
            }
            let current_limits = remaining_limits(limits, total)?;
            let stage = Stage::create(destination)?;
            let mut entries = Vec::with_capacity(2);
            for artifact in artifacts {
                let (member, bytes, digest) = match artifact.bytes {
                    Some(bytes) => {
                        stage.write(artifact.kind.member(), &bytes)?;
                        (
                            Some(artifact.kind.member().to_owned()),
                            bytes.len() as u64,
                            Some(crate::model::hash(&bytes)),
                        )
                    }
                    None => (None, 0, None),
                };
                entries.push(PreflushEntry {
                    kind: artifact.kind,
                    present: member.is_some(),
                    member,
                    bytes,
                    sha256: digest,
                });
            }
            let mut borrowed = Borrowed { frozen };
            let current = crate::stage(
                &mut borrowed,
                inventory,
                &stage.root.path.join("current"),
                &current_limits,
            )?;
            let manifest = BundleManifest {
                version: 1,
                id: Uuid::new_v4(),
                current: current.identity().clone(),
                preflush: entries,
            };
            let bytes = serde_json::to_vec(&manifest).map_err(|_| Error::Invalid)?;
            if bytes.len() > MAX_BUNDLE_MANIFEST {
                return Err(Error::Bounds);
            }
            let identity = BundleIdentity {
                id: manifest.id,
                manifest_sha256: crate::model::hash(&bytes),
            };
            stage.write("bundle.json", &bytes)?;
            verify_bundle(&stage.root.path, &identity, limits)?;
            borrowed.frozen.revalidate()?;
            parent.verify()?;
            stage.publish()?;
            Ok(identity)
        })?;
        verify_bundle(destination, &identity, limits)
    }
}
struct Borrowed<'a> {
    frozen: &'a mut dyn FrozenProfile,
}
impl FreezeOwner for Borrowed<'_> {
    fn with_frozen<T>(
        &mut self,
        expected: &RootBinding,
        action: impl FnOnce(&mut dyn FrozenProfile) -> Result<T>,
    ) -> Result<T> {
        if self.frozen.binding() != expected {
            return Err(Error::Conflict);
        }
        self.frozen.revalidate()?;
        action(&mut *self.frozen)
    }
}
fn remaining_limits(limits: &Limits, bytes: u64) -> Result<Limits> {
    let mut remaining = limits.clone();
    remaining.max_total_bytes = limits
        .max_total_bytes
        .checked_sub(bytes)
        .filter(|v| *v > 0)
        .ok_or(Error::Bounds)?;
    remaining.max_file_bytes = limits.max_file_bytes.min(remaining.max_total_bytes);
    remaining.validate()?;
    Ok(remaining)
}

pub fn verify_bundle(
    path: &Path,
    expected: &BundleIdentity,
    limits: &Limits,
) -> Result<VerifiedBundle> {
    limits.validate()?;
    if !crate::model::valid_hash(&expected.manifest_sha256) {
        return Err(Error::Invalid);
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(Error::Unsupported)
    }
    #[cfg(unix)]
    {
        use crate::platform::{hash_file, stamp, Root};
        use std::{collections::BTreeSet, io::Read, os::unix::fs::PermissionsExt};
        let root = Root::private(path)?;
        let mut file = root.open(Path::new("bundle.json"))?;
        let original = stamp(&file)?;
        if file
            .metadata()
            .map_err(|_| Error::Storage)?
            .permissions()
            .mode()
            & 0o077
            != 0
        {
            return Err(Error::Integrity);
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_BUNDLE_MANIFEST as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Storage)?;
        if bytes.len() > MAX_BUNDLE_MANIFEST
            || crate::model::hash(&bytes) != expected.manifest_sha256
        {
            return Err(Error::Integrity);
        }
        let manifest: BundleManifest =
            serde_json::from_slice(&bytes).map_err(|_| Error::Integrity)?;
        if manifest.version != 1 || manifest.id != expected.id || manifest.preflush.len() != 2 {
            return Err(Error::Integrity);
        }
        let mut members = BTreeSet::from(["bundle.json".to_owned(), "current".to_owned()]);
        let mut kinds = BTreeSet::new();
        let mut total = 0u64;
        let mut evidence = Vec::new();
        for entry in &manifest.preflush {
            if !kinds.insert(entry.kind) || entry.bytes > MAX_PREFLUSH_BYTES as u64 {
                return Err(Error::Integrity);
            }
            if !entry.present {
                if entry.member.is_some() || entry.bytes != 0 || entry.sha256.is_some() {
                    return Err(Error::Integrity);
                }
                continue;
            }
            let name = entry.kind.member();
            if entry.member.as_deref() != Some(name)
                || entry
                    .sha256
                    .as_deref()
                    .is_none_or(|v| !crate::model::valid_hash(v))
            {
                return Err(Error::Integrity);
            }
            let mut file = root.open(Path::new(name))?;
            let before = stamp(&file)?;
            if file
                .metadata()
                .map_err(|_| Error::Storage)?
                .permissions()
                .mode()
                & 0o077
                != 0
            {
                return Err(Error::Integrity);
            }
            let (bytes, digest) = hash_file(&mut file, MAX_PREFLUSH_BYTES as u64)?;
            if bytes != entry.bytes
                || Some(&digest) != entry.sha256.as_ref()
                || before != stamp(&file)?
            {
                return Err(Error::Integrity);
            }
            total = total.checked_add(bytes).ok_or(Error::Bounds)?;
            evidence.push((name, file, before));
            members.insert(name.to_owned());
        }
        let current = crate::verify(
            &root.path.join("current"),
            &manifest.current,
            &remaining_limits(limits, total)?,
        )?;
        let actual = std::fs::read_dir(&root.path)
            .map_err(|_| Error::Storage)?
            .take(5)
            .map(|entry| {
                entry
                    .map_err(|_| Error::Storage)
                    .and_then(|v| v.file_name().into_string().map_err(|_| Error::Integrity))
            })
            .collect::<Result<BTreeSet<_>>>()?;
        if actual != members
            || original != stamp(&file)?
            || original != stamp(&root.open(Path::new("bundle.json"))?)?
        {
            return Err(Error::Integrity);
        }
        for (name, file, before) in evidence {
            if before != stamp(&file)? || before != stamp(&root.open(Path::new(name))?)? {
                return Err(Error::Integrity);
            }
        }
        root.verify()?;
        Ok(VerifiedBundle {
            identity: expected.clone(),
            manifest,
            current,
            root,
        })
    }
}
