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
