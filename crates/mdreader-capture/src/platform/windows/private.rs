//! Private native service artifacts; this does not certify spool namespace durability.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);

/// Retains the local NTFS directory chain and verifies its protected current-user
/// DACL. Existing broad/inherited directories are refused, never silently repaired.
pub struct PrivateDirectory {
    pub(super) chain: authority::Chain,
}
impl PrivateDirectory {
    /// Remove one explicitly selected unpublished projection through its exact
    /// private, single-link NTFS handle. Competing writers/deletion are refused.
    /// This mutates a rebuildable projection; SQLite owns the durable drop fence.
    #[allow(unsafe_code)]
    pub fn remove_projection_file(&self, name: &Path, maximum: u64) -> Result<()> {
        let path = self.child(name)?;
        let wide = authority::wide(&path)?;
        // SAFETY: terminated live path; DELETE handle denies write/delete sharing.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                DELETE | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
                FILE_SHARE_READ,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return self.verify();
            }
            return Err(error.into());
        }
        // SAFETY: successful native open transfers this handle to one File owner.
        let file = unsafe { File::from_raw_handle(handle.cast()) };
        authority::inspect(&file, false, true)?;
        authority::ntfs(&file)?;
        if file.metadata()?.len() > maximum {
            return Err(CaptureError::UnsafePath);
        }
        self.verify()?;
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: live DELETE-capable handle and matching bounded ABI payload.
        if unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle().cast(),
                FileDispositionInfo,
                ptr::addr_of!(disposition).cast(),
                std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        drop(file);
        self.verify()
    }
    /// Own one regular private projection file for a bounded write/flush. The
    /// handle denies competing writers and deletion until publication finishes.
    /// Missing files are created only when explicitly requested.
    pub fn projection_file(&self, name: &Path, create: bool) -> Result<Option<File>> {
        let path = self.child(name)?;
        let file = match open_file(&path, OPEN_EXISTING, true) {
            Ok(file) => Some(file),
            Err(CaptureError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                if create {
                    Some(open_file(&path, CREATE_NEW, true)?)
                } else {
                    None
                }
            }
            Err(error) => return Err(error),
        };
        self.verify()?;
        Ok(file)
    }
    /// Create a new private SQLite-compatible file, never adopt an existing name.
    pub fn create_database_file(&self, name: &Path) -> Result<File> {
        let path = self.child(name)?;
        let file = open_file_with_lock(&path, CREATE_NEW, true, true)?;
        self.verify()?;
        Ok(file)
    }
    /// Atomically create an absent private child directory without repairing or
    /// adopting an existing namespace. All ancestor handles remain pinned.
    pub fn create_child(&self, name: &Path) -> Result<Self> {
        let path = self.child(name)?;
        super::directory_publication::create(&path)?;
        self.verify()?;
        Self::open(&path)
    }
    pub fn identity(&self) -> Result<WindowsFileIdentity> {
        self.verify()?;
        object_identity(&self.chain.handles.last().ok_or(CaptureError::UnsafePath)?.1)
    }
    /// Consume a completed private stage and rename it to an absent sibling.
    /// This is an identity-preserving atomic rename, not a durability receipt.
    /// All stage child handles must already be closed; ancestor pins stay held.
    pub fn publish_sibling(self, name: &Path) -> Result<Self> {
        super::directory_publication::publish(self, name)
    }
    pub fn create(path: &Path) -> Result<Self> {
        Ok(Self {
            chain: authority::Chain::open(path, true, true)?,
        })
    }
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            chain: authority::Chain::open(path, false, true)?,
        })
    }
    pub fn verify(&self) -> Result<()> {
        self.chain.verify()?;
        security::verify(&self.chain.handles.last().ok_or(CaptureError::UnsafePath)?.1)
    }
    fn child(&self, name: &Path) -> Result<PathBuf> {
        if name.components().count() != 1
            || !matches!(name.components().next(), Some(Component::Normal(_)))
        {
            return Err(CaptureError::UnsafePath);
        }
        let path = self
            .chain
            .handles
            .last()
            .ok_or(CaptureError::UnsafePath)?
            .0
            .join(name);
        validate_leaf(&path)?;
        self.verify()?;
        Ok(path)
    }
    /// An actual lock object permitting competing openers to reach the byte lock.
    pub fn open_lock(&self, name: &Path) -> Result<File> {
        let path = self.child(name)?;
        let file = open_file_with_lock(&path, OPEN_ALWAYS, true, true)?;
        self.verify()?;
        Ok(file)
    }
    /// Pin a protected SQLite file while SQLite owns its separate descriptors.
    /// `create=false` does not create missing artifacts during namespace admission.
    pub fn database_file(&self, name: &Path, create: bool) -> Result<Option<File>> {
        let path = self.child(name)?;
        let file = if create {
            Some(pin_database(&path)?)
        } else {
            pin_existing_database(&path)?
        };
        self.verify()?;
        Ok(file)
    }
    pub fn read(&self, name: &Path, bound: usize) -> Result<Vec<u8>> {
        let bytes = read_bounded(&self.child(name)?, bound)?;
        self.verify()?;
        Ok(bytes)
    }
    /// Atomic publication of ephemeral discovery metadata, with exact readback.
    /// File flushing is real; callers must not use this as a durable queue receipt.
    pub fn publish(&self, name: &Path, bytes: &[u8]) -> Result<()> {
        if bytes.len() > 1024 * 1024 {
            return Err(CaptureError::TooLarge);
        }
        let destination = self.child(name)?;
        let temporary = self.child(Path::new(&format!(
            ".service-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))?;
        let mut file = private_open(&temporary, true)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        replace(&file, &temporary, &destination)?;
        drop(file);
        if self.read(name, 1024 * 1024)? != bytes {
            return Err(CaptureError::UnsafePath);
        }
        self.verify()
    }
}
/// Exact opened object identity, without opening or closing a SQLite database.
pub fn same_open_file(a: &File, b: &File) -> Result<bool> {
    authority::inspect(a, false, true)?;
    authority::inspect(b, false, true)?;
    Ok(authority::identity(a)? == authority::identity(b)?)
}

/// Full local NTFS object identity. Never truncate the 128-bit file identifier.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsFileIdentity {
    volume: u64,
    file_id: [u8; 16],
}
/// Retains every component of an explicitly selected local directory without
/// requiring a private DACL on user-owned project folders.
pub struct SelectedDirectory {
    chain: authority::Chain,
}
impl SelectedDirectory {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            chain: authority::Chain::open(path, false, false)?,
        })
    }
    pub fn verify(&self) -> Result<()> {
        self.chain.verify()
    }
    /// Create one absent, protected child under an explicitly selected parent.
    /// Existing folders are never adopted or repaired, even on name collision.
    pub fn create_private_child(&self, name: &Path) -> Result<PrivateDirectory> {
        if name.components().count() != 1
            || !matches!(name.components().next(), Some(Component::Normal(_)))
        {
            return Err(CaptureError::UnsafePath);
        }
        self.verify()?;
        let path = self
            .chain
            .handles
            .last()
            .ok_or(CaptureError::UnsafePath)?
            .0
            .join(name);
        validate_leaf(&path)?;
        super::directory_publication::create(&path)?;
        let child = PrivateDirectory::open(&path)?;
        self.verify()?;
        Ok(child)
    }
    /// Create an empty private file atomically, even in a user project whose
    /// directory ACL is broader. Existing entries are never repaired or truncated.
    pub fn create_private_file(&self, name: &Path) -> Result<File> {
        if name.components().count() != 1
            || !matches!(name.components().next(), Some(Component::Normal(_)))
        {
            return Err(CaptureError::UnsafePath);
        }
        self.verify()?;
        let parent = &self.chain.handles.last().ok_or(CaptureError::UnsafePath)?.0;
        let file = open_file_policy(&parent.join(name), CREATE_NEW, true, false, false)?;
        self.verify()?;
        Ok(file)
    }
    pub fn identity(&self) -> Result<WindowsFileIdentity> {
        self.verify()?;
        let file = &self.chain.handles.last().ok_or(CaptureError::UnsafePath)?.1;
        object_identity(file)
    }
}
pub(super) fn object_identity(file: &File) -> Result<WindowsFileIdentity> {
    let identity = authority::identity(file)?;
    Ok(WindowsFileIdentity {
        volume: identity.0,
        file_id: identity.1,
    })
}
/// Regular, single-link, non-reparse NTFS file identity for native admission.
pub fn selected_file_identity(file: &File) -> Result<WindowsFileIdentity> {
    authority::inspect(file, false, false)?;
    authority::ntfs(file)?;
    object_identity(file)
}
