//! Legacy useful-record materialization uses the real retained original owner.
//! Native activation remains a lifecycle CAS, never direct archive-root launch.
use super::*;
use mdreader_legacy_export::{self as legacy, catalog::reader::ReaderDocument};
use mdreader_profile::legacy_windows::{ClosedOriginal, ConversionArchive};
use sha2::{Digest, Sha256};
#[path = "legacy/completed.rs"]
mod completed;
#[path = "legacy/resume.rs"]
mod resume;
pub(in crate::profile_transition::offline) use resume::{observation, Recovery};

pub(in crate::profile_transition::offline) fn failure(error: legacy::Error) -> Error {
    match error {
        legacy::Error::Bounds => Error::Bounds,
        legacy::Error::Conflict => Error::Conflict,
        legacy::Error::Invalid => Error::Invalid,
        legacy::Error::Unsupported => Error::Unsupported,
        _ => Error::Integrity,
    }
}
fn filename(title: &str) -> String {
    let mut name = String::new();
    for c in title.chars() {
        let c = if c.is_control() || matches!(c, '/' | '\\') {
            '_'
        } else {
            c
        };
        if name.len() + c.len_utf8() > 220 {
            break;
        }
        name.push(c);
    }
    if name.is_empty() {
        name.push_str("Recovered record");
    }
    if !name.to_ascii_lowercase().ends_with(".md") {
        name.push_str(".md");
    }
    name
}
pub(in crate::profile_transition::offline) fn documents(
    archive: &ConversionArchive,
) -> Result<Vec<super::super::commands::Document>> {
    archive.verify()?;
    let documents =
        legacy::catalog::reader::documents(archive.path(), archive.identity(), &Default::default())
            .map_err(failure)?;
    if documents.len() > 1000 {
        return Err(Error::Bounds);
    }
    Ok(documents
        .into_iter()
        .map(|doc| super::super::commands::Document {
            document_id: doc.id,
            name: filename(&doc.title),
            revision: blake3::hash(doc.markdown.as_bytes()).to_hex().to_string(),
            bytes: doc.markdown.len() as u32,
            pinned: false,
            line: 1,
        })
        .collect())
}
fn materialize(
    documents: Vec<ReaderDocument>,
    destination: &mut ReaderStore,
    root: &Path,
    selected: &[Selection],
    identity: &legacy::reviews::ArchiveIdentity,
) -> Result<(u64, String)> {
    let selected_ids = selected
        .iter()
        .map(|s| (&s.document_id, &s.revision))
        .collect::<std::collections::BTreeMap<_, _>>();
    if selected.is_empty() || selected.len() > 1000 || selected_ids.len() != selected.len() {
        return Err(Error::Invalid);
    }
    let mut aliases = Vec::new();
    let directory = root.join("reader-v1/device-markdown-v1");
    let mut hash = Sha256::new();
    hash.update(b"mdreader-windows-legacy-selected-import-v1\0");
    for doc in documents
        .into_iter()
        .filter(|d| selected_ids.contains_key(&d.id))
    {
        let revision = blake3::hash(doc.markdown.as_bytes()).to_hex().to_string();
        if selected_ids
            .get(&doc.id)
            .is_none_or(|expected| expected.as_str() != revision)
        {
            return Err(Error::Conflict);
        }
        let imported = mdreader_core::device::import(
            destination,
            &directory,
            filename(&doc.title),
            doc.markdown,
        )
        .map_err(|_| Error::Storage)?;
        if imported.revision != revision {
            return Err(Error::Integrity);
        }
        for value in [&doc.id, &revision, &imported.id, &imported.source_id] {
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value.as_bytes());
        }
        aliases.push(mdreader_core::device::archive::Alias {
            receipt: mdreader_core::device::catalog::receipt(&imported),
            artifact_index: doc.artifact_index as u32,
            legacy_id: if doc.legacy_id.len() <= 16384 {
                doc.legacy_id
            } else {
                blake3::hash(doc.legacy_id.as_bytes()).to_hex().to_string()
            },
        });
    }
    if aliases.len() != selected.len() {
        return Err(Error::Conflict);
    }
    mdreader_core::device::archive::record(
        destination,
        &directory,
        &identity.export_id.to_string(),
        &identity.manifest_sha256,
        &aliases,
    )
    .map_err(|_| Error::Integrity)?;
    Ok((aliases.len() as u64, format!("{:x}", hash.finalize())))
}
pub(in crate::profile_transition::offline) struct Draft {
    control: Option<ReaderStore>,
    lock: (std::fs::File, mdreader_capture::WindowsObjectStamp),
    root: SourceRoot,
    operation: Uuid,
    native_operation: Uuid,
    snapshot: mdreader_profile::VerifiedSnapshot,
    archive: ConversionArchive,
    launch: mdreader_profile::legacy_windows::LaunchRecord,
}
impl Draft {
    pub(in crate::profile_transition::offline) fn begin(
        original: &mut ClosedOriginal,
        parent: &Path,
        native_operation: Uuid,
    ) -> Result<Self> {
        let limits = Limits::default();
        original.revalidate()?;
        let launch = original.launch_record()?;
        let inventory = original.inventory()?;
        let directory = private_directory(parent, "MDReader-migration-")?;
        let root = SourceRoot::capture(directory).map_err(|_| Error::Storage)?;
        let private =
            mdreader_capture::PrivateDirectory::open(&root.path).map_err(|_| Error::Conflict)?;
        let lock = private
            .open_lock(Path::new("draft.lock"))
            .map_err(|_| Error::Storage)?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| Error::NotQuiescent)?;
        let stamp =
            mdreader_capture::observe_object(&lock, false, true).map_err(|_| Error::Conflict)?;
        let mut control =
            ReaderStore::open_private(&root.path.join("control")).map_err(|_| Error::Storage)?;
        let operation = control.with_library_connection(|db| -> Result<_> {
            let mut journal = Journal::attach(db)?;
            let generation = journal.bootstrap(&inventory)?.generation;
            if generation != 0 {
                return Err(Error::Conflict);
            }
            journal.prepare(&inventory, generation)
        })?;
        control.with_library_connection(|db| {
            super::legacy_record::record(
                db,
                operation,
                &launch,
                super::legacy_record::Draft {
                    native_operation,
                    lock: stamp,
                    conversion: None,
                    selected: None,
                },
            )
        })?;
        let snapshot =
            mdreader_profile::stage(original, inventory, &root.path.join("original"), &limits)?;
        control.with_library_connection(|db| {
            Journal::attach(db)?.preserved(operation, &snapshot, &limits)
        })?;
        let archive =
            original.conversion_archive(&snapshot, &root.path, &root.path.join("conversion"))?;
        control.with_library_connection(|db| {
            super::legacy_record::conversion(
                db,
                operation,
                super::legacy_record::Conversion {
                    relative: "conversion".into(),
                    identity: archive.identity().clone(),
                },
            )
        })?;
        Ok(Self {
            control: Some(control),
            lock: (lock, stamp),
            root,
            operation,
            native_operation,
            snapshot,
            archive,
            launch,
        })
    }
    pub(in crate::profile_transition::offline) fn documents(
        &self,
    ) -> Result<Vec<super::super::commands::Document>> {
        self.verify_lock()?;
        documents(&self.archive)
    }
    pub(in crate::profile_transition::offline) fn finish(
        &mut self,
        original: &mut ClosedOriginal,
        selected: &[Selection],
    ) -> Result<Prepared> {
        let limits = Limits::default();
        self.verify_lock()?;
        original.revalidate()?;
        if original.binding() != &self.snapshot.manifest().original
            || original.inventory()?.identity() != self.snapshot.manifest().inventory_sha256
        {
            return Err(Error::Conflict);
        }
        self.archive.verify()?;
        let control = self.control.as_mut().ok_or(Error::ReconciliationRequired)?;
        let root = &self.root;
        let operation = self.operation;
        let native_operation = self.native_operation;
        let snapshot = &self.snapshot;
        let archive = &self.archive;
        let launch = &self.launch;
        let lock_stamp = self.lock.1;
        let recorded = control
            .with_library_connection(|db| super::legacy_record::read(db)?.ok_or(Error::Conflict))?;
        if recorded.operation != operation
            || &recorded.launch != launch
            || recorded.draft.as_ref().is_none_or(|draft| {
                draft.native_operation != native_operation
                    || draft.lock != lock_stamp
                    || draft
                        .conversion
                        .as_ref()
                        .is_none_or(|conversion| &conversion.identity != archive.identity())
            })
        {
            return Err(Error::Conflict);
        }
        let path = root.path.join("candidate");
        let candidate_exists = path.try_exists().map_err(|_| Error::Conflict)?;
        let choices = documents(archive)?
            .into_iter()
            .map(|document| (document.document_id, document.revision))
            .collect::<std::collections::BTreeMap<_, _>>();
        if selected
            .iter()
            .any(|selection| choices.get(&selection.document_id) != Some(&selection.revision))
        {
            return Err(Error::Conflict);
        }
        control.with_library_connection(|db| {
            super::legacy_record::select(db, operation, selected, candidate_exists)
        })?;
        if candidate_exists {
            let (mut closed, receipt) =
                completed::candidate(root, &recorded, snapshot, archive, None)?;
            control.with_library_connection(|db| -> Result<()> {
                let mut owners = Owners {
                    original,
                    candidate: &mut closed,
                    rollback: None,
                };
                Journal::attach(db)?.staged(operation, &receipt, &mut owners)?;
                marker::record(db, native_operation, &receipt)?;
                owners.revalidate()?;
                root.verify().map_err(|_| Error::Conflict)
            })?;
            return Ok(Prepared {
                control: self.control.take().ok_or(Error::ReconciliationRequired)?,
                candidate: Some(closed),
                root: self.root.clone(),
                operation,
                native_operation,
                candidate_handoff: Default::default(),
                rollback_fence: None,
            });
        }
        let mut committed = None;
        let profile = legacy::migration::archive::prepare(
            archive.path(),
            archive.identity(),
            &path,
            &Default::default(),
            |retained, identity, root| {
                let private = mdreader_capture::PrivateDirectory::create(&root.join("reader-v1"))
                    .map_err(|_| legacy::Error::Integrity)?;
                let writer = private
                    .open_lock(Path::new("reader.lock"))
                    .map_err(|_| legacy::Error::Integrity)?;
                fs2::FileExt::try_lock_exclusive(&writer).map_err(|_| legacy::Error::Conflict)?;
                let json = super::super::super::persistence::JsonOwner::new(
                    SourceRoot::capture(root.to_owned()).map_err(|_| legacy::Error::Conflict)?,
                )
                .map_err(|_| legacy::Error::Integrity)?;
                super::super::legacy_preferences::restore(
                    retained,
                    identity,
                    root,
                    &Default::default(),
                )
                .map_err(|_| legacy::Error::Integrity)?;
                let docs =
                    legacy::catalog::reader::documents(retained, identity, &Default::default())?;
                let mut destination = ReaderStore::open_private(&root.join("reader-v1"))
                    .map_err(|_| legacy::Error::Integrity)?;
                let result = materialize(docs, &mut destination, root, selected, identity)
                    .map_err(|_| legacy::Error::Integrity)?;
                let plan = candidate::prepare_at_database(
                    operation,
                    snapshot,
                    root,
                    Path::new("reader-v1/library.sqlite"),
                    &limits,
                )
                .map_err(|_| legacy::Error::Integrity)?;
                let receipt = destination
                    .with_library_connection(|db| -> Result<_> {
                        let tx = db.transaction().map_err(|_| Error::Storage)?;
                        let receipt = candidate::record(&tx, &plan, result.0, &result.1)?;
                        original.revalidate()?;
                        tx.commit().map_err(|_| Error::ReconciliationRequired)?;
                        Ok(receipt)
                    })
                    .map_err(|_| legacy::Error::Integrity)?;
                committed = Some(receipt);
                drop(destination);
                drop(json);
                drop(writer);
                Ok(result.0)
            },
        )
        .map_err(failure)?;
        archive.verify()?;
        original.revalidate()?;
        let receipt = committed.ok_or(Error::Integrity)?;
        if profile.materialized_records != Some(receipt.imported_records) {
            return Err(Error::Conflict);
        }
        let mut closed = ClosedOwner::acquire(&path)?;
        closed.verify_candidate(&receipt)?;
        control.with_library_connection(|db| -> Result<()> {
            let mut owners = Owners {
                original,
                candidate: &mut closed,
                rollback: None,
            };
            Journal::attach(db)?.staged(operation, &receipt, &mut owners)?;
            let recorded = super::legacy_record::read(db)?.ok_or(Error::Conflict)?;
            if recorded.operation != operation
                || &recorded.launch != launch
                || recorded.draft.as_ref().is_none_or(|draft| {
                    draft.native_operation != native_operation
                        || draft.lock != lock_stamp
                        || draft
                            .conversion
                            .as_ref()
                            .is_none_or(|conversion| &conversion.identity != archive.identity())
                })
            {
                return Err(Error::Conflict);
            }
            marker::record(db, native_operation, &receipt)?;
            owners.revalidate()?;
            root.verify().map_err(|_| Error::Conflict)
        })?;
        Ok(Prepared {
            control: self.control.take().ok_or(Error::ReconciliationRequired)?,
            candidate: Some(closed),
            root: self.root.clone(),
            operation,
            native_operation,
            candidate_handoff: Default::default(),
            rollback_fence: None,
        })
    }
}
