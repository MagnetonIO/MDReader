//! Closed SQLite inputs are raw evidence. SQLite only opens private recovery
//! copies, so readonly WAL access cannot change an original SHM or its stamps.
use super::*;
use mdreader_core::{
    source_root::file_stamp::{single_link_stamp, SourceFileStamp},
    SourceRoot,
};
use rusqlite::{
    backup::{Backup, StepResult},
    Connection, OpenFlags,
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

const MAX_BYTES: u64 = 256 * 1024 * 1024;
struct Member {
    relative: PathBuf,
    file: Option<(File, SourceFileStamp, blake3::Hash)>,
}
pub(super) struct Fence {
    root: SourceRoot,
    members: Vec<Member>,
}
fn hash(file: &mut File) -> Result<blake3::Hash> {
    file.seek(SeekFrom::Start(0)).map_err(|_| Error::Storage)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0; 65536];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buffer).map_err(|_| Error::Storage)?;
        if n == 0 {
            break;
        }
        total = total.checked_add(n as u64).ok_or(Error::Bounds)?;
        if total > MAX_BYTES {
            return Err(Error::Bounds);
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize())
}
impl Fence {
    pub(super) fn verify(&mut self) -> Result<()> {
        self.root.verify().map_err(|_| Error::Conflict)?;
        for member in &mut self.members {
            let named = self
                .root
                .open_optional_file_relative(&member.relative)
                .map_err(|_| Error::Conflict)?;
            match (&mut member.file, named) {
                (None, None) => {}
                (Some((file, stamp, digest)), Some(named)) => {
                    if single_link_stamp(&named).map_err(|_| Error::Conflict)? != *stamp
                        || single_link_stamp(file).map_err(|_| Error::Conflict)? != *stamp
                        || hash(file)? != *digest
                        || single_link_stamp(file).map_err(|_| Error::Conflict)? != *stamp
                    {
                        return Err(Error::Conflict);
                    }
                }
                _ => return Err(Error::Conflict),
            }
        }
        self.root.verify().map_err(|_| Error::Conflict)
    }
}
/// Caller holds actual writer guards. Retains exact present/absent membership,
/// descriptors/stamps/digests before and after private SQLite recovery.
pub(super) fn recover_to(
    source: &SourceRoot,
    relative: &Path,
    working: &SourceRoot,
    name: &str,
) -> Result<Fence> {
    let base = relative.to_str().ok_or(Error::Unsupported)?;
    let working_name = format!("{name}.working");
    let mut members = Vec::new();
    let mut total = 0u64;
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let relative = PathBuf::from(format!("{base}{suffix}"));
        let file = if let Some(mut file) = source
            .open_optional_file_relative(&relative)
            .map_err(|_| Error::Conflict)?
        {
            let stamp = single_link_stamp(&file).map_err(|_| Error::Conflict)?;
            let len = file.metadata().map_err(|_| Error::Storage)?.len();
            total = total.checked_add(len).ok_or(Error::Bounds)?;
            if total > MAX_BYTES {
                return Err(Error::Bounds);
            }
            let digest = hash(&mut file)?;
            file.seek(SeekFrom::Start(0)).map_err(|_| Error::Storage)?;
            let mut out = working
                .create_new_relative(Path::new(&format!("{working_name}{suffix}")))
                .map_err(|_| Error::Storage)?;
            let copied = std::io::copy(&mut (&mut file).take(len + 1), &mut out)
                .map_err(|_| Error::Storage)?;
            out.flush()
                .and_then(|()| out.sync_all())
                .map_err(|_| Error::Storage)?;
            if copied != len || single_link_stamp(&file).map_err(|_| Error::Conflict)? != stamp {
                return Err(Error::Conflict);
            }
            Some((file, stamp, digest))
        } else {
            if suffix.is_empty() {
                return Err(Error::Integrity);
            }
            None
        };
        members.push(Member { relative, file });
    }
    let mut fence = Fence {
        root: source.clone(),
        members,
    };
    fence.verify()?;
    let input = Connection::open_with_flags(
        working.path.join(&working_name),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| Error::Storage)?;
    input
        .execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")
        .map_err(|_| Error::Storage)?;
    let pages: u64 = input
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .map_err(|_| Error::Integrity)?;
    let page_bytes: u64 = input
        .query_row("PRAGMA page_size", [], |r| r.get(0))
        .map_err(|_| Error::Integrity)?;
    if pages.checked_mul(page_bytes).ok_or(Error::Bounds)? > MAX_BYTES {
        return Err(Error::Bounds);
    }
    working
        .create_new_relative(Path::new(name))
        .map_err(|_| Error::Storage)?;
    let mut output = Connection::open(working.path.join(name)).map_err(|_| Error::Storage)?;
    let backup = Backup::new(&input, &mut output).map_err(|_| Error::Storage)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match backup.step(256).map_err(|_| Error::Storage)? {
            StepResult::Done => break,
            StepResult::More if Instant::now() < deadline => {}
            _ => return Err(Error::NotQuiescent),
        }
    }
    drop(backup);
    output.close().map_err(|_| Error::Storage)?;
    drop(input);
    working
        .open_optional_file_relative(Path::new(name))
        .map_err(|_| Error::Storage)?
        .ok_or(Error::Storage)?
        .sync_all()
        .map_err(|_| Error::Storage)?;
    fence.verify()?;
    Ok(fence)
}
pub(super) struct Snapshot {
    pub directory: SourceRoot,
    fence: Fence,
    _temporary: tempfile::TempDir,
}
impl Snapshot {
    pub(super) fn backup_source(&self) -> Result<mdreader_profile::database_backup::BackupSource> {
        use std::os::unix::fs::MetadataExt;
        let mdreader_core::SourceRootIdentity::Unix { device, inode } = self.directory.identity
        else {
            return Err(Error::Unsupported);
        };
        self.directory.verify().map_err(|_| Error::Conflict)?;
        let file = self
            .directory
            .open_optional_file_relative(Path::new("library.sqlite"))
            .map_err(|_| Error::Conflict)?
            .ok_or(Error::Conflict)?;
        single_link_stamp(&file).map_err(|_| Error::Conflict)?;
        let m = file.metadata().map_err(|_| Error::Storage)?;
        Ok(mdreader_profile::database_backup::BackupSource {
            root: mdreader_profile::RootBinding {
                windows_file_id: None,
                locator: mdreader_profile::RootLocator::capture(&self.directory.path)?,
                device,
                inode,
            },
            relative: mdreader_profile::RelativeLocator::capture(Path::new("library.sqlite"))?,
            stamp: (
                m.dev(),
                m.ino(),
                m.len(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
            ),
        })
    }
    pub(super) fn capture(source: &SourceRoot, relative: &Path) -> Result<Self> {
        let parent = std::env::temp_dir()
            .canonicalize()
            .map_err(|_| Error::Storage)?;
        let temporary = tempfile::Builder::new()
            .prefix("MDReader-closed-db-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(parent)
            .map_err(|_| Error::Storage)?;
        let directory =
            SourceRoot::capture(temporary.path().to_owned()).map_err(|_| Error::Conflict)?;
        let fence = recover_to(source, relative, &directory, "library.sqlite")?;
        Ok(Self {
            directory,
            fence,
            _temporary: temporary,
        })
    }
    pub(super) fn verify(&mut self) -> Result<()> {
        self.fence.verify()
    }
    pub(super) fn store(&mut self) -> Result<mdreader_core::ReaderStore> {
        self.verify()?;
        let store = mdreader_core::ReaderStore::open_durable_readonly(&self.directory.path)
            .map_err(|_| Error::Integrity)?;
        self.verify()?;
        Ok(store)
    }
}
