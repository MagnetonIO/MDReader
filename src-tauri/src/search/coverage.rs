//! Renderer-safe presentation of portable owner observations. No native paths.
use serde::Serialize;
use specta::Type;
use specta_typescript::Number;

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
    #[specta(type = Number)]
    pub known_documents: f64,
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct SearchSourceCoverageDto {
    pub source_id: String,
    pub kind: SearchSourceKindDto,
    pub authority_snapshot: String,
    pub index_snapshot: Option<String>,
    #[specta(type = Option<Number>)]
    pub observed_at_ms: Option<f64>,
    pub job_phase: SearchIndexPhaseDto,
    #[specta(type = Option<Number>)]
    pub job_generation: Option<f64>,
    #[specta(type = Number)]
    pub indexed_documents: f64,
    #[specta(type = Number)]
    pub indexed_chunks: f64,
    #[specta(type = Number)]
    pub embedded_chunks: f64,
    #[specta(type = Option<Number>)]
    pub eligible_documents: Option<f64>,
    #[specta(type = Option<Number>)]
    pub admitted_bodies: Option<f64>,
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
    ) -> Result<Self, String> {
        Ok(Self {
            scope_token: value.scope_token,
            model: value.model.into(),
            sources: value
                .sources
                .into_iter()
                .map(|source| -> Result<SearchSourceCoverageDto, String> {
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
                    Ok(SearchSourceCoverageDto {
                        source_id: source.source_id,
                        kind,
                        authority_snapshot: source.snapshot.authority,
                        index_snapshot: source.snapshot.index,
                        observed_at_ms: source
                            .snapshot
                            .observed_at_ms
                            .map(|at| {
                                number(u64::try_from(at).map_err(|_| "Invalid coverage timestamp")?)
                            })
                            .transpose()?,
                        job_phase: source.job_phase.into(),
                        job_generation: source.job_generation.map(number).transpose()?,
                        indexed_documents: number(source.indexed_documents)?,
                        indexed_chunks: number(source.indexed_chunks)?,
                        embedded_chunks: number(source.embedded_chunks)?,
                        eligible_documents: source.eligible_documents.map(number).transpose()?,
                        admitted_bodies: source.admitted_bodies.map(number).transpose()?,
                        metadata: source
                            .metadata
                            .map(|m| -> Result<SearchMetadataCoverageDto, String> {
                                Ok(SearchMetadataCoverageDto {
                                    state: m.state.into(),
                                    snapshot: m.snapshot,
                                    known_documents: number(m.known_documents)?,
                                })
                            })
                            .transpose()?,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

// These numeric fields are constructed only through this checked adapter.
// The exported Number type excludes the nonfinite/null values permitted by raw f64.
fn number(value: u64) -> Result<f64, String> {
    if value > 9_007_199_254_740_991 {
        return Err("Search coverage exceeds the exact numeric transport bound".into());
    }
    Ok(value as f64)
}

#[cfg(test)]
mod tests {
    use super::number;
    #[test]
    fn numeric_transport_refuses_inexact_counts_instead_of_rounding() {
        assert_eq!(number(0).unwrap(), 0.0);
        assert_eq!(
            number(9_007_199_254_740_991).unwrap(),
            9_007_199_254_740_991.0
        );
        assert!(number(9_007_199_254_740_992).is_err());
        assert!(number(u64::MAX).is_err());
    }
}
