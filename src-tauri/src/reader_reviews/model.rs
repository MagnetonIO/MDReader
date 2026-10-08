use mdreader_service::reviews as service;
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Clone, Deserialize, Serialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewCurrent {
    pub source_id: String,
    pub path: String,
    pub revision: String,
}
#[derive(Deserialize, Type)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ArchivedReviewsRequest {
    Select {
        export_id: String,
        manifest_sha256: String,
    },
    List {
        handle: String,
        offset: u32,
        limit: u32,
    },
    Get {
        handle: String,
        record_id: String,
    },
    Current {
        path: String,
        generation: u32,
    },
    Compare {
        handle: String,
        record_id: String,
        current: ReviewCurrent,
        generation: Option<u32>,
    },
    Close {
        handle: String,
    },
}
#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedReviewRecord {
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
impl From<service::ReviewRecord> for ArchivedReviewRecord {
    fn from(v: service::ReviewRecord) -> Self {
        Self {
            record_id: v.record_id,
            row_id: v.row_id,
            artifact_index: v.artifact_index,
            row_sha256: v.row_sha256,
            state: v.state,
            reviewer: v.reviewer,
            recorded_content_hash: v.recorded_content_hash,
            reviewed_content_hash: v.reviewed_content_hash,
            recorded_hash_comparison: v.recorded_hash_comparison,
            graph_membership: v.graph_membership,
            chronology: v.chronology,
            schema: v.schema,
            extractor: v.extractor,
            rule: v.rule,
            qualified: v.qualified,
            reading: v.reading,
            start_line: v.start_line,
            end_line: v.end_line,
            unavailable_reason: v.unavailable_reason,
        }
    }
}
#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedReviewComparison {
    pub review: ArchivedReviewRecord,
    pub current: ReviewCurrent,
    pub observation: String,
    pub current_span_hash: String,
    pub current_passage: String,
    pub recorded_current: String,
    pub reviewed_current: String,
}
#[derive(Serialize, Type)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ArchivedReviewsReply {
    Selected {
        handle: String,
        export_id: String,
        manifest_sha256: String,
        owner_scope: String,
    },
    Listed {
        records: Vec<ArchivedReviewRecord>,
        next_offset: Option<u32>,
    },
    Record {
        record: ArchivedReviewRecord,
    },
    Compared {
        comparison: ArchivedReviewComparison,
    },
    Current {
        reference: ReviewCurrent,
    },
    Closed,
    Cancelled,
}
#[derive(Debug, Serialize, Type)]
pub struct ArchivedReviewsError {
    pub kind: String,
}
impl From<service::ReviewError> for ArchivedReviewsError {
    fn from(error: service::ReviewError) -> Self {
        Self {
            kind: match error {
                service::ReviewError::Busy => "busy",
                service::ReviewError::Invalid => "invalid",
                service::ReviewError::Unavailable => "unavailable",
                service::ReviewError::Conflict => "conflict",
                service::ReviewError::Unsupported => "unsupported",
                service::ReviewError::TooLarge => "too_large",
                service::ReviewError::Storage => "storage",
            }
            .into(),
        }
    }
}
pub(super) fn failure(kind: &str) -> ArchivedReviewsError {
    ArchivedReviewsError { kind: kind.into() }
}
pub(super) fn reply(
    value: service::ReviewReply,
) -> Result<ArchivedReviewsReply, ArchivedReviewsError> {
    Ok(match value {
        service::ReviewReply::Selected {
            handle,
            identity,
            owner_scope,
        } => ArchivedReviewsReply::Selected {
            handle,
            export_id: identity.export_id.to_string(),
            manifest_sha256: identity.manifest_sha256,
            owner_scope,
        },
        service::ReviewReply::Listed { page } => ArchivedReviewsReply::Listed {
            records: page.records.into_iter().map(Into::into).collect(),
            next_offset: page
                .next_offset
                .map(u32::try_from)
                .transpose()
                .map_err(|_| failure("too_large"))?,
        },
        service::ReviewReply::Record { record } => ArchivedReviewsReply::Record {
            record: record.into(),
        },
        service::ReviewReply::Compared { comparison: v } => ArchivedReviewsReply::Compared {
            comparison: ArchivedReviewComparison {
                review: v.review.into(),
                current: ReviewCurrent {
                    source_id: v.current.source_id,
                    path: v.current.path,
                    revision: v.current.revision,
                },
                observation: v.observation,
                current_span_hash: v.current_span_hash,
                current_passage: v.current_passage,
                recorded_current: v.recorded_current,
                reviewed_current: v.reviewed_current,
            },
        },
        service::ReviewReply::Closed => ArchivedReviewsReply::Closed,
    })
}
