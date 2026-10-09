//! Native-only launch identity retained before the original application exits.
//! A new reader executable must never be used to reopen an old CatDB profile.
use super::{Error, Result};
use mdreader_core::SourceRoot;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone)]
struct Member {
    relative: PathBuf,
    stamp: [u64; 8],
    digest: blake3::Hash,
}
fn stamp(file: &File) -> Result<[u64; 8]> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata().map_err(|_| Error::Storage)?;
    if !m.is_file() || m.nlink() != 1 {
        return Err(Error::Conflict);
    }
    Ok([
        m.dev(),
        m.ino(),
        m.len(),
        m.mode() as u64,
        m.mtime() as u64,
        m.mtime_nsec() as u64,
        m.ctime() as u64,
        m.ctime_nsec() as u64,
    ])
}
fn member(root: &SourceRoot, relative: PathBuf) -> Result<Member> {
    let mut file = root
        .open_optional_file_relative(&relative)
        .map_err(|_| Error::Conflict)?
        .ok_or(Error::Conflict)?;
    let before = stamp(&file)?;
    if before[2] > 1024 * 1024 * 1024 {
        return Err(Error::Bounds);
    }
    let mut hasher = blake3::Hasher::new();
    let mut bytes = 0u64;
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(|_| Error::Storage)?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        if bytes > before[2] {
            return Err(Error::Conflict);
        }
        hasher.update(&buffer[..n]);
    }
    let named = root
        .open_optional_file_relative(&relative)
        .map_err(|_| Error::Conflict)?
        .ok_or(Error::Conflict)?;
    if bytes != before[2] || stamp(&file)? != before || stamp(&named)? != before {
        return Err(Error::Conflict);
    }
    root.verify().map_err(|_| Error::Conflict)?;
    Ok(Member {
        relative,
        stamp: before,
        digest: hasher.finalize(),
    })
}
#[derive(Clone)]
pub(super) struct LegacyLaunch {
    root: SourceRoot,
    members: Vec<Member>,
}
impl LegacyLaunch {
    pub(super) fn capture(bundle: PathBuf, executable: PathBuf) -> Result<Self> {
        let root = SourceRoot::capture(bundle).map_err(|_| Error::Conflict)?;
        let relative = executable
            .strip_prefix(&root.path)
            .map_err(|_| Error::Unsupported)?;
        if relative.parent() != Some(Path::new("Contents/MacOS")) {
            return Err(Error::Unsupported);
        }
        let members = vec![
            member(&root, PathBuf::from("Contents/Info.plist"))?,
            member(&root, relative.to_owned())?,
        ];
        Ok(Self { root, members })
    }
    pub(super) fn same_application(&self, other: &Self) -> bool {
        self.root == other.root
            && self.members.iter().zip(&other.members).all(|(a, b)| {
                a.relative == b.relative && a.stamp == b.stamp && a.digest == b.digest
            })
    }
    pub(super) fn verify(&self) -> Result<()> {
        self.root.verify().map_err(|_| Error::Conflict)?;
        for retained in &self.members {
            let fresh = member(&self.root, retained.relative.clone())?;
            if fresh.stamp != retained.stamp || fresh.digest != retained.digest {
                return Err(Error::Conflict);
            }
        }
        Ok(())
    }
    /// Caller releases original/candidate writer fences only after preservation
    /// succeeds. This launches the exact unchanged native bundle, with no paths
    /// or credentials accepted from the renderer and no original-profile writes.
    pub(super) fn launch(&self) -> Result<()> {
        self.verify()?;
        let status = std::process::Command::new("/usr/bin/open")
            .args(["-n", "-a"])
            .arg(&self.root.path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|_| Error::ReconciliationRequired)?;
        if !status.success() {
            return Err(Error::ReconciliationRequired);
        }
        Ok(())
    }
}
