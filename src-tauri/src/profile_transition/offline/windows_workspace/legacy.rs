//! Legacy useful-record materialization uses the real retained original owner.
//! Native activation remains a lifecycle CAS, never direct archive-root launch.
use super::*;
use mdreader_legacy_export::{self as legacy, catalog::reader::ReaderDocument};
use mdreader_profile::legacy_windows::{ClosedOriginal, ConversionArchive};
use sha2::{Digest, Sha256};

pub(super) fn failure(error: legacy::Error) -> Error {
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
pub(super) fn documents(
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
impl Prepared {
    /// Separate immutable raw preservation, useful-record archive and modern
    /// owner construction. Original closure stays held through final CAS fences.
    pub(super) fn prepare_legacy(
        original: &mut ClosedOriginal,
        parent: &Path,
        selected: &[Selection],
        native_operation: Uuid,
    ) -> Result<Self> {
        let limits = Limits::default();
        original.revalidate()?;
        let launch = original.launch_record()?;
        let inventory = original.inventory()?;
        let directory = private_directory(parent, "MDReader-migration-")?;
        let root = SourceRoot::capture(directory).map_err(|_| Error::Storage)?;
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
        let snapshot =
            mdreader_profile::stage(original, inventory, &root.path.join("original"), &limits)?;
        control.with_library_connection(|db| {
            Journal::attach(db)?.preserved(operation, &snapshot, &limits)
        })?;
        let archive =
            original.conversion_archive(&snapshot, &root.path, &root.path.join("conversion"))?;
        let path = root.path.join("candidate");
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
                    &snapshot,
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
            super::legacy_record::record(db, operation, &launch)?;
            marker::record(db, native_operation, &receipt)?;
            owners.revalidate()?;
            root.verify().map_err(|_| Error::Conflict)
        })?;
        Ok(Self {
            control,
            candidate: Some(closed),
            root,
            operation,
            native_operation,
            candidate_handoff: Default::default(),
            rollback_fence: None,
        })
    }
}
