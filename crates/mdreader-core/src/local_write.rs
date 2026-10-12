//! Guarded local publication shared by native editing and agent operations.
//! Slow staging occurs outside SQLite; the caller holds its owner fence during
//! commit. The advisory lock also serializes cooperating desktop saves.
use crate::source_root::file_stamp::{single_link_stamp, SourceFileStamp};
use crate::{ReaderError, Result, SourceRoot};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};
pub const MAX_BYTES: usize = 1024 * 1024;
pub fn body_revision(markdown: &str) -> String {
    blake3::hash(markdown.as_bytes()).to_hex().to_string()
}
pub struct PreparedWrite {
    root: SourceRoot,
    path: PathBuf,
    temp: PathBuf,
    lock: File,
    expected: Option<(String, SourceFileStamp)>,
    revision: String,
    committed: bool,
}
fn hash(root: &SourceRoot, path: &Path) -> Result<(String, SourceFileStamp)> {
    let mut file = root
        .open_optional_file_relative(path)?
        .ok_or(ReaderError::Unavailable)?;
    let stamp = single_link_stamp(&file)?;
    if file.metadata()?.len() > MAX_BYTES as u64 {
        return Err(ReaderError::Unavailable);
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES
        || single_link_stamp(&file)? != stamp
        || single_link_stamp(
            &root
                .open_optional_file_relative(path)?
                .ok_or(ReaderError::Unavailable)?,
        )? != stamp
    {
        return Err(ReaderError::Conflict);
    }
    Ok((blake3::hash(&bytes).to_hex().to_string(), stamp))
}
impl PreparedWrite {
    /// Stages a bounded Markdown body. None means exclusive creation.
    pub fn stage(
        root: SourceRoot,
        path: &str,
        expected: Option<&str>,
        markdown: &str,
    ) -> Result<Self> {
        if !crate::agent_sessions::markdown_path(path)
            || markdown.len() > MAX_BYTES
            || expected.is_some_and(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(ReaderError::Unavailable);
        }
        root.verify()?;
        let path = PathBuf::from(path);
        let parent = path.parent().ok_or(ReaderError::Unavailable)?;
        root.verify_directory_relative(parent)?;
        let lock = root.open_lock_relative(&parent.join(".mdreader.savelock"))?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| ReaderError::Unavailable)?;
        let expected = match expected {
            Some(expected) => {
                let (observed, stamp) = hash(&root, &path)?;
                if observed != expected {
                    return Err(ReaderError::Conflict);
                }
                Some((observed, stamp))
            }
            None => {
                if root.metadata_optional_relative(&path)?.is_some() {
                    return Err(ReaderError::Conflict);
                }
                None
            }
        };
        let temp = parent.join(format!(".mdreader-save-{}.tmp", uuid::Uuid::new_v4()));
        let mut file = root.create_new_relative(&temp)?;
        let write = (|| -> Result<()> {
            file.write_all(markdown.as_bytes())?;
            file.sync_all()?;
            Ok(())
        })();
        if let Err(error) = write {
            let _ = root.remove_file_relative(&temp);
            return Err(error);
        }
        Ok(Self {
            root,
            path,
            temp,
            lock,
            expected,
            revision: blake3::hash(markdown.as_bytes()).to_hex().to_string(),
            committed: false,
        })
    }
    /// Caller must keep its grant/generation fence through this short commit.
    pub fn commit(self, validate: impl FnOnce() -> Result<()>) -> Result<String> {
        self.commit_with_sync(validate, |root, parent| {
            root.sync_directory_relative(parent).map_err(Into::into)
        })
    }
    fn commit_with_sync(
        mut self,
        validate: impl FnOnce() -> Result<()>,
        sync: impl FnOnce(&SourceRoot, &Path) -> Result<()>,
    ) -> Result<String> {
        validate()?;
        self.root.verify()?;
        match &self.expected {
            Some(expected) => {
                if hash(&self.root, &self.path)? != *expected {
                    return Err(ReaderError::Conflict);
                }
                self.root.rename_relative(&self.temp, &self.path)?;
            }
            None => self
                .root
                .publish_new_relative(&self.temp, &self.path)
                .map_err(|error| {
                    if self
                        .root
                        .metadata_optional_relative(&self.path)
                        .is_ok_and(|v| v.is_some())
                    {
                        ReaderError::Conflict
                    } else {
                        error.into()
                    }
                })?,
        }
        self.committed = true;
        sync(
            &self.root,
            self.path.parent().ok_or(ReaderError::Unavailable)?,
        )?;
        self.root.verify()?;
        if hash(&self.root, &self.path)?.0 != self.revision {
            return Err(ReaderError::Conflict);
        }
        Ok(self.revision.clone())
    }
}
impl Drop for PreparedWrite {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.root.remove_file_relative(&self.temp);
        }
        let _ = fs2::FileExt::unlock(&self.lock);
    }
}
#[cfg(test)]
mod tests;
