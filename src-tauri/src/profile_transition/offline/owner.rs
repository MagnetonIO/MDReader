//! Closed-owner capability. Exact cooperating writer locks, retained capture
//! producer barrier and root identity are held through import and selector CAS.
//! No PID absence, caller boolean or successful shutdown reply is proof.
use mdreader_core::SourceRoot;
use mdreader_profile::{Error, FrozenProfile, Result, RootBinding};
use rusqlite::{Connection, OpenFlags};
use std::{
    fs::File,
    path::{Path, PathBuf},
};
struct Lock {
    relative: PathBuf,
    file: File,
}
pub(super) struct ClosedOwner {
    root: SourceRoot,
    binding: RootBinding,
    locks: Vec<Lock>,
    capture: Option<mdreader_capture::FrozenSpool>,
    database: Option<Connection>,
    snapshot: super::closed_database::Snapshot,
    #[cfg(windows)]
    json: ClosedJson,
}
impl ClosedOwner {
    pub(super) fn acquire(path: &Path) -> Result<Self> {
        Self::acquire_inner(path, None)
    }
    pub(super) fn acquire_retaining(
        path: &Path,
        handoff: &mut super::shutdown_handoff::ShutdownHandoff,
    ) -> Result<Self> {
        Self::acquire_inner(path, Some(handoff))
    }
    fn acquire_inner(
        path: &Path,
        mut handoff: Option<&mut super::shutdown_handoff::ShutdownHandoff>,
    ) -> Result<Self> {
        let root = SourceRoot::capture(path.to_owned()).map_err(|_| Error::Conflict)?;
        // Exact native root/DACL policy is checked before writer dispatch.
        #[cfg(unix)]
        let binding = RootBinding::capture(path)?;
        #[cfg(windows)]
        let binding = RootBinding::capture_private(path)?;
        // Old clients lacking this lifetime lock cannot establish closure here.
        // Their inputs require the standalone offline exporter instead.
        if root
            .open_optional_file_relative(Path::new(".mdreader-json-owner.lock"))
            .map_err(|_| Error::Conflict)?
            .is_none()
            || root
                .open_optional_file_relative(Path::new("reader-v1/library.sqlite"))
                .map_err(|_| Error::Conflict)?
                .is_none()
        {
            return Err(Error::Unsupported);
        }
        let mut locks = Vec::new();
        for relative in [
            ".mdreader-json-owner.lock",
            "library.lock",
            "diagnostics/diagnostics.lock",
            "reader-v1/reader.lock",
            "reader-v1/native-provider-admissions-v1.lock",
            "reader-v1/integrations/settings/installer.lock",
        ] {
            let relative = Path::new(relative);
            // Missing optional subsystems have no writer to close. Capture the
            // actual existing parent before creating its cooperating lock.
            if !root
                .path
                .join(relative.parent().ok_or(Error::Invalid)?)
                .exists()
            {
                continue;
            }
            #[cfg(unix)]
            let file = root
                .open_lock_relative(relative)
                .map_err(|_| Error::Conflict)?;
            #[cfg(windows)]
            let file = private_lock(&root, relative)?;
            if fs2::FileExt::try_lock_exclusive(&file).is_err() {
                if relative != Path::new("reader-v1/reader.lock") {
                    return Err(Error::NotQuiescent);
                }
                // Dispatch only when the caller can retain the exact issued
                // attempt across errors. A stack-local attempt would abandon an
                // uncertain held lease and mint a different operation on retry.
                let retained = handoff.as_deref_mut().ok_or(Error::NotQuiescent)?;
                retained.close(&root, &file)?;
            }
            locks.push(Lock {
                relative: relative.to_owned(),
                file,
            });
        }
        let capture = if root.path.join("reader-v1/captures").exists() {
            Some(
                mdreader_capture::Spool::open(
                    root.path.join("reader-v1/captures"),
                    Default::default(),
                )
                .and_then(mdreader_capture::Spool::freeze)
                .map_err(|_| Error::NotQuiescent)?,
            )
        } else {
            None
        };
        // Never open the original via SQLite's WAL VFS. Even a readonly
        // connection can alter/delete original SHM stamps when it closes.
        let snapshot = super::closed_database::Snapshot::capture(
            &root,
            Path::new("reader-v1/library.sqlite"),
        )?;
        let database = Connection::open_with_flags(
            snapshot.directory.path.join("library.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| Error::Storage)?;
        database
            .busy_timeout(std::time::Duration::ZERO)
            .map_err(|_| Error::Storage)?;
        database
            .pragma_update(None, "trusted_schema", false)
            .map_err(|_| Error::Storage)?;
        database
            .pragma_update(None, "query_only", true)
            .map_err(|_| Error::Storage)?;
        database
            .query_row("SELECT count(*) FROM sqlite_schema", [], |row| {
                row.get::<_, u64>(0)
            })
            .map_err(|_| Error::Storage)?;
        #[cfg(windows)]
        let json = ClosedJson::capture(&root)?;
        let mut closed = Self {
            root,
            binding,
            locks,
            capture,
            database: Some(database),
            snapshot,
            #[cfg(windows)]
            json,
        };
        closed.revalidate()?;
        if let Some(handoff) = handoff {
            handoff.completed();
        }
        Ok(closed)
    }
    pub(super) fn store(&mut self) -> Result<mdreader_core::ReaderStore> {
        self.revalidate()?;
        self.snapshot.store()
    }
    pub(super) fn path(&self) -> &Path {
        &self.root.path
    }
    pub(super) fn verify_candidate(
        &mut self,
        expected: &mdreader_profile::lifecycle::CandidateReceipt,
    ) -> Result<()> {
        self.revalidate()?;
        if expected.candidate != self.binding {
            return Err(Error::Conflict);
        }
        let source = self.snapshot.backup_source()?;
        let connection = self.database.as_ref().ok_or(Error::NotQuiescent)?;
        let snapshot = &mut self.snapshot;
        mdreader_profile::candidate::verify_committed_closed_snapshot(
            connection,
            expected,
            &source,
            || snapshot.verify(),
        )?;
        self.revalidate()
    }
}
impl FrozenProfile for ClosedOwner {
    fn binding(&self) -> &RootBinding {
        &self.binding
    }
    fn backup_database(
        &mut self,
        target: &mut mdreader_profile::database_backup::DatabaseBackupTarget,
    ) -> Result<mdreader_profile::database_backup::BackupReceipt> {
        self.revalidate()?;
        let request = target.request();
        if request.source.root != self.binding {
            return Err(Error::Conflict);
        }
        let relative = request.source.relative.restore()?;
        #[cfg(windows)]
        if relative == Path::new("reader-json-v1.sqlite") {
            let receipt = self.json.backup(target)?;
            self.revalidate()?;
            return Ok(receipt);
        }
        if relative != Path::new("reader-v1/library.sqlite") {
            return Err(Error::Conflict);
        }
        let source = self.snapshot.backup_source()?;
        let connection = self.database.as_ref().ok_or(Error::NotQuiescent)?;
        let snapshot = &mut self.snapshot;
        let receipt = target.with_closed_snapshot(connection, source, || snapshot.verify())?;
        self.revalidate()?;
        Ok(receipt)
    }
    fn revalidate(&mut self) -> Result<()> {
        self.root.verify().map_err(|_| Error::Conflict)?;
        self.snapshot.verify()?;
        #[cfg(windows)]
        self.json.verify()?;
        for lock in &self.locks {
            let named = self
                .root
                .open_optional_file_relative(&lock.relative)
                .map_err(|_| Error::Conflict)?
                .ok_or(Error::Conflict)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let old = lock.file.metadata().map_err(|_| Error::Storage)?;
                let new = named.metadata().map_err(|_| Error::Storage)?;
                if new.nlink() != 1 || (old.dev(), old.ino()) != (new.dev(), new.ino()) {
                    return Err(Error::Conflict);
                }
            }
            #[cfg(windows)]
            if !mdreader_capture::same_open_file(&lock.file, &named).map_err(|_| Error::Conflict)? {
                return Err(Error::Conflict);
            }
        }
        if let Some(capture) = &self.capture {
            capture.revalidate().map_err(|_| Error::Conflict)?;
        }
        Ok(())
    }
    fn with_database(
        &mut self,
        relative: &Path,
        action: &mut dyn FnMut(&Connection) -> Result<()>,
    ) -> Result<()> {
        self.revalidate()?;
        #[cfg(windows)]
        if relative == Path::new("reader-json-v1.sqlite") {
            self.json.lend(action)?;
            return self.revalidate();
        }
        // Only the actual known reader DB is supported. Unknown legacy stores
        // remain blocked by inventory classification/standalone offline export.
        if relative != Path::new("reader-v1/library.sqlite") {
            return Err(Error::Unsupported);
        }
        let connection = self.database.as_ref().ok_or(Error::NotQuiescent)?;
        action(connection)?;
        self.revalidate()
    }
}
impl Drop for ClosedOwner {
    fn drop(&mut self) {
        // Explicitly release actual flock descriptors. No exported bytes are
        // proof that another process still holds these capabilities.
        // Drop SQLite before allowing any new original writer.
        drop(self.database.take());
        #[cfg(windows)]
        self.json.close();
        for lock in &self.locks {
            let _ = fs2::FileExt::unlock(&lock.file);
        }
    }
}

#[cfg(windows)]
#[path = "closed_json.rs"]
mod json;
#[cfg(windows)]
use json::ClosedJson;

#[cfg(windows)]
fn private_lock(root: &SourceRoot, relative: &Path) -> Result<File> {
    if root
        .open_optional_file_relative(relative)
        .map_err(|_| Error::Conflict)?
        .is_none()
    {
        let parent = relative.parent().ok_or(Error::Invalid)?;
        let name = relative.file_name().ok_or(Error::Invalid)?;
        let selected = mdreader_capture::SelectedDirectory::open(&root.path.join(parent))
            .map_err(|_| Error::Conflict)?;
        drop(
            selected
                .create_private_file(Path::new(name))
                .map_err(|_| Error::Conflict)?,
        );
        selected.verify().map_err(|_| Error::Conflict)?;
    }
    let file = root
        .open_lock_relative(relative)
        .map_err(|_| Error::Conflict)?;
    mdreader_capture::observe_object(&file, false, true).map_err(|_| Error::Conflict)?;
    root.verify().map_err(|_| Error::Conflict)?;
    Ok(file)
}
#[cfg(all(test, windows))]
#[path = "closed_windows_tests.rs"]
mod windows_tests;
