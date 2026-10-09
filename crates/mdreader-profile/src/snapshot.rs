#[cfg(any(unix, windows))]
pub(crate) mod backup;
#[cfg(any(unix, windows))]
use crate::database_backup::{BackupSource, DatabaseBackupTarget};
#[cfg(any(unix, windows))]
use crate::RelativeLocator;
use crate::{
    Disposition, Error, Inventory, Limits, Manifest, Result, RootBinding, SnapshotIdentity,
};
use rusqlite::Connection;
use std::path::Path;

// Only synthetic unit-test builds emit fixed public phase names on refusal.
// Original paths, file names, contents and OS/process identities never appear.
#[cfg(any(unix, windows))]
macro_rules! phase {
    ($name:literal, $action:expr) => {{
        let result = $action;
        #[cfg(test)]
        if result.is_err() {
            eprintln!("profile snapshot refused at {}", $name);
        }
        result
    }};
}

/// Implemented by the native composition owner with ALL required participants.
/// The callback runs only after denial of new effects, strict JSON success acks,
/// and joined completion of already issued work. Deadline failure is an error;
/// it must never invoke the callback. No boolean/offline assertion substitutes.
pub trait FreezeOwner {
    fn with_frozen<T>(
        &mut self,
        expected: &RootBinding,
        action: impl FnOnce(&mut dyn FrozenProfile) -> Result<T>,
    ) -> Result<T>;
}
/// A borrowed exclusive snapshot owner. It cannot escape `with_frozen`.
/// Implementations yield actual owner connections outside unrelated owner locks;
/// they must not open/migrate a selected original with a legacy SDK.
/// Retain/check DB object identity from owner creation: SQLite's exposed pathname
/// alone cannot detect an old connection after that pathname was replaced.
/// A legacy owner without a borrow API may instead yield a read-only SQLite
/// backup connection only after its real shutdown/drain capability has closed
/// every writer and denied restart. Retain/revalidate its original DB identity
/// and recovery sidecars throughout; a caller's offline boolean is insufficient.
pub trait FrozenProfile {
    fn binding(&self) -> &RootBinding;
    fn revalidate(&mut self) -> Result<()>;
    /// Exact preflush JSON issued by real renderer persistence participants.
    /// This optional seam is required only by the output-only outer bundle;
    /// existing current-snapshot archives keep their protocol unchanged.
    fn preflush_json(&mut self) -> Result<Vec<crate::bundle::PreflushArtifact>> {
        Err(Error::NotQuiescent)
    }
    /// Local default borrows the real owner; remote participants override this
    /// exact staged-target operation without pretending to yield a Connection.
    fn backup_database(
        &mut self,
        target: &mut crate::database_backup::DatabaseBackupTarget,
    ) -> Result<crate::database_backup::BackupReceipt> {
        #[cfg(any(unix, windows))]
        {
            target.local(self)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = target;
            Err(Error::Unsupported)
        }
    }
    fn with_database(
        &mut self,
        relative: &Path,
        action: &mut dyn FnMut(&Connection) -> Result<()>,
    ) -> Result<()>;
}
/// Created only by complete verification against an explicitly expected digest.
/// This is preservation evidence, not permission to grant or activate a profile.
pub struct VerifiedSnapshot {
    identity: SnapshotIdentity,
    manifest: Manifest,
    #[cfg(any(unix, windows))]
    root: crate::platform::Root,
}
impl VerifiedSnapshot {
    pub fn identity(&self) -> &SnapshotIdentity {
        &self.identity
    }
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn revalidate(&self, limits: &Limits) -> Result<()> {
        #[cfg(any(unix, windows))]
        {
            self.root.verify()?;
            verify(&self.root.path, &self.identity, limits).map(|_| ())
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = limits;
            Err(Error::Unsupported)
        }
    }
}
pub fn stage(
    owner: &mut impl FreezeOwner,
    inventory: Inventory,
    destination: &Path,
    limits: &Limits,
) -> Result<VerifiedSnapshot> {
    limits.validate()?;
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (owner, inventory, destination);
        Err(Error::Unsupported)
    }
    #[cfg(any(unix, windows))]
    stage_inner(owner, inventory, destination, limits, None)
}

