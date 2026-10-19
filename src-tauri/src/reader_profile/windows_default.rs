//! Windows default selection is canonical private SQLite, never a rename/fsync
//! claim about a JSON projection. Every mutation compares the retained generation.
use mdreader_core::{ReaderStore, SourceRoot};
use mdreader_profile::{Error, Result, RootBinding};
use rusqlite::{params, Connection};
use std::{path::Path, sync::Mutex};
mod schema;

#[derive(Clone, PartialEq, Eq)]
struct Selection {
    generation: u64,
    target: Option<RootBinding>,
}
struct Owner {
    store: ReaderStore,
    expected: Selection,
}
pub(super) struct DefaultSelection {
    owner: Mutex<Owner>,
    root: SourceRoot,
}
fn read(db: &Connection) -> Result<Selection> {
    schema::verify(db)?;
    let (generation, target): (u64, Option<String>) = db.query_row(
        "SELECT generation,target FROM native_default_windows_v1 WHERE singleton=1 AND version=1 AND generation BETWEEN 1 AND 9007199254740991 AND (target IS NULL OR length(CAST(target AS BLOB))<=32768)", [], |r| Ok((r.get(0)?, r.get(1)?)),
    ).map_err(|_| Error::Conflict)?;
    let target = target
        .map(|text| -> Result<RootBinding> {
            let binding: RootBinding = serde_json::from_str(&text).map_err(|_| Error::Integrity)?;
            binding.validate()?;
            Ok(binding)
        })
        .transpose()?;
    Ok(Selection { generation, target })
}
fn selected(binding: &RootBinding) -> Result<SourceRoot> {
    let path = binding.locator.restore()?;
    if RootBinding::capture_private(&path)? != *binding {
        return Err(Error::Conflict);
    }
    let root = crate::profile_transition::offline::windows_workspace::selected_for_launch(&path)?;
    if RootBinding::capture_private(&path)? != *binding {
        return Err(Error::Conflict);
    }
    Ok(root)
}
impl DefaultSelection {
    pub(super) fn open(standard: &Path) -> Result<Self> {
        let parent = standard.parent().ok_or(Error::Invalid)?;
        let parent =
            mdreader_capture::SelectedDirectory::open(parent).map_err(|_| Error::Conflict)?;
        let name = standard
            .file_name()
            .ok_or(Error::Invalid)?
            .to_str()
            .ok_or(Error::Unsupported)?;
        let name = format!("{name}.reader-selection-windows-v1");
        let path = standard.parent().ok_or(Error::Invalid)?.join(&name);
        let existing = std::fs::symlink_metadata(&path);
        let (private, fresh) = match existing {
            Ok(_) => (
                mdreader_capture::PrivateDirectory::open(&path).map_err(|_| Error::Conflict)?,
                false,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
                parent
                    .create_private_child(Path::new(&name))
                    .map_err(|_| Error::Storage)?,
                true,
            ),
            Err(_) => return Err(Error::Storage),
        };
        let root = SourceRoot::capture(path).map_err(|_| Error::Conflict)?;
        RootBinding::capture_private(&root.path)?;
        let mut store =
            ReaderStore::open_private(&root.path.join("control")).map_err(|_| Error::Storage)?;
        store.with_library_connection(|db| schema::initialize(db, fresh))?;
        let expected = store.with_library_connection(|db| read(db))?;
        private.verify().map_err(|_| Error::Conflict)?;
        parent.verify().map_err(|_| Error::Conflict)?;
        Ok(Self {
            owner: Mutex::new(Owner { store, expected }),
            root,
        })
    }

