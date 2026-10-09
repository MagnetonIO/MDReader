//! Closed Windows renderer state. SQLite only opens a private recovery copy;
//! the original JSON writer lock and every raw journal remain with ClosedOwner.
use super::*;

pub(super) struct ClosedJson {
    // Connection must close before its private directory capability.
    connection: Option<Connection>,
    snapshot: super::super::closed_database::Snapshot,
}
impl ClosedJson {
    pub(super) fn capture(root: &SourceRoot) -> Result<Self> {
        let snapshot = super::super::closed_database::Snapshot::capture(
            root,
            Path::new("reader-json-v1.sqlite"),
        )?;
        let connection = Connection::open_with_flags(
            snapshot.directory.path.join("library.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| Error::Storage)?;
        connection
            .pragma_update(None, "trusted_schema", false)
            .map_err(|_| Error::Storage)?;
        connection
            .pragma_update(None, "query_only", true)
            .map_err(|_| Error::Storage)?;
        let mut closed = Self {
            connection: Some(connection),
            snapshot,
        };
        closed.verify()?;
        Ok(closed)
    }
    pub(super) fn verify(&mut self) -> Result<()> {
        self.snapshot.verify()?;
        super::super::super::persistence::verify_closed_windows_database(
            self.connection.as_ref().ok_or(Error::NotQuiescent)?,
        )?;
        self.snapshot.verify()
    }
    pub(super) fn backup(
        &mut self,
        target: &mut mdreader_profile::database_backup::DatabaseBackupTarget,
    ) -> Result<mdreader_profile::database_backup::BackupReceipt> {
        self.verify()?;
        let source = self.snapshot.backup_source()?;
        let connection = self.connection.as_ref().ok_or(Error::NotQuiescent)?;
        let snapshot = &mut self.snapshot;
        let receipt = target.with_closed_snapshot(connection, source, || snapshot.verify())?;
        self.verify()?;
        Ok(receipt)
    }
    pub(super) fn lend(&mut self, action: &mut dyn FnMut(&Connection) -> Result<()>) -> Result<()> {
        self.verify()?;
        action(self.connection.as_ref().ok_or(Error::NotQuiescent)?)?;
        self.verify()
    }
    pub(super) fn close(&mut self) {
        drop(self.connection.take());
    }
}
