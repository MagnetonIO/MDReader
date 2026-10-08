//! Private native service artifacts; this does not certify spool namespace durability.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);

/// Retains the local NTFS directory chain and verifies its protected current-user
/// DACL. Existing broad/inherited directories are refused, never silently repaired.
pub struct PrivateDirectory {
    chain: authority::Chain,
}
impl PrivateDirectory {
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
fn object_identity(file: &File) -> Result<WindowsFileIdentity> {
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
