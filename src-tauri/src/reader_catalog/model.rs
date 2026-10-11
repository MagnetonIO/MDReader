use mdreader_service::catalog as service;
use serde::{Deserialize, Serialize};
use specta::Type;
#[derive(Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogError {
    pub kind: String,
}
pub(super) fn failure(kind: &str) -> CatalogError {
    CatalogError { kind: kind.into() }
}
#[derive(Clone, Copy, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CatalogLinkKind {
    Relative,
    Wikilink,
}
#[derive(Deserialize, Type)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CatalogRequest {
    ReadCurrentLocal {
        source_id: String,
        path: String,
    },
    Attachment {
        source_id: String,
        path: String,
        revision: String,
        target: String,
    },
    Image {
        source_id: String,
        path: String,
        revision: String,
        target: String,
    },
    EditLocal {
        source_id: String,
        path: String,
        revision: String,
    },
    List,
    Page {
        catalog_id: String,
        cursor: Option<String>,
        limit: u32,
    },
    Close {
        cursor: String,
    },
    Resolve {
        source_id: String,
        path: String,
        revision: String,
        kind: CatalogLinkKind,
        target: String,
    },
    Read {
        source_id: String,
        path: String,
        revision: String,
    },
}
#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSource {
    pub catalog_id: String,
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub coverage: String,
    pub observed_at_ms: Option<f64>,
    pub failure: Option<String>,
    pub library_source_id: Option<String>,
}
#[derive(Serialize, Type)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum CatalogOpen {
    LocalPending {
        source_id: String,
        path: String,
    },
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
#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogDocument {
    pub name: String,
    pub display_path: String,
    pub modified_at_ms: Option<f64>,
    /// Decimal string preserves provider metadata without JavaScript precision loss.
    pub size_bytes: Option<String>,
    pub observed_at_ms: Option<f64>,
    pub observation: String,
    pub open: CatalogOpen,
}
#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogReference {
    pub source_id: String,
    pub path: String,
    pub revision: String,
    pub line: u32,
}
#[derive(Serialize, Type)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum CatalogReply {
    Attachment {
        source_id: String,
        path: String,
        revision: String,
        target: String,
        preview: crate::reader_assets::AssetPreview,
    },
    Image {
        source_id: String,
        path: String,
        revision: String,
        target: String,
        mime: String,
        width: u32,
        height: u32,
        base64: String,
    },
    EditLocal {
        source_id: String,
        library_source_id: String,
        root: String,
        path: String,
        relative_path: String,
        revision: String,
    },
    List {
        sources: Vec<CatalogSource>,
        provider_failure: Option<String>,
        local_failure: Option<String>,
    },
    Page {
        source: CatalogSource,
        documents: Vec<CatalogDocument>,
        next_cursor: Option<String>,
    },
    ReadDocument {
        source_kind: String,
        source_id: String,
        path: String,
        revision: String,
        markdown: String,
        line_start: u32,
        line_end: u32,
    },
    Resolved {
        references: Vec<CatalogReference>,
    },
    Complete,
}
pub(super) fn error_kind(e: service::CatalogError) -> String {
    match e {
        service::CatalogError::Invalid => "invalid",
        service::CatalogError::Stale => "stale",
        service::CatalogError::Unauthorized => "unauthorized",
        service::CatalogError::Unsupported => "unsupported",
        service::CatalogError::Unavailable => "unavailable",
        service::CatalogError::Persistence => "persistence",
        service::CatalogError::TooLarge => "too_large",
        service::CatalogError::Busy => "busy",
    }
    .into()
}
fn time(t: Option<i64>) -> Option<f64> {
    t.filter(|t| t.unsigned_abs() <= 9_007_199_254_740_991)
        .map(|t| t as f64)
}
impl From<service::CatalogSource> for CatalogSource {
    fn from(s: service::CatalogSource) -> Self {
        Self {
            catalog_id: s.catalog_id,
            name: s.name,
            kind: s.kind,
            enabled: s.enabled,
            coverage: s.coverage,
            observed_at_ms: time(s.observed_at_ms),
            failure: s.failure.map(error_kind),
            library_source_id: None,
        }
    }
}
impl From<service::CatalogDocument> for CatalogDocument {
    fn from(d: service::CatalogDocument) -> Self {
        Self {
            name: d.name,
            display_path: d.display_path,
            modified_at_ms: time(d.modified_at_ms),
            size_bytes: d.size_bytes.map(|n| n.to_string()),
            observed_at_ms: time(d.observed_at_ms),
            observation: d.observation,
            open: match d.open {
                service::CatalogOpen::LocalPending { source_id, path } => {
                    CatalogOpen::LocalPending { source_id, path }
                }
                service::CatalogOpen::Device {
                    document_id,
                    source_id,
                    revision,
                    line,
                } => CatalogOpen::Device {
                    document_id,
                    source_id,
                    revision,
                    line,
                },
                service::CatalogOpen::Exact {
                    source_id,
                    path,
                    revision,
                } => CatalogOpen::Exact {
                    source_id,
                    path,
                    revision,
                },
                service::CatalogOpen::SelectedUncached {
                    source_id,
                    reader_source_id,
                    document_id,
                } => CatalogOpen::SelectedUncached {
                    source_id,
                    reader_source_id,
                    document_id,
                },
            },
        }
    }
}
