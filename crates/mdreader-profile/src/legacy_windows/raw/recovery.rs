//! Recover only disposable private copies. Original database and recovery
//! sidecar handles stay retained without write/delete sharing throughout.
use super::*;
use mdreader_capture::PrivateDirectory;
use rusqlite::{
    backup::{Backup, StepResult},
    Connection, OpenFlags,
};
use std::time::{Duration, Instant};

/// Sealed standalone SQLite conversion input. The private working namespace is
/// not an activation target; only a separately verified archive can be imported.
pub struct RecoveredDatabase {
    directory: crate::platform::Root,
    file: File,
    stamp: WindowsObjectStamp,
    bytes: u64,
    sha256: String,
    bound: u64,
}
impl RecoveredDatabase {
    pub fn path(&self) -> PathBuf {
        self.directory.path.join("recovered.sqlite")
    }
    pub fn verify(&mut self) -> Result<()> {
        self.directory.verify()?;
        let named = self.directory.open(Path::new("recovered.sqlite"))?;
        if observe_object(&self.file, false, true).map_err(|_| Error::Conflict)? != self.stamp
            || observe_object(&named, false, true).map_err(|_| Error::Conflict)? != self.stamp
            || crate::platform::hash_file(&mut self.file, self.bound)?
                != (self.bytes, self.sha256.clone())
        {
            return Err(Error::Conflict);
        }
        self.directory.verify()
    }
}
impl ClosedOriginal {
    /// Produce a bounded standalone SQLite input for useful-record decoders,
    /// including committed WAL pages or a hot rollback journal. SQLite opens
    /// only the fresh private working copies; no legacy SDK or original SQL runs.
    pub fn recover_database(
        &mut self,
        relative: &Path,
        private_parent: &Path,
    ) -> Result<RecoveredDatabase> {
        crate::model::relative(relative)?;
        self.revalidate()?;
        let parent = crate::platform::Root::private(private_parent)?;
        let original = self.binding.locator.restore()?;
        if parent.path.starts_with(&original) {
            return Err(Error::InvalidDestination);
        }
        let selected = PrivateDirectory::open(&parent.path).map_err(|_| Error::Conflict)?;
        let name = format!("legacy-recovery-{}", uuid::Uuid::new_v4());
        let private = selected
            .create_child(Path::new(&name))
            .map_err(|_| Error::Storage)?;
        let directory = crate::platform::Root::private(&parent.path.join(name))?;
        let bound = self.limits.max_file_bytes.min(256 * 1024 * 1024);
        let mut total = 0u64;
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut native = relative.as_os_str().to_owned();
            native.push(suffix);
            let source = self
                .application
                .profile
                .open_optional_file_relative(Path::new(&native))
                .map_err(|_| Error::Conflict)?;
            let Some(source) = source else {
                if suffix.is_empty() {
                    return Err(Error::Integrity);
                }
                continue;
            };
            let expected = observe_object(&source, false, false).map_err(|_| Error::Conflict)?;
            // Join by full opened object stamp, not case-sensitive spelling:
            // Windows may preserve uppercase sidecar names for a lowercase DB.
            let retained = self
                .files
                .values_mut()
                .find(|entry| entry.stamp == expected)
                .ok_or(Error::Conflict)?;
            total = total.checked_add(expected.2).ok_or(Error::Bounds)?;
            if total > bound {
                return Err(Error::Bounds);
            }
            let mut output = private
                .create_database_file(Path::new(&format!("working.sqlite{suffix}")))
                .map_err(|_| Error::Storage)?;
            let copied = crate::platform::copy_file(&mut retained.file, &mut output, bound)?;
            if copied.0 != expected.2
                || crate::platform::hash_file(&mut retained.file, bound)? != copied
                || observe_object(&retained.file, false, false).map_err(|_| Error::Conflict)?
                    != expected
            {
                return Err(Error::Conflict);
            }
        }
        self.revalidate()?;
        let input = Connection::open_with_flags(
            directory.path.join("working.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|_| Error::Integrity)?;
        input
            .busy_timeout(Duration::ZERO)
            .map_err(|_| Error::Storage)?;
        input
            .execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")
            .map_err(|_| Error::Integrity)?;
        let pages: u64 = input
            .query_row("PRAGMA page_count", [], |row| row.get(0))
            .map_err(|_| Error::Integrity)?;
        let page_size: u64 = input
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(|_| Error::Integrity)?;
        if pages == 0 || pages.checked_mul(page_size).ok_or(Error::Bounds)? > bound {
            return Err(Error::Bounds);
        }
        drop(
            private
                .create_database_file(Path::new("recovered.sqlite"))
                .map_err(|_| Error::Storage)?,
        );
        let mut output = Connection::open_with_flags(
            directory.path.join("recovered.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|_| Error::Storage)?;
        output
            .busy_timeout(Duration::ZERO)
            .map_err(|_| Error::Storage)?;
        let backup = Backup::new(&input, &mut output).map_err(|_| Error::Integrity)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut steps = 0usize;
        loop {
            steps += 1;
            if steps > self.limits.max_backup_steps || Instant::now() >= deadline {
                return Err(Error::NotQuiescent);
            }
            match backup.step(256).map_err(|_| Error::Integrity)? {
                StepResult::Done => break,
                StepResult::More => (),
                _ => return Err(Error::NotQuiescent),
            }
        }
        drop(backup);
        let mode: String = output
            .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
            .map_err(|_| Error::Integrity)?;
        if mode != "delete" {
            return Err(Error::Integrity);
        }
        output.close().map_err(|_| Error::Storage)?;
        input.close().map_err(|_| Error::Storage)?;
        private
            .database_file(Path::new("recovered.sqlite"), false)
            .map_err(|_| Error::Storage)?
            .ok_or(Error::Storage)?
            .sync_all()
            .map_err(|_| Error::Storage)?;
        // Seal the finished member against subsequent writes/deletion before
        // handing its native pathname to the readonly immutable decoder.
        let root = SourceRoot::capture(directory.path.clone()).map_err(|_| Error::Conflict)?;
        let mut file = files::retain(&root, Path::new("recovered.sqlite"))?;
        let stamp = observe_object(&file, false, true).map_err(|_| Error::Conflict)?;
        let (bytes, sha256) = crate::platform::hash_file(&mut file, bound)?;
        self.revalidate()?;
        private.verify().map_err(|_| Error::Conflict)?;
        selected.verify().map_err(|_| Error::Conflict)?;
        parent.verify()?;
        let mut result = RecoveredDatabase {
            directory,
            file,
            stamp,
            bytes,
            sha256,
            bound,
        };
        result.verify()?;
        Ok(result)
    }
}
