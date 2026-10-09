use super::*;

impl SourceRoot {
    /// Remove an empty child directory through the retained Unix parent handle.
    /// Symlinks and nonempty directories are refused; other hosts fail closed.
    pub fn remove_empty_directory_relative(&self, relative: &Path) -> Result<(), SourceRootError> {
        validate_relative(relative, false)?;
        self.verify()?;
        #[cfg(unix)]
        {
            let (parent, name) = open_unix_parent(self, relative)?;
            rustix::fs::unlinkat(&parent, &name, rustix::fs::AtFlags::REMOVEDIR).map_err(|error| {
                SourceRootError::CannotInspect {
                    path: self.path.join(relative),
                    reason: error.to_string(),
                }
            })
        }
        #[cfg(not(unix))]
        Err(SourceRootError::UnsupportedPlatform)
    }
}
