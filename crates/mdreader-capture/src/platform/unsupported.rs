use crate::*;

pub(crate) struct DirectoryAuthority;
impl DirectoryAuthority {
    pub(crate) fn capture(_: &Path) -> Result<Self> {
        Err(CaptureError::UnsupportedPlatform)
    }
    pub(crate) fn verify(&self, _: &Path) -> Result<()> {
        Err(CaptureError::UnsupportedPlatform)
    }
}
pub(crate) fn private_directory(_: &Path) -> Result<()> {
    Err(CaptureError::UnsupportedPlatform)
}
pub(crate) fn ensure_regular_or_missing(_: &Path) -> Result<()> {
    Err(CaptureError::UnsupportedPlatform)
}
pub(crate) fn regular_exists(_: &Path) -> Result<bool> {
    Err(CaptureError::UnsupportedPlatform)
}
pub(crate) fn private_open(_: &Path, _: bool) -> Result<File> {
    Err(CaptureError::UnsupportedPlatform)
}
pub(crate) fn read_bounded(_: &Path, _: usize) -> Result<Vec<u8>> {
    Err(CaptureError::UnsupportedPlatform)
}
pub(crate) fn sync_directory(_: &Path) -> Result<()> {
    Err(CaptureError::UnsupportedPlatform)
}
pub(crate) fn replace(_: &File, _: &Path, _: &Path) -> Result<()> {
    Err(CaptureError::UnsupportedPlatform)
}
pub(crate) fn confirm_file(_: &Path) -> Result<()> {
    Err(CaptureError::UnsupportedPlatform)
}
pub(crate) fn lock_would_block(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
}
