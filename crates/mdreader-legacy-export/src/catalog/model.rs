use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Exact SQL cells retain unknown attributes and byte encodings alongside the
/// decoded reader projection. They never authorize an original source or write.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Cell {
    Null,
    Integer(String),
    RealBits(String),
    TextBytes(String),
    BlobBytes(String),
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "recordType", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArchivedRecord {
    Object {
        row_id: String,
        object_id: String,
        entity: String,
        value: serde_json::Value,
        provenance: serde_json::Value,
        fields: BTreeMap<String, Cell>,
    },
    Morphism {
        row_id: String,
        morphism_id: String,
        arrow: String,
        source: String,
        target: String,
        provenance: serde_json::Value,
        fields: BTreeMap<String, Cell>,
    },
    History {
        table: String,
        row_id: String,
        fields: BTreeMap<String, Cell>,
    },
    Operation {
        row_id: String,
        /// Original JSON TEXT is retained byte-for-byte, independently of the
        /// typed canonical encoding used only for the chain check.
        envelope: String,
        fields: BTreeMap<String, Cell>,
    },
    Chain {
        observed_count: String,
        computed_digest: Option<String>,
        stored_count: Option<String>,
        stored_digest: Option<String>,
        /// Unkeyed integrity only. Never an authenticated provenance assertion.
        status: ChainStatus,
    },
    Unavailable {
        table: String,
        row_id: String,
    },
    Scope {
        supported_tables: Vec<String>,
        raw_only_tables: Vec<String>,
    },
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainStatus {
    MatchesStoredAnchor,
    Mismatch,
    NoIndependentAnchor,
    UnsupportedEnvelope,
}
impl std::fmt::Debug for ArchivedRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ArchivedRecord { [redacted] }")
    }
}
