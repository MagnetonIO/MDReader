//! Archival document insights, never source grants or current graph evidence.
use crate::{Error, Limits, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchiveIdentity {
    pub export_id: uuid::Uuid,
    pub manifest_sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawReference {
    pub archive: ArchiveIdentity,
    pub artifact_index: usize,
    pub raw_member: String,
    pub raw_sha256: String,
    pub records_sha256: String,
    /// Decimal string avoids changing an int64 rowid in JavaScript.
    pub row_id: String,
    /// Hash of the exact exported JSON record, excluding its final newline.
    pub row_sha256: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchivedState {
    Proposed,
    Approved,
    Rejected,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordedHashComparison {
    Equal,
    Differ,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableAssertion {
    UnsupportedSchemaOrRule,
    MalformedAssertion,
    UnknownReviewState,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Assertion {
    DocumentSpan(Box<DocumentSpan>),
    Unavailable { reason: UnavailableAssertion },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSpan {
    pub fact_id: String,
    pub schema: String,
    pub extractor: String,
    pub rule: String,
    pub qualified: String,
    pub fact_kind: String,
    /// Opaque legacy locator. Never opened or joined to a filesystem root.
    pub source_locator: String,
    /// Inclusive, 1-based MDReader extraction range.
    pub start_line: u32,
    pub end_line: u32,
    pub content_hash: String,
    pub value: Option<serde_json::Value>,
    pub endpoint_src: Option<String>,
    pub endpoint_dst: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphMembership {
    Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewChronology {
    Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrentEvidence {
    SourceBindingRequired,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchivedReview {
    pub record_id: String,
    pub raw: RawReference,
    pub state: ArchivedState,
    pub reviewer: Option<String>,
    pub reviewed_content_hash: Option<String>,
    pub recorded_content_hash: Option<String>,
    pub recorded_hash_comparison: RecordedHashComparison,
    pub assertion: Assertion,
    pub graph_membership: GraphMembership,
    pub chronology: ReviewChronology,
    pub current_evidence: CurrentEvidence,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewPage {
    pub records: Vec<ArchivedReview>,
    pub next_offset: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageReceipt {
    pub archive: ArchiveIdentity,
    pub records: u64,
    pub unavailable_artifacts: usize,
    pub replayed: bool,
}
#[cfg(any(unix, windows))]
#[path = "reviews_store.rs"]
mod store;
#[cfg(any(unix, windows))]
pub use store::{stage_reviews, ReviewArchive};
#[cfg(not(any(unix, windows)))]
pub fn stage_reviews(_: &Path, _: &ArchiveIdentity, _: &Path, _: &Limits) -> Result<StageReceipt> {
    Err(Error::Unsupported)
}
#[cfg(not(any(unix, windows)))]
pub struct ReviewArchive;
#[cfg(not(any(unix, windows)))]
impl ReviewArchive {
    pub fn open(_: &Path, _: &ArchiveIdentity, _: &Limits) -> Result<Self> {
        Err(Error::Unsupported)
    }
    pub fn list(&self, _: u64, _: usize) -> Result<ReviewPage> {
        Err(Error::Unsupported)
    }
    pub fn get(&self, _: &str) -> Result<Option<ArchivedReview>> {
        Err(Error::Unsupported)
    }
    pub fn raw_record(&self, _: &str) -> Result<Option<Vec<u8>>> {
        Err(Error::Unsupported)
    }
}

#[cfg(any(unix, windows))]
pub(crate) fn text(
    fields: &std::collections::BTreeMap<String, crate::SqlValue>,
    name: &str,
) -> Option<String> {
    let crate::SqlValue::TextBytes(encoded) = fields.get(name)? else {
        return None;
    };
    if !encoded.len().is_multiple_of(2) {
        return None;
    }
    let bytes = encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |b: u8| (b as char).to_digit(16).map(|x| x as u8);
            Some(digit(pair[0])? * 16 + digit(pair[1])?)
        })
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}
#[cfg(any(unix, windows))]
fn valid_hash(value: &str) -> bool {
    value
        .strip_prefix("blake3:")
        .is_some_and(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
}
#[cfg(any(unix, windows))]
pub(crate) fn interpret(
    fields: &std::collections::BTreeMap<String, crate::SqlValue>,
    raw: RawReference,
    record_id: String,
) -> ArchivedReview {
    let state = match text(fields, "state").as_deref() {
        Some("proposed") => ArchivedState::Proposed,
        Some("approved") => ArchivedState::Approved,
        Some("rejected") => ArchivedState::Rejected,
        _ => ArchivedState::Unknown,
    };
    let reviewer = text(fields, "reviewer").filter(|s| s.len() <= 4096);
    let reviewed = text(fields, "reviewed_hash").filter(|s| s.len() <= 256);
    let recorded = text(fields, "content_hash").filter(|s| s.len() <= 256);
    let comparison = match (&reviewed, &recorded) {
        (Some(a), Some(b)) if valid_hash(a) && valid_hash(b) => {
            if a.eq_ignore_ascii_case(b) {
                RecordedHashComparison::Equal
            } else {
                RecordedHashComparison::Differ
            }
        }
        _ => RecordedHashComparison::Unknown,
    };
    let assertion = if state == ArchivedState::Unknown {
        Assertion::Unavailable {
            reason: UnavailableAssertion::UnknownReviewState,
        }
    } else if state != ArchivedState::Proposed
        && reviewer.as_ref().is_none_or(|r| r.trim().is_empty())
    {
        Assertion::Unavailable {
            reason: UnavailableAssertion::MalformedAssertion,
        }
    } else {
        match candidate(fields) {
            Ok(candidate) => candidate,
            Err(reason) => Assertion::Unavailable { reason },
        }
    };
    ArchivedReview {
        record_id,
        raw,
        state,
        reviewer,
        reviewed_content_hash: reviewed,
        recorded_content_hash: recorded,
        recorded_hash_comparison: comparison,
        assertion,
        graph_membership: GraphMembership::Unavailable,
        chronology: ReviewChronology::Unavailable,
        current_evidence: CurrentEvidence::SourceBindingRequired,
    }
}
#[cfg(any(unix, windows))]
fn candidate(
    fields: &std::collections::BTreeMap<String, crate::SqlValue>,
) -> std::result::Result<Assertion, UnavailableAssertion> {
    use UnavailableAssertion::{MalformedAssertion as Bad, UnsupportedSchemaOrRule as Unsupported};
    let required = |name: &str, max: usize| {
        text(fields, name)
            .filter(|v| !v.is_empty() && v.len() <= max)
            .ok_or(Bad)
    };
    let schema = required("schema_name", 128)?;
    let extractor = required("extractor", 128)?;
    let rule = required("rule", 32)?;
    let qualified = required("qualified", 256)?;
    let kind = required("kind", 32)?;
    let expected_extractor = match schema.as_str() {
        "decision_overlay" => "mdreader.decision@1",
        "architecture_overlay" => "mdreader.architecture@1",
        _ => return Err(Unsupported),
    };
    if extractor != expected_extractor {
        return Err(Unsupported);
    }
    let suffix = qualified
        .strip_prefix(&format!("{schema}."))
        .ok_or(Unsupported)?;
    let supported = matches!(
        (schema.as_str(), rule.as_str(), kind.as_str(), suffix),
        ("decision_overlay", "D1" | "D2", "object", "Decision")
            | ("decision_overlay", "D3", "morphism", "supersedes")
            | ("architecture_overlay", "A1", "object", "Component")
            | ("architecture_overlay", "A2", "object", "Layer")
            | ("architecture_overlay", "A2", "morphism", "inLayer")
            | ("architecture_overlay", "A3", "object", "Interface")
            | ("architecture_overlay", "A3", "morphism", "exposes")
            | ("architecture_overlay", "A4", "object", "Dependency")
            | (
                "architecture_overlay",
                "A4",
                "morphism",
                "dependant" | "provider" | "via"
            )
    );
    if !supported {
        return Err(Unsupported);
    }
    let fact_id = required("fact_id", 64)?;
    uuid::Uuid::parse_str(&fact_id).map_err(|_| Bad)?;
    let source = required("source_path", 16384)?;
    let line = |name: &str| match fields.get(name) {
        Some(crate::SqlValue::Integer(n)) => n.parse::<u32>().ok().filter(|n| *n > 0).ok_or(Bad),
        _ => Err(Bad),
    };
    let start = line("start_line")?;
    let end = line("end_line")?;
    if end < start {
        return Err(Bad);
    }
    let hash = required("content_hash", 256)?;
    if !valid_hash(&hash) {
        return Err(Bad);
    }
    let payload: serde_json::Value =
        serde_json::from_str(&required("payload", 4 * 1024 * 1024)?).map_err(|_| Bad)?;
    let p = payload.get("provenance").ok_or(Bad)?;
    if payload.get("id").and_then(|v| v.as_str()) != Some(&fact_id)
        || p.get("source").and_then(|v| v.as_str()) != Some(&source)
        || p.get("range") != Some(&serde_json::json!([start, end]))
        || p.get("content_hash").and_then(|v| v.as_str()) != Some(&hash)
        || p.get("extractor").and_then(|v| v.as_str()) != Some(&extractor)
        || p.get("schema_version").and_then(|v| v.as_str()) != Some("1")
        || !p
            .get("confidence")
            .and_then(|v| v.as_f64())
            .is_some_and(|n| n.is_finite() && (0.0..=1.0).contains(&n))
    {
        return Err(Bad);
    }
    let (value, endpoint_src, endpoint_dst) = if kind == "object" {
        if payload.get("entity").and_then(|v| v.as_str()) != Some(&qualified) {
            return Err(Bad);
        }
        let value = payload.get("value").filter(|v| v.is_object()).ok_or(Bad)?;
        let attribute = match suffix {
            "Decision" => "decision_title",
            "Component" => "component_name",
            "Layer" => "layer_name",
            "Interface" => "interface_name",
            "Dependency" => "dependency_kind",
            _ => return Err(Bad),
        };
        if value.get(attribute).and_then(|v| v.as_str()).is_none() {
            return Err(Bad);
        }
        if suffix == "Component"
            && value
                .get("component_kind")
                .and_then(|v| v.as_str())
                .is_none()
        {
            return Err(Bad);
        }
        if suffix == "Dependency" && value["dependency_kind"] != "depends_on" {
            return Err(Bad);
        }
        (Some(value.clone()), None, None)
    } else {
        let a = required("endpoint_src", 64)?;
        let b = required("endpoint_dst", 64)?;
        uuid::Uuid::parse_str(&a).map_err(|_| Bad)?;
        uuid::Uuid::parse_str(&b).map_err(|_| Bad)?;
        if payload.get("arrow").and_then(|v| v.as_str()) != Some(&qualified)
            || payload.get("source").and_then(|v| v.as_str()) != Some(&a)
            || payload.get("target").and_then(|v| v.as_str()) != Some(&b)
        {
            return Err(Bad);
        }
        (None, Some(a), Some(b))
    };
    Ok(Assertion::DocumentSpan(Box::new(DocumentSpan {
        fact_id,
        schema,
        extractor,
        rule,
        qualified,
        fact_kind: kind,
        source_locator: source,
        start_line: start,
        end_line: end,
        content_hash: hash,
        value,
        endpoint_src,
        endpoint_dst,
    })))
}

#[cfg(all(test, unix))]
#[path = "reviews_tests.rs"]
mod tests;
