use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub authority: String,
    pub index: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub path: String,
    pub revision: String,
    pub pending: bool,
    pub title: String,
    pub modified_ms: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub source_name: String,
    pub snapshot: Option<Snapshot>,
    /// Unknown for adopted historical derived rows; never fabricated.
    pub indexed_at_ms: Option<i64>,
    pub documents: Vec<Document>,
    pub next_after: Option<String>,
}
