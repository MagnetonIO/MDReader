//! Classification for the current reader-owned profile only. Unknown/legacy
//! stores must use explicit offline export; they are never guessed to be closed.
use mdreader_core::SourceRoot;
use mdreader_profile::{Disposition, Error, Exclusion, InventorySelection, Limits, Result};
use std::path::Path;

pub(super) fn selections(root: &Path, limits: &Limits) -> Result<Vec<InventorySelection>> {
    let root = SourceRoot::capture(root.to_owned()).map_err(|_| Error::Conflict)?;
    let mut result = Vec::new();
    let mut directories = 0usize;
    walk(&root, Path::new(""), limits, &mut directories, &mut result)?;
    root.verify().map_err(|_| Error::Conflict)?;
    Ok(result)
}
fn walk(
    root: &SourceRoot,
    relative: &Path,
    limits: &Limits,
    directories: &mut usize,
    output: &mut Vec<InventorySelection>,
) -> Result<()> {
    if relative.components().count() > limits.max_depth {
        return Err(Error::Bounds);
    }
    *directories += 1;
    if directories.saturating_add(output.len()) > limits.max_entries {
        return Err(Error::Bounds);
    }
    let directory = SourceRoot::capture(root.path.join(relative)).map_err(|_| Error::Conflict)?;
    for entry in std::fs::read_dir(&directory.path).map_err(|_| Error::Storage)? {
        let entry = entry.map_err(|_| Error::Storage)?;
        let kind = entry.file_type().map_err(|_| Error::Storage)?;
        let path = relative.join(entry.file_name());
        if kind.is_dir() {
            walk(root, &path, limits, directories, output)?;
        } else if kind.is_file() {
            if directories.saturating_add(output.len()) >= limits.max_entries {
                return Err(Error::Bounds);
            }
            output.push(InventorySelection {
                disposition: classify(&path)?,
                relative: path,
            });
        } else {
            return Err(Error::Invalid);
        } // No symlinks, sockets or special files.
    }
    directory.verify().map_err(|_| Error::Conflict)?;
    Ok(())
}
fn classify(path: &Path) -> Result<Disposition> {
    use Disposition::{Excluded, Raw, Sqlite};
    use Exclusion::{
        Diagnostics, ModelCache, RebuildableIndex, RuntimeEndpointOrLock, SqliteRecoverySidecar,
    };
    // Only the diagnostic owner's exact known files are admitted. Unknown
    // stores inside this directory still require explicit offline preservation.
    if path == Path::new("diagnostics/settings.json") {
        return Ok(Raw);
    }
    if path == Path::new("diagnostics/diagnostics.lock") {
        return Ok(Excluded(RuntimeEndpointOrLock));
    }
    if path == Path::new("diagnostics/mdreader.log")
        || (1..crate::diagnostics::LOG_RETENTION)
            .any(|slot| path == Path::new(&format!("diagnostics/mdreader.log.{slot}")))
    {
        return Ok(Excluded(Diagnostics));
    }
    if [
        "session.json",
        "settings.json",
        "library.json",
        "reader-profile.json",
        "native-reader-profile.json",
        ".mdreader-legacy-return-v1.json",
        ".mdreader-legacy-return-next-v1.json",
    ]
    .iter()
    .any(|name| path == Path::new(name))
    {
        return Ok(Raw);
    }
    if [
        ".mdreader-json-owner.lock",
        "library.lock",
        "reader-v1/reader.lock",
        "reader-v1/reader-endpoint.json",
        "reader-v1/native-provider-admissions-v1.lock",
        "reader-v1/captures/queue.lock",
        "reader-v1/integrations/settings/installer.lock",
    ]
    .iter()
    .any(|name| path == Path::new(name))
    {
        return Ok(Excluded(RuntimeEndpointOrLock));
    }
    for database in ["reader-v1/library.sqlite", "reader-v1/index.sqlite"] {
        if path == Path::new(database) {
            return Ok(if database.ends_with("/index.sqlite") {
                Excluded(RebuildableIndex)
            } else {
                Sqlite
            });
        }
        if ["-wal", "-shm", "-journal"]
            .iter()
            .any(|suffix| path == Path::new(&format!("{database}{suffix}")))
        {
            return Ok(Excluded(SqliteRecoverySidecar));
        }
    }
    if path.starts_with("reader-v1/models") {
        return Ok(Excluded(ModelCache));
    }
    if path == Path::new("reader-v1/native-provider-admissions-v1.json")
        || [
            "reader-v1/device-markdown-v1",
            "reader-v1/device-assets-v1",
            "reader-v1/provider-cache",
            "reader-v1/captures",
            "reader-v1/integrations/settings/receipts",
            "legacy-archive",
            "archived-reviews",
        ]
        .iter()
        .any(|directory| path.starts_with(directory))
    {
        return Ok(Raw);
    }
    Err(Error::Unclassified)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn fresh_profile_diagnostics_are_classified_without_hiding_unknown_stores() {
        let profile = tempfile::tempdir().unwrap();
        let diagnostics = profile.path().join("diagnostics");
        std::fs::create_dir(&diagnostics).unwrap();
        for name in [
            "diagnostics.lock",
            "mdreader.log",
            "mdreader.log.1",
            "mdreader.log.2",
            "settings.json",
        ] {
            std::fs::write(diagnostics.join(name), b"synthetic fixture").unwrap();
        }
        let limits = Limits::default();
        let selected = selections(profile.path(), &limits).unwrap();
        let inventory =
            mdreader_profile::Inventory::capture(profile.path(), &selected, &limits).unwrap();
        assert_eq!(inventory.entries().len(), 5);
        assert_eq!(
            selected
                .iter()
                .filter(|entry| entry.disposition == Disposition::Raw)
                .count(),
            1
        );
        assert_eq!(
            classify(Path::new("diagnostics/settings.json")).unwrap(),
            Disposition::Raw
        );
        assert_eq!(
            classify(Path::new("diagnostics/mdreader.log")).unwrap(),
            Disposition::Excluded(Exclusion::Diagnostics)
        );
        std::fs::write(diagnostics.join("library.sqlite"), b"unknown durable bytes").unwrap();
        assert!(matches!(
            selections(profile.path(), &limits),
            Err(Error::Unclassified)
        ));
        assert_eq!(
            std::fs::read(diagnostics.join("library.sqlite")).unwrap(),
            b"unknown durable bytes"
        );
    }
}
