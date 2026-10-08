//! Validate the existing candidate main handle without opening a DB descriptor.
//! Closing any extra descriptor on its inode can release Unix SQLite locks.
#[cfg(all(test, unix))]
pub(super) fn check(
    connection: &rusqlite::Connection,
    binding: &crate::RootBinding,
) -> crate::Result<()> {
    check_at(connection, binding, std::path::Path::new("library.sqlite"))
}
use crate::{Error, Result, RootBinding};
use rusqlite::Connection;

pub(super) fn check_at(
    connection: &Connection,
    binding: &RootBinding,
    relative: &std::path::Path,
) -> Result<()> {
    crate::model::relative(relative)?;
    #[cfg(unix)]
    {
        let root = crate::platform::Root::private(&binding.locator.restore()?)?;
        if &root.binding != binding {
            return Err(Error::Conflict);
        }
        let expected = root.named_stamp(relative)?;
        crate::live_database::check(connection, &root, relative, &expected)
    }
    #[cfg(not(unix))]
    {
        let _ = (connection, binding, relative);
        Err(Error::Unsupported)
    }
}
