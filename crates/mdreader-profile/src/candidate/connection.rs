//! Validate the existing candidate main handle without opening a DB descriptor.
//! Closing any extra descriptor on its inode can release Unix SQLite locks.
use crate::{Error, Result, RootBinding};
use rusqlite::Connection;

pub(super) fn check(connection: &Connection, binding: &RootBinding) -> Result<()> {
    #[cfg(unix)]
    {
        let root = crate::platform::Root::private(&binding.locator.restore()?)?;
        if &root.binding != binding {
            return Err(Error::Conflict);
        }
        let path = std::path::Path::new("library.sqlite");
        let expected = root.named_stamp(path)?;
        crate::live_database::check(connection, &root, path, &expected)
    }
    #[cfg(not(unix))]
    {
        let _ = (connection, binding);
        Err(Error::Unsupported)
    }
}