    pub(super) fn startup_selection(&self) -> Result<(Option<SourceRoot>, bool)> {
        self.root.verify().map_err(|_| Error::Conflict)?;
        let mut owner = self
            .owner
            .lock()
            .map_err(|_| Error::ReconciliationRequired)?;
        let actual = owner.store.with_library_connection(|db| read(db))?;
        if actual != owner.expected {
            return Err(Error::Conflict);
        }
        let observation = if let Some(target) = &actual.target {
            let path = target.locator.restore()?;
            if RootBinding::capture_private(&path)? != *target {
                return Err(Error::Conflict);
            }
            let (mut root, imported) =
                crate::profile_transition::offline::windows_workspace::startup_selection(&path)?;
            if !imported
                && crate::profile_transition::offline::windows_workspace::legacy_selection(&path)?
            {
                root = self.fresh_reader()?;
            }
            if RootBinding::capture_private(&path)? != *target {
                return Err(Error::Conflict);
            }
            (Some(root), imported)
        } else {
            (None, false)
        };
        if owner.store.with_library_connection(|db| read(db))? != actual {
            return Err(Error::Conflict);
        }
        self.root.verify().map_err(|_| Error::Conflict)?;
        Ok(observation)
    }
    pub(super) fn fresh_reader(&self) -> Result<SourceRoot> {
        self.root.verify().map_err(|_| Error::Conflict)?;
        let path = self.root.path.join("current-reader");
        let private =
            mdreader_capture::PrivateDirectory::create(&path).map_err(|_| Error::Conflict)?;
        let root = SourceRoot::capture(path).map_err(|_| Error::Conflict)?;
        RootBinding::capture_private(&root.path)?;
        private.verify().map_err(|_| Error::Conflict)?;
        self.root.verify().map_err(|_| Error::Conflict)?;
        Ok(root)
    }
    pub(super) fn return_controller(&self) -> Result<SourceRoot> {
        self.root.verify().map_err(|_| Error::Conflict)?;
        self.root
            .read_dir_relative_bounded(Path::new(""), 64)
            .map_err(|_| Error::Bounds)?;
        let parent = mdreader_capture::PrivateDirectory::open(&self.root.path)
            .map_err(|_| Error::Conflict)?;
        let name = format!("return-{}", uuid::Uuid::new_v4());
        let child = parent
            .create_child(Path::new(&name))
            .map_err(|_| Error::Storage)?;
        child.verify().map_err(|_| Error::Conflict)?;
        parent.verify().map_err(|_| Error::Conflict)?;
        SourceRoot::capture(self.root.path.join(name)).map_err(|_| Error::Conflict)
    }
    pub(super) fn return_original(&self, candidate: &Path) -> Result<()> {
        let binding = RootBinding::capture_private(candidate.parent().ok_or(Error::Invalid)?)?;
        let mut owner = self
            .owner
            .lock()
            .map_err(|_| Error::ReconciliationRequired)?;
        let actual = owner.store.with_library_connection(|db| read(db))?;
        if actual != owner.expected || actual.target.as_ref() != Some(&binding) {
            return Err(Error::Conflict);
        }
        if crate::profile_transition::offline::windows_workspace::active_selection(
            &binding.locator.restore()?,
        )? {
            return Err(Error::Conflict);
        }
        // The portable lifecycle CAS now selects the original within this exact
        // native selector. Keep this durable target for nonstandard originals too.
        selected(&binding)?;
        self.root.verify().map_err(|_| Error::Conflict)
    }
    pub(super) fn selected(&self) -> Result<Option<SourceRoot>> {
        self.startup_selection().map(|(root, _)| root)
    }
    pub(super) fn select(&self, candidate: &Path) -> Result<()> {
        let migration = candidate.parent().ok_or(Error::Invalid)?;
        let binding = RootBinding::capture_private(migration)?;
        let actual = selected(&binding)?;
        if RootBinding::capture_private(candidate)? != RootBinding::capture_private(&actual.path)? {
            return Err(Error::Conflict);
        }
        self.publish(Some(binding))
    }
    fn publish(&self, target: Option<RootBinding>) -> Result<()> {
        self.root.verify().map_err(|_| Error::Conflict)?;
        if let Some(target) = &target {
            selected(target)?;
        }
        let mut owner = self
            .owner
            .lock()
            .map_err(|_| Error::ReconciliationRequired)?;
        let expected = owner.expected.clone();
        // Exact same-value publication is an idempotent reconciliation, not CAS.
        let next = if expected.target == target {
            expected.clone()
        } else {
            Selection {
                generation: expected
                    .generation
                    .checked_add(1)
                    .filter(|g| *g <= 9007199254740991)
                    .ok_or(Error::Bounds)?,
                target,
            }
        };
        let encoded = next
            .target
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| Error::Invalid)?;
        if encoded.as_ref().is_some_and(|text| text.len() > 32768) {
            return Err(Error::Bounds);
        }
        let publication = owner.store.with_library_connection(|db| -> Result<()> {
            let tx = db.transaction().map_err(|_| Error::Storage)?;
            if read(&tx)? != expected { return Err(Error::Conflict); }
            if next != expected {
                let changed = tx.execute("UPDATE native_default_windows_v1 SET generation=?1,target=?2 WHERE singleton=1 AND generation=?3", params![next.generation, encoded, expected.generation]).map_err(|_| Error::Storage)?;
                if changed != 1 { return Err(Error::Conflict); }
            }
            if read(&tx)? != next { return Err(Error::Conflict); }
            if let Some(target) = &next.target { selected(target)?; }
            self.root.verify().map_err(|_| Error::Conflict)?;
            tx.commit().map_err(|_| Error::ReconciliationRequired)
        });
        if publication.is_err() {
            // Preserve an uncertain result, but retain exact successful CAS if
            // the durable row proves that this desired generation committed.
            if owner
                .store
                .with_library_connection(|db| read(db))
                .is_ok_and(|actual| actual == next)
            {
                owner.expected = next;
            }
            return publication;
        }
        owner.expected = next;
        Ok(())
    }
}

#[cfg(test)]
#[path = "windows_default/tests.rs"]
mod tests;
