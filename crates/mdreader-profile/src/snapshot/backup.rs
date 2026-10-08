use crate::{Error, Limits, Result};
use rusqlite::Connection;
use std::path::Path;

#[cfg(unix)]
pub(super) fn check_database(
    source: &Connection,
    root: &crate::platform::Root,
    path: &Path,
    expected: &crate::platform::Stamp,
) -> Result<()> {
    let actual: String = source
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name='main'",
            [],
            |r| r.get(0),
        )
        .map_err(|_| Error::Storage)?;
    let named = std::fs::File::open(&actual).map_err(|_| Error::Storage)?;
    let stamp = crate::platform::stamp(&named)?;
    if (stamp.0, stamp.1) != (expected.0, expected.1)
        || crate::platform::stamp(&root.open(path)?)? != *expected
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
#[cfg(unix)]
pub(super) fn backup(
    source: &Connection,
    destination: &Path,
    bound: u64,
    limits: &Limits,
) -> Result<()> {
    use rusqlite::backup::{Backup, StepResult};
    let mut output = Connection::open_with_flags(
        destination,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| Error::Storage)?;
    let page_size: u64 = source
        .query_row("PRAGMA page_size", [], |r| r.get(0))
        .map_err(|_| Error::Storage)?;
    if !(512..=65536).contains(&page_size) {
        return Err(Error::Integrity);
    }
    let pages: u64 = source
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .map_err(|_| Error::Storage)?;
    if pages.checked_mul(page_size).ok_or(Error::Bounds)? > bound {
        return Err(Error::Bounds);
    }
    let backup = Backup::new(source, &mut output).map_err(|_| Error::Storage)?;
    let mut done = false;
    for _ in 0..limits.max_backup_steps {
        match backup.step(128).map_err(|_| Error::Storage)? {
            StepResult::Done => {
                done = true;
                break;
            }
            StepResult::More => (),
            StepResult::Busy | StepResult::Locked => return Err(Error::NotQuiescent),
            _ => return Err(Error::Storage),
        }
        if u64::try_from(backup.progress().pagecount)
            .map_err(|_| Error::Bounds)?
            .checked_mul(page_size)
            .ok_or(Error::Bounds)?
            > bound
        {
            return Err(Error::Bounds);
        }
    }
    if !done {
        return Err(Error::Bounds);
    }
    drop(backup);
    // The immutable archive is standalone. Do not inherit WAL mode from the
    // source and let a subsequent read create members outside its manifest.
    let mode: String = output
        .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
        .map_err(|_| Error::Storage)?;
    if mode != "delete" {
        return Err(Error::Integrity);
    }
    output.close().map_err(|_| Error::Storage)?;
    Ok(())
}
