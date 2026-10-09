use super::*;
use crate::{
    platform::{stamp, Root, Stage},
    Error, Limits, Result,
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
        source: BackupSource,
        bound: u64,
        limits: &Limits,
    ) -> Result<Self> {
        let root = Root::private(&stage.root.path)?;
        if root.binding != stage.root.binding {
            return Err(Error::Conflict);
        }
        let request = BackupRequest {
            id: uuid::Uuid::new_v4(),
            source,
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
    /// An actually closed native owner may lend its privately recovered SQLite
    /// snapshot instead of opening the original WAL VFS. The native capability
    /// retains/verifies every original DB and sidecar descriptor/digest; this
    /// target still verifies the exact original request, actual private SQLite
    /// handle, staged inode and receipt. This is not a caller's offline boolean.
    pub fn with_closed_snapshot(
        &mut self,
        connection: &rusqlite::Connection,
        recovered: BackupSource,
        mut revalidate_original: impl FnMut() -> Result<()>,
    ) -> Result<BackupReceipt> {
        self.verify_empty()?;
        let original = Root::capture(&self.request.source.root.locator.restore()?)?;
        let relative = self.request.source.relative.restore()?;
        let verify = || -> Result<()> {
            if original.binding != self.request.source.root
                || original.named_stamp(&relative)? != self.request.source.stamp
            {
                return Err(Error::Conflict);
            }
            original.verify()
        };
        revalidate_original()?;
        verify()?;
        // The recovered copy is a separate private object; it never replaces
        // the original identity in the durable snapshot receipt.
        let recovered_root = Root::private(&recovered.root.locator.restore()?)?;
        if recovered_root.binding != recovered.root
            || recovered_root.path.starts_with(&original.path)
            || original.path.starts_with(&recovered_root.path)
        {
            return Err(Error::Conflict);
        }
        let readonly: bool = connection
            .query_row("PRAGMA query_only", [], |r| r.get(0))
            .map_err(|_| Error::Storage)?;
        if !readonly {
            return Err(Error::NotQuiescent);
        }
        let mut request = self.request.clone();
        request.source = recovered;
        let mut backup = RemoteBackup::open(request, self.deadline)?;
        backup.write_from(connection)?;
        revalidate_original()?;
        verify()?;
        let mut receipt = backup.finish()?;
        receipt.request = self.request.clone();
        self.accept(&receipt)?;
        revalidate_original()?;
        original.verify()?;
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
