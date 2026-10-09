//! Native-owned private SQLite initialization, not generic source discovery.
//! Original broad/inherited artifacts are refused, never ACL-repaired or reset.
use super::*;
use mdreader_capture::{same_open_file, PrivateDirectory};
use std::fs::File;

struct Database {
    name: &'static str,
    files: Vec<File>,
}
pub(super) struct PrivateDatabases {
    databases: Vec<Database>,
    root: SourceRoot,
    private: PrivateDirectory,
}
impl PrivateDatabases {
    fn new(path: &Path) -> Result<Self> {
        let private = PrivateDirectory::create(path).map_err(|_| ReaderError::Unavailable)?;
        let root = SourceRoot::capture(path.to_owned())?;
        private.verify().map_err(|_| ReaderError::Conflict)?;
        Ok(Self {
            databases: vec![],
            root,
            private,
        })
    }
    fn prepare(&mut self, name: &'static str) -> Result<()> {
        self.verify()?;
        let main = self
            .private
            .database_file(Path::new(name), false)
            .map_err(|_| ReaderError::Unavailable)?;
        let mut sidecars = Vec::new();
        for suffix in ["-wal", "-shm", "-journal"] {
            let file = self
                .private
                .database_file(Path::new(&format!("{name}{suffix}")), false)
                .map_err(|_| ReaderError::Unavailable)?;
            sidecars.push(file);
        }
        let fresh = match &main {
            None => {
                if sidecars.iter().any(Option::is_some) {
                    return Err(ReaderError::Conflict);
                }
                // A lost durable DB must not be regenerated from its disposable
                // index or surviving service metadata.
                if name == "library.sqlite"
                    && self
                        .private
                        .database_file(Path::new("index.sqlite"), false)
                        .map_err(|_| ReaderError::Unavailable)?
                        .is_some()
                {
                    return Err(ReaderError::Conflict);
                }
                true
            }
            Some(file) if file.metadata()?.len() == 0 && sidecars.iter().all(Option::is_none) => {
                true
            }
            Some(file) => {
                if sidecars.iter().any(Option::is_none) {
                    return Err(ReaderError::Unavailable);
                }
                false
            }
        };
        let main = match main {
            Some(file) => file,
            None => self
                .private
                .create_database_file(Path::new(name))
                .map_err(|_| ReaderError::Unavailable)?,
        };
        let mut files = vec![main];
        for (suffix, existing) in ["-wal", "-shm", "-journal"].into_iter().zip(sidecars) {
            files.push(match existing {
                Some(file) => file,
                None if fresh => self
                    .private
                    .create_database_file(Path::new(&format!("{name}{suffix}")))
                    .map_err(|_| ReaderError::Unavailable)?,
                None => return Err(ReaderError::Conflict),
            });
        }
        // Flush actual writable handles, not directory/read-only descriptors.
        if fresh {
            for file in &files {
                file.sync_all()?;
            }
        }
        self.databases.push(Database { name, files });
        self.verify()
    }
    pub(super) fn verify(&self) -> Result<()> {
        self.root.verify()?;
        self.private.verify().map_err(|_| ReaderError::Conflict)?;
        for database in &self.databases {
            for (suffix, file) in ["", "-wal", "-shm", "-journal"]
                .into_iter()
                .zip(&database.files)
            {
                let named = self
                    .private
                    .database_file(Path::new(&format!("{}{suffix}", database.name)), false)
                    .map_err(|_| ReaderError::Conflict)?
                    .ok_or(ReaderError::Conflict)?;
                if !same_open_file(file, &named).map_err(|_| ReaderError::Conflict)? {
                    return Err(ReaderError::Conflict);
                }
            }
        }
        Ok(())
    }
}

fn open_database(path: &Path, schema: &str, index: bool) -> Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::default() | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    // Set the non-NeedSchema pragma before any schema/page read: hot rollback
    // recovery must zero its header instead of deleting a pinned journal.
    connection.pragma_update(None, "locking_mode", "EXCLUSIVE")?;
    // PERSIST permits NORMAL + completed read TX to release exclusive recovery.
    // Entering WAL while EXCLUSIVE would retain SQLite's no-SHM optimization
    // and prevent the index's independent readonly attachment from opening.
    let connection = initialize_database(connection, schema, index, DatabaseJournal::Persist)?;
    connection.pragma_update(None, "locking_mode", "NORMAL")?;
    connection.execute_batch("BEGIN; SELECT count(*) FROM sqlite_schema; COMMIT;")?;
    Ok(connection)
}

impl ReaderStore {
    /// Open a native-owned private Windows library. Keep protected DB/WAL/SHM/
    /// rollback objects pinned until SQLite closes. Existing incompatible ACLs
    /// or partial sidecar namespaces require explicit preservation, not repair.
    pub fn open_private(directory: &Path) -> Result<Self> {
        register_vectors()?;
        let mut files = PrivateDatabases::new(directory)?;
        files.prepare("library.sqlite")?;
        let (library, library_identity) = identity::open_library(directory, true, |path| {
            open_database(path, LIBRARY_SCHEMA, false)
        })?;
        projection::initialize(&library)?;
        crate::provider_reading_state::schema::initialize(&library)?;
        // Only admitted durable library versions permit derived-index creation.
        files.prepare("index.sqlite")?;
        let (index, index_identity) = identity::open_index(directory, true, |path| {
            open_database(path, INDEX_SCHEMA, true)
        })?;
        projection::attach_authority(&index, &directory.join("library.sqlite"))?;
        catalog::schema::initialize(&library, &index)?;
        identity::verify_connection(&library, library_identity.as_ref())?;
        identity::verify_connection(&index, index_identity.as_ref())?;
        identity::verify_attached(&index, library_identity.as_ref())?;
        parser_version::queue_stale(&library, &index)?;
        files.verify()?;
        Ok(Self {
            library_identity,
            index_identity,
            library,
            index,
            #[cfg(feature = "device-assets")]
            owner_session: Arc::new(()),
            private_databases: Some(files),
        })
    }
}

#[cfg(test)]
#[path = "windows_private_tests.rs"]
mod tests;