/// Intermediate snapshot only: its owning unpublished bundle stage retains
/// the publication capability. It cannot issue a standalone durable receipt.
#[cfg(any(unix, windows))]
pub(crate) fn stage_nested(
    owner: &mut impl FreezeOwner,
    inventory: Inventory,
    parent: &crate::platform::Stage,
    limits: &Limits,
) -> Result<VerifiedSnapshot> {
    stage_inner(
        owner,
        inventory,
        &parent.root.path.join("current"),
        limits,
        Some(parent),
    )
}

#[cfg(any(unix, windows))]
fn stage_inner(
    owner: &mut impl FreezeOwner,
    mut inventory: Inventory,
    destination: &Path,
    limits: &Limits,
    parent: Option<&crate::platform::Stage>,
) -> Result<VerifiedSnapshot> {
    limits.validate()?;
    use crate::platform::{copy_file, hash_file, Stage};
    if inventory
        .entries
        .iter()
        .any(|e| e.disposition == Disposition::Unclassified)
    {
        return Err(Error::Unclassified);
    }
    let binding = inventory.binding.clone();
    let identity = owner.with_frozen(&binding, |frozen| {
        if frozen.binding() != &binding {
            return Err(Error::Conflict);
        }
        phase!("original_before", frozen.revalidate())?;
        phase!("inventory_before", inventory.revalidate())?;
        let stage = match parent {
            Some(parent) => Stage::create_nested(parent, destination)?,
            None => phase!("stage_create", Stage::create(destination))?,
        };
        let mut total = 0u64;
        for (index, entry) in inventory.entries.iter_mut().enumerate() {
            if matches!(entry.disposition, Disposition::Excluded(_)) {
                continue;
            }
            let name = format!("{index:05}.snapshot");
            let mut output = stage.create_file(&name)?;
            let (path, input, expected) = &mut inventory.files[index];
            let bound = limits
                .max_file_bytes
                .min(limits.max_total_bytes.saturating_sub(total));
            let result = match entry.disposition {
                Disposition::Raw => {
                    copy_file(input.as_mut().ok_or(Error::Invalid)?, &mut output, bound)?
                }
                Disposition::Sqlite => {
                    let retained = output.try_clone().map_err(|_| Error::Storage)?;
                    let mut target = phase!(
                        "backup_target",
                        DatabaseBackupTarget::new(
                            &stage,
                            &name,
                            retained,
                            BackupSource {
                                root: inventory.root.binding.clone(),
                                relative: RelativeLocator::capture(path)?,
                                stamp: *expected,
                            },
                            bound,
                            limits,
                        )
                    )?;
                    let receipt = phase!("database_backup", frozen.backup_database(&mut target))?;
                    phase!("backup_accept", target.accept(&receipt))?;
                    (receipt.bytes, receipt.sha256)
                }
                _ => return Err(Error::Invalid),
            };
            if entry.disposition == Disposition::Raw
                && hash_file(input.as_mut().ok_or(Error::Invalid)?, bound)? != result
            {
                return Err(Error::Conflict);
            }
            total = total.checked_add(result.0).ok_or(Error::Bounds)?;
            entry.member = Some(name);
            entry.bytes = result.0;
            entry.sha256 = Some(result.1);
        }
        phase!("inventory_after_copy", inventory.revalidate())?;
        phase!("original_after_copy", frozen.revalidate())?;
        let manifest = Manifest {
            version: 1,
            id: uuid::Uuid::new_v4(),
            original: binding.clone(),
            inventory_sha256: inventory.identity.clone(),
            entries: inventory.entries.clone(),
            directories: inventory
                .directories
                .iter()
                .filter(|(path, _)| !path.as_os_str().is_empty())
                .map(|(path, stamp)| {
                    Ok(crate::ManifestDirectory {
                        relative: crate::RelativeLocator::capture(path)?,
                        source_stamp: *stamp,
                    })
                })
                .collect::<Result<_>>()?,
        };
        let bytes = serde_json::to_vec(&manifest).map_err(|_| Error::Invalid)?;
        if bytes.len() > limits.max_manifest_bytes {
            return Err(Error::Bounds);
        }
        let identity = SnapshotIdentity {
            id: manifest.id,
            manifest_sha256: crate::model::hash(&bytes),
        };
        phase!("manifest_write", stage.write("manifest.json", &bytes))?;
        phase!("inventory_before_publish", inventory.revalidate())?;
        phase!("original_before_publish", frozen.revalidate())?;
        phase!("stage_publish", stage.publish())?;
        Ok(identity)
    })?;
    phase!("published_verify", verify(destination, &identity, limits))
}

