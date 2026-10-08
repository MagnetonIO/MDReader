//! Trusted owner declarations govern which files may be opened for raw copy.
use mdreader_profile::{Disposition, Error, Exclusion, InventorySelection, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy)]
pub(crate) enum DatabaseKind {
    Durable,
    Rebuildable,
}
pub(crate) struct DatabaseRegistration {
    pub relative: PathBuf,
    pub kind: DatabaseKind,
}
impl DatabaseRegistration {
    pub(super) fn disposition(&self) -> Disposition {
        match self.kind {
            DatabaseKind::Durable => Disposition::Sqlite,
            DatabaseKind::Rebuildable => Disposition::Excluded(Exclusion::RebuildableIndex),
        }
    }
}

fn recovery_paths(path: &Path) -> [PathBuf; 3] {
    ["-wal", "-shm", "-journal"].map(|suffix| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    })
}
fn relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path.as_os_str().len() > 16384
        || path.components().count() > 64
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
pub(super) fn validate_registrations<'a>(
    databases: impl Iterator<Item = &'a DatabaseRegistration>,
) -> Result<()> {
    let mut occupied = BTreeSet::new();
    for database in databases {
        relative(&database.relative)?;
        for path in
            std::iter::once(database.relative.clone()).chain(recovery_paths(&database.relative))
        {
            if !occupied.insert(path) {
                return Err(Error::Invalid);
            }
        }
    }
    Ok(())
}
pub(super) fn validate_choices<'a>(
    databases: impl Iterator<Item = &'a DatabaseRegistration>,
    choices: &[InventorySelection],
) -> Result<()> {
    if choices.len() > 65536 {
        return Err(Error::Bounds);
    }
    let mut expected = BTreeMap::new();
    let mut durable = BTreeSet::new();
    for database in databases {
        expected.insert(database.relative.clone(), database.disposition());
        if matches!(database.kind, DatabaseKind::Durable) {
            durable.insert(database.relative.clone());
        }
        for path in recovery_paths(&database.relative) {
            expected.insert(
                path,
                Disposition::Excluded(Exclusion::SqliteRecoverySidecar),
            );
        }
    }
    let mut seen = BTreeSet::new();
    for choice in choices {
        relative(&choice.relative)?;
        if !seen.insert(&choice.relative) {
            return Err(Error::Invalid);
        }
        match expected.get(&choice.relative) {
            Some(disposition) if *disposition != choice.disposition => return Err(Error::Invalid),
            Some(_) => {
                durable.remove(&choice.relative);
            }
            None if matches!(
                choice.disposition,
                Disposition::Sqlite
                    | Disposition::Excluded(
                        Exclusion::SqliteRecoverySidecar | Exclusion::RebuildableIndex
                    )
            ) =>
            {
                return Err(Error::Invalid)
            }
            None => (),
        }
    }
    if !durable.is_empty() {
        return Err(Error::Invalid);
    }
    Ok(())
}
