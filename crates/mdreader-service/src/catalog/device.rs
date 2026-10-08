use super::*;
use std::path::Path;

pub(super) fn service_error(error: crate::device::DeviceError) -> CatalogError {
    use crate::device::DeviceError as E;
    match error {
        E::Busy => CatalogError::Busy,
        E::Stale => CatalogError::Stale,
        E::Unsupported | E::Unconfigured => CatalogError::Unsupported,
        E::Invalid => CatalogError::Invalid,
        E::Persistence => CatalogError::Persistence,
        E::Unavailable => CatalogError::Unavailable,
    }
}

pub(super) fn error(error: mdreader_core::device::Error) -> CatalogError {
    use mdreader_core::device::Error as E;
    match error {
        E::UnsupportedSchema => CatalogError::Unsupported,
        E::InvalidSchema | E::Storage => CatalogError::Persistence,
        E::Conflict => CatalogError::Stale,
        E::InvalidSelection => CatalogError::Invalid,
        _ => CatalogError::Unauthorized,
    }
}
pub(super) fn header(failure: Option<CatalogError>) -> CatalogSource {
    CatalogSource {
        catalog_id: "device:library".into(),
        name: "On this device".into(),
        kind: "device".into(),
        enabled: failure.is_none(),
        coverage: if failure.is_some() {
            "unavailable"
        } else {
            "published_device_metadata"
        }
        .into(),
        observed_at_ms: None,
        failure,
    }
}
pub(super) fn source(store: &ReaderStore, directory: &Path) -> Option<CatalogSource> {
    match mdreader_core::device::catalog::page_in(store, directory, None, None, 1) {
        Ok(page) if page.snapshot.is_none() => None,
        Ok(_) => Some(header(None)),
        Err(e) => Some(header(Some(error(e)))),
    }
}
pub(super) fn document(document: mdreader_core::device::DeviceDocument) -> CatalogDocument {
    CatalogDocument {
        name: document.name.clone(),
        display_path: document.name,
        modified_at_ms: None,
        size_bytes: Some(u64::from(document.bytes)),
        observed_at_ms: None,
        observation: "published_device".into(),
        open: CatalogOpen::Device {
            document_id: document.id,
            source_id: document.source_id,
            revision: document.revision,
            line: document.line,
        },
    }
}
