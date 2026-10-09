//! Archive-only observation of a Unix link. The target is opaque bytes, never
//! opened or converted into source authority.
use super::*;
use rustix::fs::{fstat, readlinkat_raw, statat, AtFlags, FileType, Stat};

#[derive(Clone, PartialEq, Eq)]
pub struct SourceSymlinkSnapshot {
    pub target: Vec<u8>,
    pub device: u64,
    pub inode: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub links: u64,
    pub bytes: u64,
    pub mtime: [i64; 2],
    pub ctime: [i64; 2],
}
const MAX_TARGET: usize = 16384;
fn observation(stat: &Stat, target: Vec<u8>) -> Result<SourceSymlinkSnapshot, ()> {
    fn unsigned<T: TryInto<u64>>(n: T) -> Result<u64, ()> {
        n.try_into().map_err(|_| ())
    }
    fn signed<T: TryInto<i64>>(n: T) -> Result<i64, ()> {
        n.try_into().map_err(|_| ())
    }
    if !FileType::from_raw_mode(stat.st_mode).is_symlink() {
        return Err(());
    }
    Ok(SourceSymlinkSnapshot {
        target,
        device: unsigned(stat.st_dev)?,
        inode: unsigned(stat.st_ino)?,
        mode: unsigned(stat.st_mode)?.try_into().map_err(|_| ())?,
        uid: stat.st_uid,
        gid: stat.st_gid,
        links: unsigned(stat.st_nlink)?,
        bytes: unsigned(stat.st_size)?,
        mtime: [signed(stat.st_mtime)?, signed(stat.st_mtime_nsec)?],
        ctime: [signed(stat.st_ctime)?, signed(stat.st_ctime_nsec)?],
    })
}
impl SourceRoot {
    /// Capture at most 16 KiB of link target bytes through a retained no-follow
    /// parent descriptor. Compare named identity before/after the read and after
    /// re-opening the parent through this root; never follow the target.
    pub fn read_symlink_relative(
        &self,
        relative: &Path,
    ) -> Result<SourceSymlinkSnapshot, SourceRootError> {
        validate_relative(relative, false)?;
        self.verify()?;
        let path = self.path.join(relative);
        let error = || SourceRootError::CannotInspect {
            path: path.clone(),
            reason: "symlink identity or bounded target changed".into(),
        };
        let (parent, name) = open_unix_parent(self, relative)?;
        let before = statat(&parent, &name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|e| unix_source_error(path.clone(), e))?;
        let initial = observation(&before, Vec::new()).map_err(|_| error())?;
        if initial.bytes == 0 || initial.bytes > MAX_TARGET as u64 {
            return Err(error());
        }
        let mut buffer = [0u8; MAX_TARGET + 1];
        let read = readlinkat_raw(&parent, &name, &mut buffer[..])
            .map_err(|e| unix_source_error(path.clone(), e))?;
        if read == 0 || read > MAX_TARGET || read as u64 != initial.bytes {
            return Err(error());
        }
        let snapshot = observation(&before, buffer[..read].to_vec()).map_err(|_| error())?;
        let after = statat(&parent, &name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|e| unix_source_error(path.clone(), e))?;
        let (named_parent, named) = open_unix_parent(self, relative)?;
        let first_parent = fstat(&parent).map_err(|e| unix_source_error(path.clone(), e))?;
        let current_parent =
            fstat(&named_parent).map_err(|e| unix_source_error(path.clone(), e))?;
        let current = statat(&named_parent, named, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|e| unix_source_error(path.clone(), e))?;
        if (first_parent.st_dev, first_parent.st_ino)
            != (current_parent.st_dev, current_parent.st_ino)
            || observation(&after, snapshot.target.clone()).map_err(|_| error())? != snapshot
            || observation(&current, snapshot.target.clone()).map_err(|_| error())? != snapshot
        {
            return Err(error());
        }
        self.verify()?;
        Ok(snapshot)
    }
}