pub fn verify(
    archive: &Path,
    expected: &SnapshotIdentity,
    limits: &Limits,
) -> Result<VerifiedSnapshot> {
    limits.validate()?;
    if !crate::model::valid_hash(&expected.manifest_sha256) {
        return Err(Error::Invalid);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = archive;
        Err(Error::Unsupported)
    }
    #[cfg(any(unix, windows))]
    {
        use crate::platform::{hash_file, stamp, Root};
        use std::io::Read;
        let root = Root::private(archive)?;
        let mut file = root.open(Path::new("manifest.json"))?;
        let original = stamp(&file)?;
        crate::platform::private_file(&file)?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(limits.max_manifest_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Storage)?;
        if bytes.len() > limits.max_manifest_bytes {
            return Err(Error::Bounds);
        }
        if crate::model::hash(&bytes) != expected.manifest_sha256 {
            return Err(Error::Integrity);
        }
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| Error::Integrity)?;
        manifest.original.validate()?;
        if manifest.version != 1
            || manifest.id != expected.id
            || !crate::model::valid_hash(&manifest.inventory_sha256)
            || manifest.entries.len() > limits.max_entries
        {
            return Err(Error::Integrity);
        }
        let mut members = std::collections::BTreeSet::from(["manifest.json".to_owned()]);
        let mut paths = std::collections::BTreeSet::new();
        let mut evidence = Vec::new();
        let mut total = 0u64;
        if manifest.directories.len() + manifest.entries.len() > limits.max_entries {
            return Err(Error::Bounds);
        }
        let mut directory_paths = std::collections::BTreeSet::new();
        for directory in &manifest.directories {
            let path = directory.relative.restore().map_err(|_| Error::Integrity)?;
            crate::model::relative(&path)?;
            if path.components().count() > limits.max_depth || !directory_paths.insert(path) {
                return Err(Error::Integrity);
            }
        }
        for (index, entry) in manifest.entries.iter().enumerate() {
            let path = entry.relative.restore().map_err(|_| Error::Integrity)?;
            crate::model::relative(&path)?;
            if path.components().count() > limits.max_depth
                || directory_paths.contains(&path)
                || !paths.insert(path)
            {
                return Err(Error::Integrity);
            }
            match entry.disposition {
                Disposition::Raw | Disposition::Sqlite => {
                    let name = format!("{index:05}.snapshot");
                    if entry.member.as_ref() != Some(&name)
                        || entry
                            .sha256
                            .as_deref()
                            .is_none_or(|s| !crate::model::valid_hash(s))
                    {
                        return Err(Error::Integrity);
                    }
                    let mut file = root.open(Path::new(&name))?;
                    let before = stamp(&file)?;
                    crate::platform::private_file(&file)?;
                    let (bytes, hash) = hash_file(&mut file, limits.max_file_bytes)?;
                    if bytes != entry.bytes
                        || Some(&hash) != entry.sha256.as_ref()
                        || before != stamp(&file)?
                        || before != stamp(&root.open(Path::new(&name))?)?
                    {
                        return Err(Error::Integrity);
                    }
                    total = total.checked_add(bytes).ok_or(Error::Bounds)?;
                    if total > limits.max_total_bytes {
                        return Err(Error::Bounds);
                    }
                    evidence.push((name.clone(), file, before));
                    members.insert(name);
                }
                Disposition::Excluded(_) => {
                    if entry.member.is_some() || entry.sha256.is_some() {
                        return Err(Error::Integrity);
                    }
                }
                Disposition::Unclassified => return Err(Error::Unclassified),
            }
        }
        let actual = std::fs::read_dir(&root.path)
            .map_err(|_| Error::Storage)?
            .take(limits.max_entries + 2)
            .map(|e| {
                e.map_err(|_| Error::Storage)
                    .and_then(|e| e.file_name().into_string().map_err(|_| Error::Integrity))
            })
            .collect::<Result<std::collections::BTreeSet<_>>>()?;
        if actual != members
            || stamp(&file)? != original
            || stamp(&root.open(Path::new("manifest.json"))?)? != original
        {
            return Err(Error::Integrity);
        }
        for (name, file, expected) in evidence {
            if stamp(&file)? != expected || stamp(&root.open(Path::new(&name))?)? != expected {
                return Err(Error::Integrity);
            }
        }
        root.verify()?;
        Ok(VerifiedSnapshot {
            identity: expected.clone(),
            manifest,
            root,
        })
    }
}
