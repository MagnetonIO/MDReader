//! Archive history matching on the actual service owner. Source references and
//! old positions stay archival; a choice never activates an old source grant.
use crate::{reader::ReaderBackend, reader_catalog, reader_device};
use mdreader_core::device::archive::history::{Binding, Reference as CoreReference};
use mdreader_service::{device as service, Request as ServiceRequest};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, State};
#[derive(Clone, Deserialize, Serialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImportedHistoryReference {
    document_id: Option<String>,
    source_id: String,
    path: String,
    revision: String,
}
impl From<CoreReference> for ImportedHistoryReference {
    fn from(v: CoreReference) -> Self {
        Self {
            document_id: v.document_id,
            source_id: v.source_id,
            path: v.path,
            revision: v.revision,
        }
    }
}
impl From<ImportedHistoryReference> for CoreReference {
    fn from(v: ImportedHistoryReference) -> Self {
        Self {
            document_id: v.document_id,
            source_id: v.source_id,
            path: v.path,
            revision: v.revision,
        }
    }
}
#[derive(Deserialize, Type)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum HistoryRequest {
    List {
        after_entry_id: Option<String>,
    },
    Bind {
        entry_id: String,
        history_digest: String,
        reference: ImportedHistoryReference,
    },
}
#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    entry_id: String,
    title: String,
    kind: String,
    context: String,
    status: &'static str,
    archived_position: bool,
    reference: Option<ImportedHistoryReference>,
}
#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryReply {
    entries: Vec<Entry>,
    history_digest: String,
    next_after_entry_id: Option<String>,
}
#[derive(Serialize, Type)]
pub(crate) struct HistoryError {
    kind: &'static str,
}
fn failure(kind: &'static str) -> HistoryError {
    HistoryError { kind }
}
#[derive(Deserialize)]
struct Envelope {
    result: Option<service::DeviceReply>,
    error: Option<service::DeviceError>,
}
fn service(
    reader: &ReaderBackend,
    request: service::DeviceRequest,
) -> Result<(Vec<Binding>, String), HistoryError> {
    let reply: Envelope = reader
        .typed(ServiceRequest::Device { request })
        .map_err(|_| failure("unavailable"))?;
    match (reply.result, reply.error) {
        (Some(service::DeviceReply::HistoryBindings { bindings, digest }), None) => {
            Ok((bindings, digest))
        }
        (_, Some(service::DeviceError::Busy)) => Err(failure("busy")),
        (_, Some(service::DeviceError::Stale)) => Err(failure("stale")),
        _ => Err(failure("unavailable")),
    }
}
fn exact(
    app: &AppHandle,
    reader: &ReaderBackend,
    r: &ImportedHistoryReference,
) -> Result<(), HistoryError> {
    if let Some(id) = &r.document_id {
        if r.path != "document.md" {
            return Err(failure("invalid"));
        }
        let reply = reader_device::request(
            reader,
            reader_device::DeviceRequest::Read {
                receipt: reader_device::DeviceReceipt {
                    document_id: id.clone(),
                    source_id: r.source_id.clone(),
                    revision: r.revision.clone(),
                },
            },
        )
        .map_err(|_| failure("stale"))?;
        match reply {
            reader_device::DeviceReply::ReadDocument {
                document_id,
                source_id,
                revision,
                ..
            } if document_id == *id && source_id == r.source_id && revision == r.revision => Ok(()),
            _ => Err(failure("stale")),
        }
    } else {
        let reply = reader_catalog::execute(
            app,
            reader,
            reader_catalog::CatalogRequest::Read {
                source_id: r.source_id.clone(),
                path: r.path.clone(),
                revision: r.revision.clone(),
            },
        )
        .map_err(|_| failure("stale"))?;
        match reply {
            reader_catalog::CatalogReply::ReadDocument {
                source_id,
                path,
                revision,
                ..
            } if source_id == r.source_id && path == r.path && revision == r.revision => Ok(()),
            _ => Err(failure("stale")),
        }
    }
}
#[cfg(unix)]
fn execute(
    app: &AppHandle,
    reader: &ReaderBackend,
    request: HistoryRequest,
) -> Result<HistoryReply, HistoryError> {
    use mdreader_legacy_export as legacy;
    let path = crate::reader_profile::directory(app).map_err(|_| failure("unavailable"))?;
    let limits = legacy::Limits::default();
    if !path
        .join("reader-profile.json")
        .try_exists()
        .map_err(|_| failure("unavailable"))?
    {
        if !matches!(request, HistoryRequest::List { .. }) {
            return Err(failure("unsupported"));
        }
        return Ok(HistoryReply {
            entries: vec![],
            history_digest: blake3::hash(b"mdreader-imported-history-empty-v1")
                .to_hex()
                .to_string(),
            next_after_entry_id: None,
        });
    }
    let profile = legacy::migration::verify(&path, &limits).map_err(|_| failure("unavailable"))?;
    let archive = path.join("legacy-archive");
    let mut entries = legacy::history::entries(&archive, &profile.archive, &limits)
        .map_err(|_| failure("unavailable"))?;
    entries.sort_by(|a, b| a.entry_id.cmp(&b.entry_id));
    let archive_id = profile.archive.export_id.to_string();
    let (mut bindings, binding_digest) = service(
        reader,
        service::DeviceRequest::HistoryBindings {
            archive_id: archive_id.clone(),
            archive_sha256: profile.archive.manifest_sha256.clone(),
        },
    )?;
    let digest = |bindings: &[Binding]| -> Result<String, HistoryError> {
        let bytes = serde_json::to_vec(&(&profile.archive, &entries, bindings))
            .map_err(|_| failure("unavailable"))?;
        Ok(blake3::hash(&bytes).to_hex().to_string())
    };
    let mut after = None;
    let mut only = None;
    match request {
        HistoryRequest::List { after_entry_id } => after = after_entry_id,
        HistoryRequest::Bind {
            entry_id,
            history_digest,
            reference,
        } => {
            if digest(&bindings)? != history_digest
                || !entries.iter().any(|e| e.entry_id == entry_id)
            {
                return Err(failure("stale"));
            }
            exact(app, reader, &reference)?;
            let result = service(
                reader,
                service::DeviceRequest::BindHistory {
                    archive_id: archive_id.clone(),
                    archive_sha256: profile.archive.manifest_sha256.clone(),
                    entry_id: entry_id.clone(),
                    expected_digest: binding_digest.clone(),
                    reference: reference.clone().into(),
                },
            )?;
            bindings = result.0;
            exact(app, reader, &reference)?;
            only = Some(entry_id);
        }
    }
    if after
        .as_ref()
        .is_some_and(|id| !entries.iter().any(|e| &e.entry_id == id))
    {
        return Err(failure("stale"));
    }
    let history_digest = digest(&bindings)?;
    let mut page = entries
        .iter()
        .filter(|e| {
            after.as_ref().is_none_or(|after| e.entry_id > *after)
                && only.as_ref().is_none_or(|only| &e.entry_id == only)
        })
        .take(101)
        .map(|e| {
            let reference = bindings
                .iter()
                .find(|b| b.entry_id == e.entry_id)
                .map(|b| ImportedHistoryReference::from(b.reference.clone()));
            Entry {
                entry_id: e.entry_id.clone(),
                title: e.title.clone(),
                kind: e.kind.clone(),
                context: e.context.clone(),
                status: if reference.is_some() {
                    "bound"
                } else {
                    "unresolved"
                },
                archived_position: e.archived_position,
                reference,
            }
        })
        .collect::<Vec<_>>();
    let next_after_entry_id = if page.len() > 100 {
        page.truncate(100);
        page.last().map(|e| e.entry_id.clone())
    } else {
        None
    };
    if legacy::migration::archive::identity(&archive, &limits)
        .map_err(|_| failure("unavailable"))?
        != profile.archive
    {
        return Err(failure("stale"));
    }
    Ok(HistoryReply {
        entries: page,
        history_digest,
        next_after_entry_id,
    })
}
#[cfg(not(unix))]
fn execute(
    _: &AppHandle,
    _: &ReaderBackend,
    _: HistoryRequest,
) -> Result<HistoryReply, HistoryError> {
    Err(failure("unsupported"))
}
#[tauri::command]
#[specta::specta]
pub(crate) async fn native_profile_imported_history(
    app: AppHandle,
    reader: State<'_, ReaderBackend>,
    request: HistoryRequest,
) -> Result<HistoryReply, HistoryError> {
    let reader = reader.inner().clone();
    tauri::async_runtime::spawn_blocking(move || execute(&app, &reader, request))
        .await
        .map_err(|_| failure("unavailable"))?
}
