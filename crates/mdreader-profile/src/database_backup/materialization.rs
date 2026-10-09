use super::*;
use crate::platform::{stamp, Root};
use crate::{Error, Result};
use rusqlite::Connection;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::Instant,
};

/// Trusted service-side writer for one exact pre-created stage member. This
/// object never opens the original SQLite file; callers lend its actual owner.
pub struct RemoteBackup {
    request: BackupRequest,
    source: Root,
    target: Root,
    file: File,
    deadline: Instant,
    attempted: bool,
    written: bool,
}
impl RemoteBackup {
    pub fn open(request: BackupRequest, deadline: Instant) -> Result<Self> {
        request.validate()?;
        if Instant::now() >= deadline {
            return Err(Error::NotQuiescent);
        }
        let source = Root::capture(&request.source.root.locator.restore()?)?;
        let target = Root::private(&request.destination.root.locator.restore()?)?;
        if source.binding != request.source.root || target.binding != request.destination.root {
            return Err(Error::Conflict);
        }
        let file = target.open_backup_member(Path::new(&request.destination.member))?;
        crate::platform::backup_file(&file)?;
        if stamp(&file)? != request.destination.stamp {
            return Err(Error::Conflict);
        }
        let material = Self {
            request,
            source,
            target,
            file,
            deadline,
            attempted: false,
            written: false,
        };
        material.check_target()?;
        Ok(material)
    }
    fn time(&self) -> Result<()> {
        if Instant::now() >= self.deadline {
            Err(Error::NotQuiescent)
        } else {
            Ok(())
        }
    }
    pub fn check_source(&self, connection: &Connection) -> Result<()> {
        self.time()?;
        crate::snapshot::backup::check_database(
            connection,
            &self.source,
            &self.request.source.relative.restore()?,
            &self.request.source.stamp,
        )
    }
    fn check_target(&self) -> Result<FileStamp> {
        self.time()?;
        self.target.verify()?;
        let named = self
            .target
            .named_stamp(Path::new(&self.request.destination.member))?;
        let retained = stamp(&self.file)?;
        if named != retained
            || !crate::platform::same_identity(&named, &self.request.destination.stamp)
            || named.2 > self.request.max_bytes
        {
            return Err(Error::Conflict);
        }
        crate::platform::backup_file(&self.file)?;
        Ok(named)
    }
    fn check_output(&self, output: &Connection) -> Result<()> {
        let expected = self.check_target()?;
        crate::live_database::check(
            output,
            &self.target,
            Path::new(&self.request.destination.member),
            &expected,
        )
    }
    pub fn write_from(&mut self, source: &Connection) -> Result<()> {
        if self.attempted {
            return Err(Error::ReconciliationRequired);
        }
        self.attempted = true;
        self.check_source(source)?;
        if self.check_target()? != self.request.destination.stamp {
            return Err(Error::Conflict);
        }
        #[cfg(windows)]
        self.target
            .prepare_backup_journal(&self.request.destination.member)?;
        let mut output = Connection::open_with_flags(
            self.target.path.join(&self.request.destination.member),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|_| Error::Storage)?;
        // No target SQL precedes the actual SQLite VFS + expected inode fence.
        self.check_output(&output)?;
        crate::snapshot::backup::backup(
            source,
            &mut output,
            self.request.max_bytes,
            self.request.max_steps,
            || self.time(),
        )?;
        self.check_source(source)?;
        self.check_output(&output)?;
        let mode: String = output
            .query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))
            .map_err(|_| Error::Storage)?;
        if mode != "delete" {
            return Err(Error::Integrity);
        }
        self.check_output(&output)?;
        output.close().map_err(|_| Error::Storage)?;
        #[cfg(windows)]
        self.target
            .finish_backup_journal(&self.request.destination.member)?;
        self.file.sync_all().map_err(|_| Error::Storage)?;
        #[cfg(unix)]
        self.target.file.sync_all().map_err(|_| Error::Storage)?;
        self.check_target()?;
        self.check_source(source)?;
        self.written = true;
        Ok(())
    }
    /// Invoke after returning the owner connection: hashing never needs it.
    pub fn finish(&mut self) -> Result<BackupReceipt> {
        if !self.written {
            return Err(Error::ReconciliationRequired);
        }
        let before = self.check_target()?;
        let (bytes, sha256) = hash(&mut self.file, self.request.max_bytes, self.deadline)?;
        if self.check_target()? != before {
            return Err(Error::Conflict);
        }
        let receipt = BackupReceipt {
            request: self.request.clone(),
            target_stamp: before,
            bytes,
            sha256,
        };
        receipt.validate()?;
        Ok(receipt)
    }
    pub fn verify_receipt(&self, receipt: &BackupReceipt) -> Result<()> {
        receipt.validate()?;
        if receipt.request != self.request || self.check_target()? != receipt.target_stamp {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}
pub(super) fn hash(file: &mut File, bound: u64, deadline: Instant) -> Result<(u64, String)> {
    use sha2::{Digest, Sha256};
    file.seek(SeekFrom::Start(0)).map_err(|_| Error::Storage)?;
    let mut digest = Sha256::new();
    let mut count = 0u64;
    let mut buffer = [0; 65536];
    loop {
        if Instant::now() >= deadline {
            return Err(Error::NotQuiescent);
        }
        let n = file.read(&mut buffer).map_err(|_| Error::Storage)?;
        if n == 0 {
            break;
        }
        count = count.checked_add(n as u64).ok_or(Error::Bounds)?;
        if count > bound {
            return Err(Error::Bounds);
        }
        digest.update(&buffer[..n]);
    }
    Ok((count, crate::model::hex(&digest.finalize())))
}
