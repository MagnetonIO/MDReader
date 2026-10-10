//! Real Windows owner connection, private namespace and reopen boundary.
use crate::*;
use rusqlite::Connection;
use std::path::Path;

struct BorrowedOwner {
    binding: RootBinding,
    connection: Connection,
}
impl FreezeOwner for BorrowedOwner {
    fn with_frozen<T>(
        &mut self,
        expected: &RootBinding,
        action: impl FnOnce(&mut dyn FrozenProfile) -> Result<T>,
    ) -> Result<T> {
        if expected != &self.binding || !self.connection.is_autocommit() {
            return Err(Error::NotQuiescent);
        }
        action(&mut Borrowed(self))
    }
}
struct Borrowed<'a>(&'a BorrowedOwner);
impl FrozenProfile for Borrowed<'_> {
    fn binding(&self) -> &RootBinding {
        &self.0.binding
    }
    fn revalidate(&mut self) -> Result<()> {
        if RootBinding::capture_private(&self.0.binding.locator.restore()?)? != self.0.binding
            || !self.0.connection.is_autocommit()
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    fn with_database(
        &mut self,
        relative: &Path,
        action: &mut dyn FnMut(&Connection) -> Result<()>,
    ) -> Result<()> {
        self.revalidate()?;
        if relative != Path::new("library.sqlite") {
            return Err(Error::Invalid);
        }
        action(&self.0.connection)?;
        self.revalidate()
    }
}

#[test]
fn windows_snapshot_uses_actual_wal_owner_keeps_originals_and_reopens_private_copy() {
    let temporary = tempfile::tempdir().unwrap();
    let source_path = temporary.path().join("original");
    let output_path = temporary.path().join("output");
    let source = mdreader_capture::PrivateDirectory::create(&source_path).unwrap();
    mdreader_capture::PrivateDirectory::create(&output_path).unwrap();
    drop(
        source
            .create_database_file(Path::new("library.sqlite"))
            .unwrap(),
    );
    let connection = Connection::open(source_path.join("library.sqlite")).unwrap();
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .unwrap();
    connection.execute_batch("CREATE TABLE originals(id INTEGER PRIMARY KEY,body BLOB NOT NULL); INSERT INTO originals VALUES(1,X'73656c6563746564206f726967696e616c');").unwrap();
    let raw = b"unknown original bytes\0\xff";
    let mut raw_file = source
        .create_database_file(Path::new("unknown.bin"))
        .unwrap();
    std::io::Write::write_all(&mut raw_file, raw).unwrap();
    raw_file.sync_all().unwrap();
    drop(raw_file);
    let limits = Limits::default();
    let mut choices = vec![];
    for entry in std::fs::read_dir(&source_path).unwrap() {
        let name = entry.unwrap().file_name();
        let disposition = if name == "library.sqlite" {
            Disposition::Sqlite
        } else if name == "unknown.bin" {
            Disposition::Raw
        } else {
            Disposition::Excluded(Exclusion::SqliteRecoverySidecar)
        };
        choices.push(InventorySelection {
            relative: name.into(),
            disposition,
        });
    }
    let inventory = Inventory::capture(&source_path, &choices, &limits).unwrap();
    let mut owner = BorrowedOwner {
        binding: inventory.binding().clone(),
        connection,
    };
    let copy = stage(&mut owner, inventory, &output_path.join("copy"), &limits).unwrap();
    let identity = copy.identity().clone();
    let database = copy
        .manifest()
        .entries
        .iter()
        .find(|entry| entry.relative.restore().unwrap() == Path::new("library.sqlite"))
        .unwrap()
        .member
        .clone()
        .unwrap();
    let copied = Connection::open_with_flags(
        output_path.join("copy").join(database),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let body: Vec<u8> = copied
        .query_row("SELECT body FROM originals WHERE id=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(body, b"selected original");
    assert_eq!(
        owner
            .connection
            .query_row("SELECT body FROM originals WHERE id=1", [], |r| r
                .get::<_, Vec<u8>>(0))
            .unwrap(),
        body
    );
    assert_eq!(std::fs::read(source_path.join("unknown.bin")).unwrap(), raw);
    assert_eq!(
        source.identity().unwrap(),
        mdreader_capture::PrivateDirectory::open(&source_path)
            .unwrap()
            .identity()
            .unwrap()
    );
    drop(copied);
    drop(copy);
    verify(&output_path.join("copy"), &identity, &limits).unwrap();
    let journal =
        Connection::open(output_path.join(".reader-profile-publications-v1.sqlite")).unwrap();
    assert_eq!(
        journal
            .query_row(
                "SELECT phase FROM publications WHERE destination='copy'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "published"
    );
}
