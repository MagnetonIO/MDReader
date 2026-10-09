//! Exact standard-profile legacy application observations. A retained process
//! handle proves that instance exited; PID absence alone is never closure proof.
//! This boundary sends no signal and does not acquire database writer locks.
use crate::{Error, Result};
use mdreader_capture::observe_object;
use mdreader_core::SourceRoot;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};
mod process;
mod raw;
pub use raw::ClosedOriginal;

struct Binary {
    root: SourceRoot,
    relative: PathBuf,
    file: File,
    hash: blake3::Hash,
}
impl Binary {
    fn capture(path: &Path) -> Result<Self> {
        let root = SourceRoot::capture(path.parent().ok_or(Error::Invalid)?.to_owned())
            .map_err(|_| Error::Conflict)?;
        let relative = PathBuf::from(path.file_name().ok_or(Error::Invalid)?);
        if relative
            .to_str()
            .is_none_or(|n| !n.eq_ignore_ascii_case("MDReader.exe"))
        {
            return Err(Error::Unsupported);
        }
        let file = root
            .open_optional_file_relative(&relative)
            .map_err(|_| Error::Conflict)?
            .ok_or(Error::Conflict)?;
        let mut binary = Self {
            root,
            relative,
            file,
            hash: blake3::hash(&[]),
        };
        binary.hash = binary.digest()?;
        binary.verify()?;
        Ok(binary)
    }
    fn path(&self) -> PathBuf {
        self.root.path.join(&self.relative)
    }
    fn digest(&mut self) -> Result<blake3::Hash> {
        use std::io::{Seek, SeekFrom};
        let before = observe_object(&self.file, false, false).map_err(|_| Error::Conflict)?;
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|_| Error::Storage)?;
        let mut hash = blake3::Hasher::new();
        let mut count = 0u64;
        let mut buffer = [0; 65536];
        loop {
            let n = self.file.read(&mut buffer).map_err(|_| Error::Storage)?;
            if n == 0 {
                break;
            }
            count += n as u64;
            if count > 1024 * 1024 * 1024 {
                return Err(Error::Bounds);
            }
            hash.update(&buffer[..n]);
        }
        if count == 0
            || observe_object(&self.file, false, false).map_err(|_| Error::Conflict)? != before
        {
            return Err(Error::Conflict);
        }
        Ok(hash.finalize())
    }
    fn verify(&mut self) -> Result<()> {
        self.root.verify().map_err(|_| Error::Conflict)?;
        let named = self
            .root
            .open_optional_file_relative(&self.relative)
            .map_err(|_| Error::Conflict)?
            .ok_or(Error::Conflict)?;
        if observe_object(&named, false, false).map_err(|_| Error::Conflict)?
            != observe_object(&self.file, false, false).map_err(|_| Error::Conflict)?
            || self.digest()? != self.hash
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}
/// Native-owned lifetime proof. The caller must additionally acquire every
/// original JSON/database/service writer fence before copying any live profile.
pub struct LegacyApplicationFence {
    profile: SourceRoot,
    binary: Binary,
    processes: Vec<process::Process>,
}
impl LegacyApplicationFence {
    /// Select the known installed executable while its standard-profile GUI is
    /// still running. Custom profiles and a current-process binary are refused.
    pub fn capture(profile: &SourceRoot, installed_executable: &Path) -> Result<Self> {
        let appdata = std::env::var_os("APPDATA").ok_or(Error::Unsupported)?;
        let standard = SourceRoot::capture(PathBuf::from(appdata).join("dev.mdreader.app"))
            .map_err(|_| Error::Unsupported)?;
        if profile != &standard {
            return Err(Error::Unsupported);
        }
        let mut binary = Binary::capture(installed_executable)?;
        let current = std::env::current_exe().map_err(|_| Error::Storage)?;
        if current.canonicalize().map_err(|_| Error::Conflict)?
            == binary.path().canonicalize().map_err(|_| Error::Conflict)?
        {
            return Err(Error::NotQuiescent);
        }
        let processes = process::matching(&binary.path(), &profile.path)?;
        if !processes.iter().any(|p| p.gui()) {
            return Err(Error::NotQuiescent);
        }
        binary.verify()?;
        profile.verify().map_err(|_| Error::Conflict)?;
        Ok(Self {
            profile: profile.clone(),
            binary,
            processes,
        })
    }
    /// Poll original handles and rescan exact executable instances. New owners
    /// cause refusal; they are never adopted as if the previously observed GUI.
    pub fn verify_closed(&mut self) -> Result<()> {
        self.profile.verify().map_err(|_| Error::Conflict)?;
        self.binary.verify()?;
        for process in &self.processes {
            process.closed()?;
        }
        if !process::matching(&self.binary.path(), &self.profile.path)?.is_empty() {
            return Err(Error::NotQuiescent);
        }
        self.binary.verify()?;
        self.profile.verify().map_err(|_| Error::Conflict)
    }
}
