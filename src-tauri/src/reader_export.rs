//! Explicit desktop Markdown copy. The renderer names only an existing exact
//! reading reference; destination authority comes solely from the native dialog.
//! Existing files are never overwritten (including source files and links).
use crate::{reader::ReaderBackend, reader_catalog, reader_device};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::Path;
use tauri::{AppHandle, State};
mod copy;
pub(crate) mod draft;
#[cfg(test)]
use copy::write_new;
pub(crate) use copy::CopyRuntime;

#[derive(Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportReference {
    pub document_id: Option<String>,
    pub source_id: String,
    pub path: String,
    pub revision: String,
    /// Display-only suggestion; never interpreted as a path or source grant.
    pub filename: Option<String>,
}
impl ExportReference {
    fn validate(&self) -> Result<(), ExportError> {
        if self.source_id.is_empty()
            || self.source_id.len() > 256
            || self.source_id.chars().any(char::is_control)
            || self.revision.len() != 64
            || !self
                .revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.path.is_empty()
            || self.path.len() > 4096
            || self.path.chars().any(char::is_control)
            || Path::new(&self.path)
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(failure("invalid"));
        }
        if self.filename.as_ref().is_some_and(|name| name.len() > 1024) {
            return Err(failure("invalid"));
        }
        if let Some(id) = &self.document_id {
            if self.path != "document.md"
                || !uuid::Uuid::parse_str(id).is_ok_and(|id_value| {
                    id_value.get_version_num() == 4 && id_value.to_string() == *id
                })
            {
                return Err(failure("invalid"));
            }
        }
        Ok(())
    }
}

fn export_filename(reference: &ExportReference) -> String {
    copy_filename(reference.filename.as_deref().unwrap_or(&reference.path))
}
fn copy_filename(selected: &str) -> String {
    let leaf = selected.rsplit(['/', '\\']).next().unwrap_or("document");
    let name: String = leaf
        .chars()
        .take(180)
        .map(|c| {
            if c.is_control() || "<>:\"|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = name.trim_end_matches(['.', ' ']);
    let name = if name.is_empty() { "document" } else { name };
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".markdown") {
        name.to_owned()
    } else {
        format!("{name}.md")
    }
}
#[derive(Debug, Serialize, Type)]
pub struct ExportError {
    pub kind: String,
}
pub(super) fn failure(kind: &str) -> ExportError {
    ExportError { kind: kind.into() }
}
#[derive(Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ExportState {
    Exported,
    Cancelled,
}
#[derive(Serialize, Type)]
pub struct ExportReply {
    pub state: ExportState,
}

/// Reuse the actual native Library membership/provider/device read boundary.
/// This happens AFTER the dialog so revoked or changed sources fail closed.
fn read_exact(
    app: &AppHandle,
    reader: &ReaderBackend,
    reference: ExportReference,
) -> Result<String, ExportError> {
    let markdown = if let Some(document_id) = reference.document_id {
        match reader_device::request(
            reader,
            reader_device::DeviceRequest::Read {
                receipt: reader_device::DeviceReceipt {
                    document_id: document_id.clone(),
                    source_id: reference.source_id.clone(),
                    revision: reference.revision.clone(),
                },
            },
        )
        .map_err(|error| failure(&error.kind))?
        {
            reader_device::DeviceReply::ReadDocument {
                document_id: actual_id,
                source_id,
                path,
                revision,
                markdown,
                ..
            } if actual_id == document_id
                && source_id == reference.source_id
                && path == reference.path
                && revision == reference.revision =>
            {
                markdown
            }
            _ => return Err(failure("stale")),
        }
    } else {
        match reader_catalog::execute(
            app,
            reader,
            reader_catalog::CatalogRequest::Read {
                source_id: reference.source_id.clone(),
                path: reference.path.clone(),
                revision: reference.revision.clone(),
            },
        )
        .map_err(|error| failure(&error.kind))?
        {
            reader_catalog::CatalogReply::ReadDocument {
                source_id,
                path,
                revision,
                markdown,
                ..
            } if source_id == reference.source_id
                && path == reference.path
                && revision == reference.revision =>
            {
                markdown
            }
            _ => return Err(failure("stale")),
        }
    };
    if markdown.len() > 2 * 1024 * 1024 {
        return Err(failure("too_large"));
    }
    Ok(markdown)
}

#[tauri::command]
#[specta::specta]
pub async fn reader_export_document(
    app: AppHandle,
    reader: State<'_, ReaderBackend>,
    reference: ExportReference,
    copies: State<'_, CopyRuntime>,
) -> Result<ExportReply, ExportError> {
    reference.validate()?;
    let filename = export_filename(&reference);
    let reader = reader.inner().clone();
    let lease = copies.claim()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = lease;
        copy::choose_and_write(&app, &filename, || read_exact(&app, &reader, reference))
    })
    .await
    .map_err(|_| failure("write_unconfirmed"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_name_uses_document_hint_and_never_interprets_it_as_a_destination() {
        let mut reference = ExportReference {
            document_id: None,
            source_id: "selected".into(),
            path: "folder/Release notes.markdown".into(),
            revision: "a".repeat(64),
            filename: None,
        };
        assert_eq!(export_filename(&reference), "Release notes.markdown");
        reference.filename = Some("jira-import_tickets_RM-059.md".into());
        assert_eq!(export_filename(&reference), "jira-import_tickets_RM-059.md");
        reference.filename = Some("../../Notes: draft?".into());
        assert_eq!(export_filename(&reference), "Notes_ draft_.md");
    }
    #[test]
    fn export_copies_exact_utf8_and_never_replaces_an_existing_destination() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let original = root.join("original.md");
        let bytes = "\u{feff}# Original\r\n\r\nPreserve UTF-8: λ.\r\n".as_bytes();
        std::fs::write(&original, bytes).unwrap();
        let exported = root.join("copy.md");
        write_new(&exported, bytes).unwrap();
        assert_eq!(std::fs::read(&exported).unwrap(), bytes);
        assert!(write_new(&original, b"replacement").is_err());
        let hardlink = root.join("hardlink.md");
        std::fs::hard_link(&original, &hardlink).unwrap();
        assert!(write_new(&hardlink, b"replacement").is_err());
        #[cfg(unix)]
        {
            let link = root.join("link.md");
            std::os::unix::fs::symlink(&original, &link).unwrap();
            assert!(write_new(&link, b"replacement").is_err());
            assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        }
        assert_eq!(std::fs::read(&original).unwrap(), bytes);
        assert_eq!(std::fs::read(&hardlink).unwrap(), bytes);
    }
}
