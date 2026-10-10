//! Renderer-safe presentation of portable owner observations. No native paths.
use serde::Serialize;
use specta::Type;

macro_rules! coverage_enum {
    ($name:ident, $core:ident, $($variant:ident),+ $(,)?) => {
        #[derive(Debug, Clone, Serialize, Type)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
        impl From<mdreader_core::$core> for $name {
            fn from(value: mdreader_core::$core) -> Self {
                match value { $(mdreader_core::$core::$variant => Self::$variant),+ }
            }
        }
    };
}
coverage_enum!(
    SearchIndexPhaseDto,
    SearchIndexPhase,
    Idle,
    Queued,
    Indexing,
    Loading,
    Embedding,
    Done,
    ModelUnavailable,
    Error
);
coverage_enum!(
    SearchModelStateDto,
    SearchModelState,
    Disabled,
    NotDownloaded,
    Loading,
    Ready,
    Busy,
    Unavailable,
    RestartRequired
);
coverage_enum!(
    SearchMetadataStateDto,
    SearchMetadataState,
    NeverObserved,
    Partial,
    CompleteSelected
);

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SearchSourceKindDto {
    Local,
    Capture,
    Device,
    Provider,
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct SearchMetadataCoverageDto {
    pub state: SearchMetadataStateDto,
    pub snapshot: Option<String>,
    pub known_documents: u64,
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct SearchSourceCoverageDto {
    pub source_id: String,
    pub kind: SearchSourceKindDto,
    pub authority_snapshot: String,
    pub index_snapshot: Option<String>,
    pub observed_at_ms: Option<i64>,
    pub job_phase: SearchIndexPhaseDto,
    pub job_generation: Option<u64>,
    pub indexed_documents: u64,
    pub indexed_chunks: u64,
    pub embedded_chunks: u64,
    pub eligible_documents: Option<u64>,
    pub admitted_bodies: Option<u64>,
    pub metadata: Option<SearchMetadataCoverageDto>,
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct SearchCoverageDto {
    pub scope_token: String,
    pub sources: Vec<SearchSourceCoverageDto>,
    pub model: SearchModelStateDto,
}

impl SearchCoverageDto {
    pub(super) fn from_owner(
        value: mdreader_core::SearchCoverage,
        owned: &mdreader_service::catalog::ReaderOwnedSources,
    ) -> Self {
        Self {
            scope_token: value.scope_token,
            model: value.model.into(),
            sources: value
                .sources
                .into_iter()
                .map(|source| {
                    let kind = if owned
                        .devices
                        .iter()
                        .any(|d| d.source_id == source.source_id)
                    {
                        SearchSourceKindDto::Device
                    } else if owned.captures.iter().any(|d| d.id == source.source_id) {
                        SearchSourceKindDto::Capture
                    } else if source.source_id.starts_with("provider:") {
                        SearchSourceKindDto::Provider
                    } else {
                        SearchSourceKindDto::Local
                    };
                    SearchSourceCoverageDto {
                        source_id: source.source_id,
                        kind,
                        authority_snapshot: source.snapshot.authority,
                        index_snapshot: source.snapshot.index,
                        observed_at_ms: source.snapshot.observed_at_ms,
                        job_phase: source.job_phase.into(),
                        job_generation: source.job_generation,
                        indexed_documents: source.indexed_documents,
                        indexed_chunks: source.indexed_chunks,
                        embedded_chunks: source.embedded_chunks,
                        eligible_documents: source.eligible_documents,
                        admitted_bodies: source.admitted_bodies,
                        metadata: source.metadata.map(|m| SearchMetadataCoverageDto {
                            state: m.state.into(),
                            snapshot: m.snapshot,
                            known_documents: m.known_documents,
                        }),
                    }
                })
                .collect(),
        }
    }
}
