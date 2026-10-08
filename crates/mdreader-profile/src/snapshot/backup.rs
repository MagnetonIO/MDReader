use crate::{Error, Result};
use rusqlite::Connection;
use std::path::Path;

#[cfg(unix)]
pub(crate) fn check_database(
    source: &Connection,
    root: &crate::platform::Root,
    path: &Path,
    expected: &crate::platform::Stamp,
) -> Result<()> {
    crate::live_database::check(source, root, path, expected)
}
#[cfg(unix)]
pub(crate) fn backup(
    source: &Connection,
    output: &mut Connection,
    bound: u64,
    max_steps: usize,
    mut before_step: impl FnMut() -> Result<()>,
) -> Result<()> {
    use rusqlite::backup::{Backup, StepResult};
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
    let backup = Backup::new(source, output).map_err(|_| Error::Storage)?;
    let mut done = false;
    for _ in 0..max_steps {
        before_step()?;
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
    Ok(())
}
