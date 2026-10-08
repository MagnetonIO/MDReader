use mdreader_legacy_export::reviews::{ArchiveIdentity, ArchivedReview, Assertion};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Deserialize, Serialize)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ReviewRequest {
    Select {
        stage: PathBuf,
        identity: ArchiveIdentity,
    },
    List {
        handle: String,
        offset: u64,
        limit: u32,
    },
    Get {
        handle: String,
        record_id: String,
    },
    Compare {
        handle: String,
        record_id: String,
        source_id: String,
        path: String,
        revision: String,
    },
    Close {
        handle: String,
    },
}
impl std::fmt::Debug for ReviewRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReviewRequest(<explicit private selection>)")
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewError {
    Busy,
    Invalid,
    Unavailable,
    Conflict,
    Unsupported,
    TooLarge,
    Storage,
}
impl std::fmt::Display for ReviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Busy => "An archival review operation is already active",
            Self::Invalid => "Invalid explicit review selection",
            Self::Unavailable => "Selected archive or current document unavailable",
            Self::Conflict => "Selected archive, profile or current revision changed",
            Self::Unsupported => "Selected archive comparison is unsupported",
            Self::TooLarge => "Review selection exceeds supported bounds",
            Self::Storage => "Review storage unavailable",
        })
    }
}
impl std::error::Error for ReviewError {}
pub type Result<T> = std::result::Result<T, ReviewError>;

/// Bounded presentation only. Original locators/payload bytes remain in the
/// retained archive; no archived path crosses this boundary as a document grant.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRecord {
    pub record_id: String,
    pub row_id: String,
    pub artifact_index: u32,
    pub row_sha256: String,
    pub state: String,
    pub reviewer: Option<String>,
    pub recorded_content_hash: Option<String>,
    pub reviewed_content_hash: Option<String>,
    pub recorded_hash_comparison: String,
    pub graph_membership: String,
    pub chronology: String,
    pub schema: Option<String>,
    pub extractor: Option<String>,
    pub rule: Option<String>,
    pub qualified: Option<String>,
    pub reading: Option<String>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub unavailable_reason: Option<String>,
}
impl TryFrom<ArchivedReview> for ReviewRecord {
    type Error = ReviewError;
    fn try_from(row: ArchivedReview) -> Result<Self> {
        use mdreader_legacy_export::reviews::{
            ArchivedState, RecordedHashComparison, UnavailableAssertion,
        };
        let mut result = Self {
            record_id: row.record_id,
            row_id: row.raw.row_id,
            artifact_index: u32::try_from(row.raw.artifact_index)
                .map_err(|_| ReviewError::TooLarge)?,
            row_sha256: row.raw.row_sha256,
            state: match row.state {
                ArchivedState::Approved => "approved",
                ArchivedState::Rejected => "rejected",
                ArchivedState::Proposed => "proposed",
                ArchivedState::Unknown => "unknown",
            }
            .into(),
            reviewer: row.reviewer,
            recorded_content_hash: row.recorded_content_hash,
            reviewed_content_hash: row.reviewed_content_hash,
            recorded_hash_comparison: match row.recorded_hash_comparison {
                RecordedHashComparison::Equal => "equal",
                RecordedHashComparison::Differ => "differ",
                RecordedHashComparison::Unknown => "unknown",
            }
            .into(),
            graph_membership: "unavailable".into(),
            chronology: "unavailable".into(),
            schema: None,
            extractor: None,
            rule: None,
            qualified: None,
            reading: None,
            start_line: None,
            end_line: None,
            unavailable_reason: None,
        };
        match row.assertion {
            Assertion::DocumentSpan(span) => {
                result.schema = Some(span.schema);
                result.extractor = Some(span.extractor);
                result.rule = Some(span.rule);
                result.qualified = Some(span.qualified);
                result.reading = span
                    .value
                    .map(|v| serde_json::to_string(&v))
                    .transpose()
                    .map_err(|_| ReviewError::Storage)?;
                result.start_line = Some(span.start_line);
                result.end_line = Some(span.end_line);
            }
            Assertion::Unavailable { reason } => {
                result.unavailable_reason = Some(
                    match reason {
                        UnavailableAssertion::MalformedAssertion => "malformed_assertion",
                        UnavailableAssertion::UnsupportedSchemaOrRule => {
                            "unsupported_schema_or_rule"
                        }
                        UnavailableAssertion::UnknownReviewState => "unknown_review_state",
                    }
                    .into(),
                )
            }
        }
        Ok(result)
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPage {
    pub records: Vec<ReviewRecord>,
    pub next_offset: Option<u64>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentReference {
    pub source_id: String,
    pub path: String,
    pub revision: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewComparison {
    pub review: ReviewRecord,
    pub current: CurrentReference,
    pub observation: String,
    pub current_span_hash: String,
    pub current_passage: String,
    pub recorded_current: String,
    pub reviewed_current: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ReviewReply {
    Selected {
        handle: String,
        identity: ArchiveIdentity,
        owner_scope: String,
    },
    Listed {
        page: ReviewPage,
    },
    Record {
        record: ReviewRecord,
    },
    Compared {
        comparison: ReviewComparison,
    },
    Closed,
}
pub(super) fn archive_error(error: mdreader_legacy_export::Error) -> ReviewError {
    use mdreader_legacy_export::Error;
    match error {
        Error::Unsupported => ReviewError::Unsupported,
        Error::Invalid => ReviewError::Invalid,
        Error::Bounds => ReviewError::TooLarge,
        Error::Conflict => ReviewError::Conflict,
        _ => ReviewError::Unavailable,
    }
}
