//! Explicit verified-export input. Real reader copies are created in a new
//! fixed private profile; original grants/credentials are never reactivated.
use super::{
    commands::{Coverage, Document},
    *,
};
use mdreader_core::{device, ReaderStore, SourceRoot};
use mdreader_legacy_export::{
    self as legacy, catalog::reader::ReaderDocument, reviews::ArchiveIdentity,
};
use std::collections::HashSet;

#[cfg(test)]
#[path = "archive_import_tests.rs"]
mod tests;

pub(super) fn failure(error: legacy::Error) -> Error {
    match error {
        legacy::Error::Bounds => Error::Bounds,
        legacy::Error::Conflict => Error::Conflict,
        legacy::Error::Unsupported => Error::Unsupported,
        legacy::Error::Invalid => Error::Invalid,
        _ => Error::Integrity,
    }
}
fn name(title: &str) -> String {
    let mut result = String::new();
    for c in title.chars() {
        let c = if c.is_control() || matches!(c, '/' | '\\') {
            '_'
        } else {
            c
        };
        if result.len() + c.len_utf8() > 220 {
            break;
        }
        result.push(c);
    }
    if result.is_empty() {
        result.push_str("Recovered record");
    }
    if !result.to_ascii_lowercase().ends_with(".md") {
        result.push_str(".md");
    }
    result
}
pub(super) struct ArchiveImport {
    root: SourceRoot,
    identity: ArchiveIdentity,
    documents: Vec<ReaderDocument>,
    candidate: Option<SourceRoot>,
}
impl ArchiveImport {
    pub(super) fn preview(path: &Path) -> Result<Self> {
        let root = SourceRoot::capture(path.to_owned()).map_err(|_| Error::Conflict)?;
        let limits = legacy::Limits::default();
        let identity =
            legacy::migration::archive::identity(&root.path, &limits).map_err(failure)?;
        let documents =
            legacy::catalog::reader::documents(&root.path, &identity, &limits).map_err(failure)?;
        root.verify().map_err(|_| Error::Conflict)?;
        Ok(Self {
            root,
            identity,
            documents,
            candidate: None,
        })
    }
    pub(super) fn resume(path: &Path) -> Result<Self> {
        let profile = legacy::migration::verify(path, &Default::default()).map_err(failure)?;
        if profile.version != 2 {
            return Err(Error::Unsupported);
        }
        let mut import = Self::preview(&path.join("legacy-archive"))?;
        import.candidate = Some(SourceRoot::capture(path.to_owned()).map_err(|_| Error::Conflict)?);
        Ok(import)
    }
    pub(super) fn path(&self) -> &Path {
        &self.root.path
    }
    fn validate(&self) -> Result<()> {
        self.root.verify().map_err(|_| Error::Conflict)?;
        if legacy::migration::archive::identity(&self.root.path, &Default::default())
            .map_err(failure)?
            != self.identity
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub(super) fn documents(&self) -> Vec<Document> {
        self.documents
            .iter()
            .map(|d| Document {
                document_id: d.id.clone(),
                name: name(&d.title),
                revision: blake3::hash(d.markdown.as_bytes()).to_hex().to_string(),
                bytes: d.markdown.len() as u32,
                pinned: false,
                line: 1,
            })
            .collect()
    }
    pub(super) fn coverage(&self) -> Result<Coverage> {
        let manifest = legacy::verify(&self.root.path, &Default::default()).map_err(failure)?;
        let history =
            legacy::history::entries(&self.root.path, &self.identity, &Default::default())
                .map_err(failure)?;
        let edges = self
            .documents
            .iter()
            .flat_map(|d| {
                d.relationships
                    .iter()
                    .map(move |r| (d.artifact_index, r.id.as_str()))
            })
            .collect::<HashSet<_>>();
        let reviews = manifest
            .artifacts
            .iter()
            .filter(|a| a.role == legacy::Role::Proposals)
            .map(|a| match a.conversion {
                legacy::Conversion::Known { rows } => rows,
                _ => 0,
            })
            .sum::<u64>();
        let unknown = manifest
            .artifacts
            .iter()
            .filter(|a| matches!(a.conversion, legacy::Conversion::Unavailable { .. }))
            .count();
        Ok(Coverage {
            documents: self.documents.len() as u32,
            relationships: edges.len() as u32,
            reviews: u32::try_from(reviews).map_err(|_| Error::Bounds)?,
            history_entries: history.len() as u32,
            unavailable_history_entries: history.len() as u32,
            settings: manifest
                .artifacts
                .iter()
                .any(|a| a.role == legacy::Role::Settings),
            unknown_records: unknown as u32,
        })
    }
    pub(super) fn export(&self, parent: &Path) -> Result<()> {
        self.validate()?;
        let directory = super::workspace::private_directory(parent, "MDReader-originals-")?;
        legacy::migration::archive::copy(
            &self.root.path,
            &self.identity,
            &directory.join("archive"),
            &Default::default(),
        )
        .map_err(failure)?;
        self.validate()
    }
    pub(super) fn prepare(&mut self, parent: &Path, selected: &[Selection]) -> Result<u32> {
        if self.candidate.is_some() {
            return Err(Error::Conflict);
        }
        self.validate()?;
        let limits = legacy::Limits::default();
        let selected_ids = selected
            .iter()
            .map(|s| (&s.document_id, &s.revision))
            .collect::<std::collections::BTreeMap<_, _>>();
        if selected_ids.len() != selected.len() {
            return Err(Error::Invalid);
        }
        for selection in selected {
            let original = self
                .documents
                .iter()
                .find(|d| d.id == selection.document_id)
                .ok_or(Error::Conflict)?;
            if blake3::hash(original.markdown.as_bytes()).to_hex().as_str() != selection.revision {
                return Err(Error::Conflict);
            }
        }
        let directory = super::workspace::private_directory(parent, "MDReader-import-")?;
        let destination = directory.join("reader-profile");
        let profile = legacy::migration::archive::prepare(
            &self.root.path,
            &self.identity,
            &destination,
            &limits,
            |archive, identity, root| {
                super::legacy_preferences::restore(archive, identity, root, &limits)
                    .map_err(|_| legacy::Error::Integrity)?;
                let docs = legacy::catalog::reader::documents(archive, identity, &limits)?;
                let mut owner = ReaderStore::open(&root.join("reader-v1"))
                    .map_err(|_| legacy::Error::Integrity)?;
                let documents = root.join("reader-v1/device-markdown-v1");
                let mut aliases = Vec::new();
                for doc in docs
                    .into_iter()
                    .filter(|d| selected_ids.contains_key(&d.id))
                {
                    if selected_ids.get(&doc.id).is_none_or(|revision| {
                        blake3::hash(doc.markdown.as_bytes()).to_hex().as_str() != revision.as_str()
                    }) {
                        return Err(legacy::Error::Conflict);
                    }
                    let copied =
                        device::import(&mut owner, &documents, name(&doc.title), doc.markdown)
                            .map_err(|_| legacy::Error::Integrity)?;
                    // Opaque cloud tuple lineage remains complete in its raw typed
                    // record. Only graph UUID aliases become navigable references.
                    aliases.push(device::archive::Alias {
                        receipt: device::catalog::receipt(&copied),
                        artifact_index: doc.artifact_index as u32,
                        legacy_id: if doc.legacy_id.len() <= 16384 {
                            doc.legacy_id
                        } else {
                            blake3::hash(doc.legacy_id.as_bytes()).to_hex().to_string()
                        },
                    });
                }
                device::archive::record(
                    &mut owner,
                    &documents,
                    &identity.export_id.to_string(),
                    &identity.manifest_sha256,
                    &aliases,
                )
                .map_err(|_| legacy::Error::Integrity)?;
                if aliases.len() != selected.len() {
                    return Err(legacy::Error::Conflict);
                }
                Ok(aliases.len() as u64)
            },
        )
        .map_err(failure)?;
        self.validate()?;
        self.candidate = Some(SourceRoot::capture(destination).map_err(|_| Error::Conflict)?);
        Ok(profile.materialized_records.ok_or(Error::Conflict)? as u32)
    }
    pub(super) fn imported(&self) -> Result<u32> {
        let Some(candidate) = &self.candidate else {
            return Ok(0);
        };
        candidate.verify().map_err(|_| Error::Conflict)?;
        let profile =
            legacy::migration::verify(&candidate.path, &Default::default()).map_err(failure)?;
        Ok(profile.materialized_records.ok_or(Error::Conflict)? as u32)
    }
    pub(super) fn launch(&self) -> Result<()> {
        let candidate = self.candidate.as_ref().ok_or(Error::Conflict)?;
        candidate.verify().map_err(|_| Error::Conflict)?;
        legacy::migration::verify(&candidate.path, &Default::default()).map_err(failure)?;
        std::process::Command::new(std::env::current_exe().map_err(|_| Error::Storage)?)
            .arg("--reader-profile")
            .arg(&candidate.path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|_| Error::ReconciliationRequired)?;
        Ok(())
    }
    /// Preserve every candidate write under its real owner locks before return
    /// to the unchanged original. No imported bytes are copied back into it.
    pub(super) fn preserve_candidate(&self) -> Result<()> {
        let candidate = self.candidate.as_ref().ok_or(Error::Conflict)?;
        candidate.verify().map_err(|_| Error::Conflict)?;
        legacy::migration::verify(&candidate.path, &Default::default()).map_err(failure)?;
        let mut closed = super::owner::ClosedOwner::acquire(&candidate.path)?;
        let parent = candidate.path.parent().ok_or(Error::Conflict)?;
        let snapshot = super::workspace::export(&mut closed, parent)?;
        snapshot.revalidate(&Default::default())?;
        closed.revalidate()?;
        candidate.verify().map_err(|_| Error::Conflict)
    }
}
