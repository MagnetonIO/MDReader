use super::*;
use crate::{
    Error, Limits, RelativeLocator, Result,
    platform::{Root, Stage, stamp},
};
use std::{
    fs::File,
    path::Path,
    time::{Duration, Instant},
};

/// Constructed only by snapshot staging. No caller-selected arbitrary writer.
pub struct DatabaseBackupTarget {
    request: BackupRequest,
    root: Root,
    file: File,
    deadline: Instant,
}
impl DatabaseBackupTarget {
    pub(crate) fn new(
        stage: &Stage,
        member: &str,
        file: File,
        source_root: &Root,
        source: &Path,
        expected: FileStamp,
        bound: u64,
        limits: &Limits,
    ) -> Result<Self> {
        let root = Root::private(&stage.root.path)?;
        if root.binding != stage.root.binding {
            return Err(Error::Conflict);
        }
        let request = BackupRequest {
            id: uuid::Uuid::new_v4(),
            source: BackupSource {
                root: source_root.binding.clone(),
                relative: RelativeLocator::capture(source)?,
                stamp: expected,
            },
            destination: BackupDestination {
                root: root.binding.clone(),
                member: member.into(),
                stamp: stamp(&file)?,
            },
            max_bytes: bound,
            max_steps: limits.max_backup_steps,
            duration_ms: 30_000,
        };
        request.validate()?;
        let target = Self {
            request,
            root,
            file,
            deadline: Instant::now() + Duration::from_secs(30),
        };
        target.verify_empty()?;
        Ok(target)
    }
    pub fn request(&self) -> &BackupRequest {
        &self.request
    }
    fn verify_empty(&self) -> Result<()> {
        self.root.verify()?;
        if self
            .root
            .named_stamp(Path::new(&self.request.destination.member))?
            != self.request.destination.stamp
            || stamp(&self.file)? != self.request.destination.stamp
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub(crate) fn local<T: crate::FrozenProfile + ?Sized>(
        &mut self,
        owner: &mut T,
    ) -> Result<BackupReceipt> {
        self.with_local_database(|source, action| owner.with_database(source, action))
    }
    /// Native local adapters lend their actual owner only within this callback.
    /// Remote adapters must use the typed service request instead.
    pub fn with_local_database(
        &mut self,
        mut lend: impl FnMut(&Path, &mut dyn FnMut(&rusqlite::Connection) -> Result<()>) -> Result<()>,
    ) -> Result<BackupReceipt> {
        self.verify_empty()?;
        let mut backup = RemoteBackup::open(self.request.clone(), self.deadline)?;
        let source = self.request.source.relative.restore()?;
        let mut invoked = false;
        lend(&source, &mut |connection| {
            if invoked {
                return Err(Error::Invalid);
            }
            invoked = true;
            backup.write_from(connection)
        })?;
        if !invoked {
            return Err(Error::NotQuiescent);
        }
        // Hashing happens after the actual-owner callback/borrow has ended.
        let receipt = backup.finish()?;
        self.accept(&receipt)?;
        Ok(receipt)
    }
    /// Independently verifies an exact remote receipt and retained member.
    /// Never opens the original database or writes/overwrites a target.
    pub fn accept(&mut self, receipt: &BackupReceipt) -> Result<()> {
        receipt.validate()?;
        if receipt.request != self.request || Instant::now() >= self.deadline {
            return Err(Error::Conflict);
        }
        self.root.verify()?;
        let expected = self
            .root
            .named_stamp(Path::new(&self.request.destination.member))?;
        if expected != receipt.target_stamp || stamp(&self.file)? != expected {
            return Err(Error::Conflict);
        }
        self.file.sync_all().map_err(|_| Error::Storage)?;
        let actual =
            super::materialization::hash(&mut self.file, self.request.max_bytes, self.deadline)?;
        if actual != (receipt.bytes, receipt.sha256.clone())
            || stamp(&self.file)? != expected
            || self
                .root
                .named_stamp(Path::new(&self.request.destination.member))?
                != expected
        {
            return Err(Error::Integrity);
        }
        self.root.verify()
    }
}
