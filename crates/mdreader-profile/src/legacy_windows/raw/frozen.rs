//! Adapt real Windows sharing-denial closure to the existing profile snapshot
//! and lifecycle contracts. Every original, including recovery sidecars, is raw.
use super::*;
use crate::{Disposition, FreezeOwner, FrozenProfile, Inventory, ManifestEntry};

impl ClosedOriginal {
    /// Build the same inventory identity the lifecycle journal understands.
    /// Discovery/classification is owned by this closed capability: no supplied
    /// offline flag can admit an endpoint, journal, or arbitrary original file.
    pub fn inventory(&mut self) -> Result<Inventory> {
        self.revalidate()?;
        let root = crate::platform::Root::capture(&self.application.profile.path)?;
        if root.binding != self.binding {
            return Err(Error::Conflict);
        }
        let mut files = Vec::new();
        let mut entries = Vec::new();
        for (relative, original) in &self.files {
            let retained = original.file.try_clone().map_err(|_| Error::Storage)?;
            if crate::platform::stamp(&retained)? != original.stamp {
                return Err(Error::Conflict);
            }
            entries.push(ManifestEntry {
                relative: RelativeLocator::capture(relative)?,
                disposition: Disposition::Raw,
                source_stamp: original.stamp,
                member: None,
                bytes: original.stamp.2,
                sha256: None,
            });
            files.push((relative.clone(), Some(retained), original.stamp));
        }
        let directories = self
            .scan
            .directories
            .iter()
            .map(|(relative, (_, stamp))| (relative.clone(), *stamp))
            .collect::<Vec<_>>();
        let directory_stamps = directories
            .iter()
            .filter(|(relative, _)| !relative.as_os_str().is_empty())
            .map(|(relative, stamp)| Ok((RelativeLocator::capture(relative)?, *stamp)))
            .collect::<Result<Vec<_>>>()?;
        let file_stamps = files.iter().map(|(_, _, stamp)| *stamp).collect::<Vec<_>>();
        let identity = crate::model::digest(&(
            self.binding.clone(),
            entries.clone(),
            file_stamps,
            directory_stamps,
        ))?;
        let result = Inventory {
            binding: self.binding.clone(),
            identity,
            entries,
            root,
            files,
            directories,
        };
        result.revalidate()?;
        self.revalidate()?;
        Ok(result)
    }
}
impl FreezeOwner for ClosedOriginal {
    fn with_frozen<T>(
        &mut self,
        expected: &RootBinding,
        action: impl FnOnce(&mut dyn FrozenProfile) -> Result<T>,
    ) -> Result<T> {
        if expected != &self.binding {
            return Err(Error::Conflict);
        }
        self.revalidate()?;
        let result = action(self)?;
        self.revalidate()?;
        Ok(result)
    }
}
impl FrozenProfile for ClosedOriginal {
    fn binding(&self) -> &RootBinding {
        &self.binding
    }
    fn revalidate(&mut self) -> Result<()> {
        ClosedOriginal::revalidate(self)
    }
    fn with_database(
        &mut self,
        _: &Path,
        _: &mut dyn FnMut(&rusqlite::Connection) -> Result<()>,
    ) -> Result<()> {
        // Useful-record conversion explicitly calls recover_database instead.
        // The raw freeze contract must never run SQL on an original.
        Err(Error::Unsupported)
    }
}
