//! Useful legacy-record snapshots come only from a fully closed raw original.
//! The canonical lifecycle snapshot remains the lossless original authority.
use super::*;
use crate::VerifiedSnapshot;
use mdreader_capture::PrivateDirectory;
use mdreader_legacy_export::{self as legacy, Input, Role};
use serde::Serialize;
use std::io::Write;

pub struct ConversionArchive {
    root: crate::platform::Root,
    identity: legacy::reviews::ArchiveIdentity,
}
impl ConversionArchive {
    /// Reopen immutable conversion bytes against a previously recorded digest.
    /// This never restores original application/process closure.
    pub fn reopen(path: &Path, identity: &legacy::reviews::ArchiveIdentity) -> Result<Self> {
        let archive = Self {
            root: crate::platform::Root::private(path)?,
            identity: identity.clone(),
        };
        archive.verify()?;
        Ok(archive)
    }
    pub fn path(&self) -> &Path {
        &self.root.path
    }
    pub fn identity(&self) -> &legacy::reviews::ArchiveIdentity {
        &self.identity
    }
    pub fn verify(&self) -> Result<()> {
        self.root.verify()?;
        if legacy::migration::archive::identity(&self.root.path, &Default::default())
            .map_err(failure)?
            != self.identity
        {
            return Err(Error::Conflict);
        }
        self.root.verify()
    }
}
fn failure(error: legacy::Error) -> Error {
    match error {
        legacy::Error::Bounds => Error::Bounds,
        legacy::Error::Conflict => Error::Conflict,
        legacy::Error::Invalid => Error::Invalid,
        legacy::Error::Unsupported => Error::Unsupported,
        _ => Error::Integrity,
    }
}
fn role(path: &Path) -> Option<Role> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    match name.as_str() {
        "cloud-library.catdb" if path.components().count() == 1 => Some(Role::CloudCatalog),
        "catdb.db"
            if path.components().count() == 3
                && path
                    .components()
                    .next()?
                    .as_os_str()
                    .to_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case("workspaces")) =>
        {
            Some(Role::Catalog)
        }
        "proposals.db"
            if path.components().count() == 1
                || (path.components().count() == 3
                    && path
                        .components()
                        .next()?
                        .as_os_str()
                        .to_str()
                        .is_some_and(|name| name.eq_ignore_ascii_case("workspaces"))) =>
        {
            Some(Role::Proposals)
        }
        "settings.json" if path.components().count() == 1 => Some(Role::Settings),
        "session.json" if path.components().count() == 1 => Some(Role::Session),
        "library.json" if path.components().count() == 1 => Some(Role::Library),
        _ => None,
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Mapping {
    relative: RelativeLocator,
    role: Role,
    artifact: usize,
}
impl ClosedOriginal {
    /// Decode only known selected profile artifacts into a separate private
    /// archive. All unknown files/rows, database journals and native names remain
    /// preserved by `original`, whose inventory must match the retained owner.
    /// No SDK, original SQLite connection or archived source grant is opened.
    pub fn conversion_archive(
        &mut self,
        original: &VerifiedSnapshot,
        private_parent: &Path,
        destination: &Path,
    ) -> Result<ConversionArchive> {
        self.revalidate()?;
        let inventory = self.inventory()?;
        if original.manifest().original != self.binding
            || original.manifest().inventory_sha256 != inventory.identity()
        {
            return Err(Error::Conflict);
        }
        original.revalidate(&self.limits)?;
        let parent = crate::platform::Root::private(private_parent)?;
        let original_path = self.binding.locator.restore()?;
        if parent.path.starts_with(&original_path) || destination.starts_with(&original_path) {
            return Err(Error::InvalidDestination);
        }
        let selected = PrivateDirectory::open(&parent.path).map_err(|_| Error::Conflict)?;
        let name = format!("legacy-conversion-{}", uuid::Uuid::new_v4());
        let copies = selected
            .create_child(Path::new(&name))
            .map_err(|_| Error::Storage)?;
        let copy_path = parent.path.join(name);
        let root = SourceRoot::capture(copy_path.clone()).map_err(|_| Error::Conflict)?;
        let mapping = self
            .files
            .keys()
            .filter_map(|path| role(path).map(|role| (path.clone(), role)))
            .collect::<Vec<_>>();
        // One extra raw provenance member links this decoder archive to the
        // verified canonical snapshot. It never creates an original path grant.
        if mapping.is_empty() {
            return Err(Error::Unsupported);
        }
        if mapping.len() >= legacy::Limits::default().max_artifacts {
            return Err(Error::Bounds);
        }
        let mut recovered = Vec::new();
        let mut sealed = Vec::new();
        let mut inputs = Vec::new();
        let mut provenance = Vec::new();
        for (relative, role) in mapping {
            let path = if matches!(role, Role::Catalog | Role::CloudCatalog | Role::Proposals) {
                let mut database = self.recover_database(&relative, &copy_path)?;
                database.verify()?;
                let path = database.path();
                recovered.push(database);
                path
            } else {
                let name = format!("input-{}.json", inputs.len());
                let original = self.files.get_mut(&relative).ok_or(Error::Conflict)?;
                let mut target = copies
                    .create_database_file(Path::new(&name))
                    .map_err(|_| Error::Storage)?;
                let bound = self.limits.max_file_bytes.min(256 * 1024 * 1024);
                let copied = crate::platform::copy_file(&mut original.file, &mut target, bound)?;
                if copied.0 != original.stamp.2
                    || crate::platform::hash_file(&mut original.file, bound)? != copied
                    || observe_object(&original.file, false, false).map_err(|_| Error::Conflict)?
                        != original.stamp
                {
                    return Err(Error::Conflict);
                }
                drop(target);
                let file = files::retain(&root, Path::new(&name))?;
                let stamp = observe_object(&file, false, true).map_err(|_| Error::Conflict)?;
                sealed.push((PathBuf::from(&name), file, stamp));
                copy_path.join(name)
            };
            provenance.push(Mapping {
                relative: RelativeLocator::capture(&relative)?,
                role,
                artifact: inputs.len(),
            });
            inputs.push(Input { role, path });
        }
        let bytes = serde_json::to_vec(&serde_json::json!({
            "version":1,"originalSnapshot":original.identity(),"original":self.binding,
            "inputs":provenance
        }))
        .map_err(|_| Error::Invalid)?;
        if bytes.len() > self.limits.max_manifest_bytes {
            return Err(Error::Bounds);
        }
        let mut output = copies
            .create_database_file(Path::new("conversion-provenance.json"))
            .map_err(|_| Error::Storage)?;
        output.write_all(&bytes).map_err(|_| Error::Storage)?;
        output.sync_all().map_err(|_| Error::Storage)?;
        drop(output);
        let file = files::retain(&root, Path::new("conversion-provenance.json"))?;
        let stamp = observe_object(&file, false, true).map_err(|_| Error::Conflict)?;
        sealed.push(("conversion-provenance.json".into(), file, stamp));
        inputs.push(Input {
            role: Role::Raw,
            path: copy_path.join("conversion-provenance.json"),
        });
        self.revalidate()?;
        legacy::export(&inputs, destination, &Default::default()).map_err(failure)?;
        for database in &mut recovered {
            database.verify()?;
        }
        for (name, file, stamp) in &sealed {
            let named = root
                .open_optional_file_relative(name)
                .map_err(|_| Error::Conflict)?
                .ok_or(Error::Conflict)?;
            if observe_object(file, false, true).map_err(|_| Error::Conflict)? != *stamp
                || observe_object(&named, false, true).map_err(|_| Error::Conflict)? != *stamp
            {
                return Err(Error::Conflict);
            }
        }
        self.revalidate()?;
        original.revalidate(&self.limits)?;
        copies.verify().map_err(|_| Error::Conflict)?;
        selected.verify().map_err(|_| Error::Conflict)?;
        parent.verify()?;
        let identity = legacy::migration::archive::identity(destination, &Default::default())
            .map_err(failure)?;
        let result = ConversionArchive {
            root: crate::platform::Root::private(destination)?,
            identity,
        };
        result.verify()?;
        Ok(result)
    }
}
