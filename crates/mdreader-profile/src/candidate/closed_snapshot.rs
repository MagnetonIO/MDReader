//! Receipt verification on an actual privately recovered closed-owner copy.
//! Live-owner verification remains unchanged; this requires the native closed
//! capability's exact original DB/sidecar fence before and after the read.
use super::*;
use crate::{database_backup::BackupSource, platform::Root};

pub fn verify_committed_closed_snapshot(
    connection: &Connection,
    expected: &CandidateReceipt,
    recovered: &BackupSource,
    mut revalidate_original: impl FnMut() -> Result<()>,
) -> Result<()> {
    expected.validate()?;
    recovered.root.validate()?;
    if !connection.is_autocommit() {
        return Err(Error::Conflict);
    }
    let readonly: bool = connection
        .query_row("PRAGMA query_only", [], |r| r.get(0))
        .map_err(|_| Error::Storage)?;
    if !readonly {
        return Err(Error::NotQuiescent);
    }
    let original = Root::private(&expected.candidate.locator.restore()?)?;
    let snapshot = Root::private(&recovered.root.locator.restore()?)?;
    if original.binding != expected.candidate
        || snapshot.binding != recovered.root
        || snapshot.path.starts_with(&original.path)
        || original.path.starts_with(&snapshot.path)
    {
        return Err(Error::Conflict);
    }
    let relative = recovered.relative.restore()?;
    let check = || -> Result<()> {
        original.verify()?;
        crate::live_database::check(connection, &snapshot, &relative, &recovered.stamp)
    };
    revalidate_original()?;
    check()?;
    schema(connection, false)?;
    if read(connection, expected.operation)?
        .as_ref()
        .map(digest)
        .transpose()?
        != Some(digest(expected)?)
    {
        return Err(Error::Conflict);
    }
    revalidate_original()?;
    check()
}
