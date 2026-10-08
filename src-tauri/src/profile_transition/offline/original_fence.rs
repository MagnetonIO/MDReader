//! Reopened offline originals are compared by durable authority, not excluded
//! runtime SHM/lock/index inodes that legitimately change on owner restart.
use mdreader_profile::{Disposition, Error, Exclusion, Inventory, Manifest, ManifestEntry, Result};

fn retained(entry: &ManifestEntry) -> bool {
    match entry.disposition {
        Disposition::Raw | Disposition::Sqlite => true,
        Disposition::Excluded(Exclusion::SqliteRecoverySidecar) => {
            // A nonempty WAL/journal may contain durable state absent from the
            // main file. Never treat it like disposable shared-memory metadata.
            entry.bytes != 0
                && entry.relative.restore().is_ok_and(|path| {
                    path.file_name().is_some_and(|name| {
                        let name = name.to_string_lossy();
                        name.ends_with("-wal") || name.ends_with("-journal")
                    })
                })
        }
        _ => false,
    }
}

pub(super) fn verify(current: &Inventory, original: &Manifest) -> Result<()> {
    if current.binding() != &original.original {
        return Err(Error::Conflict);
    }
    let evidence = |entry: &ManifestEntry| {
        (
            entry.relative.clone(),
            entry.disposition,
            entry.source_stamp,
        )
    };
    let current = current
        .entries()
        .iter()
        .filter(|entry| retained(entry))
        .map(evidence);
    let original = original
        .entries
        .iter()
        .filter(|entry| retained(entry))
        .map(evidence);
    if !current.eq(original) {
        return Err(Error::Conflict);
    }
    Ok(())
}
