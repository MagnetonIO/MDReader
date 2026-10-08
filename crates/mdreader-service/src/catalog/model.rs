//! Catalog metadata contains no filesystem grants, credentials or provider keys.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogError {
    Invalid,
    Stale,
    Unauthorized,
    Unsupported,
    Unavailable,
    Persistence,
    TooLarge,
    Busy,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CatalogRequest {
    List {
        local_sources: Vec<String>,
    },
    Page {
        catalog_id: String,
        cursor: Option<String>,
        limit: u32,
    },
    Close {
        cursor: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogSource {
    pub catalog_id: String,
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub coverage: String,
    pub observed_at_ms: Option<i64>,
    pub failure: Option<CatalogError>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CatalogOpen {
    Device {
        document_id: String,
        source_id: String,
        revision: String,
        line: u32,
    },
    Exact {
        source_id: String,
        path: String,
        revision: String,
    },
    SelectedUncached {
        source_id: String,
        reader_source_id: String,
        document_id: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogDocument {
    pub name: String,
    pub display_path: String,
    pub modified_at_ms: Option<i64>,
    pub size_bytes: Option<u64>,
    pub observed_at_ms: Option<i64>,
    pub observation: String,
    pub open: CatalogOpen,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CatalogReply {
    List {
        sources: Vec<CatalogSource>,
        provider_failure: Option<CatalogError>,
    },
    Page {
        source: CatalogSource,
        documents: Vec<CatalogDocument>,
        next_cursor: Option<String>,
    },
    Complete,
}
pub(super) fn core_error(e: mdreader_core::ReaderError) -> CatalogError {
    use mdreader_core::ReaderError as E;
    match e {
        E::Conflict => CatalogError::Stale,
        E::Source(_) => CatalogError::Unauthorized,
        E::Invalid(_) => CatalogError::Invalid,
        E::Unavailable => CatalogError::Unavailable,
        _ => CatalogError::Persistence,
    }
}
pub(super) fn provider_error(e: mdreader_provider::ProviderError) -> CatalogError {
    use mdreader_provider::ProviderError as E;
    match e {
        E::Conflict => CatalogError::Stale,
        E::Unauthorized => CatalogError::Unauthorized,
        E::Unsupported => CatalogError::Unsupported,
        E::TooLarge => CatalogError::TooLarge,
        E::Invalid => CatalogError::Invalid,
        E::Unavailable | E::NotFound => CatalogError::Unavailable,
        _ => CatalogError::Persistence,
    }
}
