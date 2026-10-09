//! Descriptor-relative bounded reads and durable JSON replacement. The owner
//! serializes cooperating writers; namespace replacement is a typed refusal.
use super::types::{PersistenceError as Error, Result, MAX_JSON_BYTES};
use mdreader_core::SourceRoot;
use same_file::Handle;
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const LOCK: &str = ".mdreader-json-owner.lock";
pub(super) struct JsonLifecycle {
    file: File,
}
impl JsonLifecycle {
    pub(super) fn capture(root: &SourceRoot) -> Result<Self> {
        root.verify().map_err(|_| Error::Conflict)?;
        let file = root
            .open_lock_relative(Path::new(LOCK))
            .map_err(|_| Error::Storage)?;
        let metadata = file.metadata().map_err(|_| Error::Storage)?;
        if !metadata.is_file() {
            return Err(Error::Conflict);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(Error::Conflict);
            }
        }
        fs2::FileExt::try_lock_exclusive(&file).map_err(|_| Error::Unavailable)?;
        let lifecycle = Self { file };
        lifecycle.verify(root)?;
        Ok(lifecycle)
    }
    pub(super) fn verify(&self, root: &SourceRoot) -> Result<()> {
        root.verify().map_err(|_| Error::Conflict)?;
        let observed = root
            .open_optional_file_relative(Path::new(LOCK))
            .map_err(|_| Error::Conflict)?
            .ok_or(Error::Conflict)?;
        if !same(&self.file, &observed)? {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}
impl Drop for JsonLifecycle {
    fn drop(&mut self) {
        // Do not let fork/duplicated descriptors extend the completed owner.
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

pub(super) struct Original {
    pub bytes: Vec<u8>,
    file: File,
    stamp: Stamp,
}

#[derive(PartialEq, Eq)]
struct Stamp {
    bytes: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64),
}
impl Stamp {
    fn read(file: &File) -> Result<Self> {
        let metadata = file.metadata().map_err(|_| Error::Storage)?;
        if !metadata.is_file() || metadata.len() > MAX_JSON_BYTES as u64 {
            return Err(Error::TooLarge);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(Error::Conflict);
            }
        }
        Ok(Self {
            bytes: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            changed: {
                use std::os::unix::fs::MetadataExt;
                (metadata.ctime(), metadata.ctime_nsec())
            },
        })
    }
}
fn same(left: &File, right: &File) -> Result<bool> {
    let left = Handle::from_file(left.try_clone().map_err(|_| Error::Storage)?)
        .map_err(|_| Error::Storage)?;
    let right = Handle::from_file(right.try_clone().map_err(|_| Error::Storage)?)
        .map_err(|_| Error::Storage)?;
    Ok(left == right)
}

pub(super) fn read(
    root: &SourceRoot,
    _lifecycle: &JsonLifecycle,
    path: &Path,
) -> Result<Option<Original>> {
    root.verify().map_err(|_| Error::Conflict)?;
    let Some(mut file) = root
        .open_optional_file_relative(path)
        .map_err(|_| Error::Conflict)?
    else {
        return Ok(None);
    };
    let stamp = Stamp::read(&file)?;
    let mut bytes = Vec::with_capacity(stamp.bytes as usize);
    (&mut file)
        .take(MAX_JSON_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Storage)?;
    if bytes.len() > MAX_JSON_BYTES {
        return Err(Error::TooLarge);
    }
    let original = Original { bytes, file, stamp };
    verify(root, _lifecycle, path, Some(&original))?;
    Ok(Some(original))
}
pub(super) fn verify(
    root: &SourceRoot,
    _lifecycle: &JsonLifecycle,
    path: &Path,
    original: Option<&Original>,
) -> Result<()> {
    root.verify().map_err(|_| Error::Conflict)?;
    let observed = root
        .open_optional_file_relative(path)
        .map_err(|_| Error::Conflict)?;
    match (original, observed) {
        (None, None) => Ok(()),
        (Some(original), Some(observed))
            if same(&original.file, &observed)?
                && original.stamp == Stamp::read(&original.file)?
                && original.stamp == Stamp::read(&observed)? =>
        {
            Ok(())
        }
        _ => Err(Error::Conflict),
    }
}

pub(super) fn replace(
    root: &SourceRoot,
    lifecycle: &JsonLifecycle,
    path: &Path,
    original: Option<&Original>,
    bytes: &[u8],
) -> Result<()> {
    if bytes.len() > MAX_JSON_BYTES {
        return Err(Error::TooLarge);
    }
    let relative = PathBuf::from(format!(".mdreader-json-{}.tmp", Uuid::new_v4()));
    let file = root
        .create_new_relative(&relative)
        .map_err(|_| Error::Storage)?;
    let mut temporary = Temporary {
        root,
        relative,
        file,
        published: false,
    };
    temporary
        .file
        .write_all(bytes)
        .map_err(|_| Error::Storage)?;
    temporary.file.sync_all().map_err(|_| Error::Storage)?;
    verify(root, lifecycle, path, original)?;
    // This is the publication point. Any subsequent error is uncertain, never
    // an instruction to blindly retry or restore an earlier private snapshot.
    root.rename_relative(&temporary.relative, path)
        .map_err(|_| Error::Storage)?;
    temporary.published = true;
    root.try_clone_directory_handle()
        .map_err(|_| Error::PublicationUncertain)?
        .sync_all()
        .map_err(|_| Error::PublicationUncertain)?;
    root.verify().map_err(|_| Error::PublicationUncertain)?;
    let target = root
        .open_optional_file_relative(path)
        .map_err(|_| Error::PublicationUncertain)?
        .ok_or(Error::PublicationUncertain)?;
    if !same(&temporary.file, &target).map_err(|_| Error::PublicationUncertain)? {
        return Err(Error::PublicationUncertain);
    }
    Ok(())
}
struct Temporary<'a> {
    root: &'a SourceRoot,
    relative: PathBuf,
    file: File,
    published: bool,
}
impl Drop for Temporary<'_> {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        if let Ok(Some(observed)) = self.root.open_optional_file_relative(&self.relative) {
            if same(&self.file, &observed).unwrap_or(false) {
                let _ = self.root.remove_file_relative(&self.relative);
            }
        }
    }
}
