//! MDReader-owned, read-only compatibility DTOs for catdb.ir/v1. Declaration
//! order is part of its canonical hash contract. No runtime SDK or replay API.
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    range: Option<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    schema_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    extractor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    confidence: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approved_by: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    DefineSchema {
        name: String,
        version: String,
        spec: serde_json::Value,
    },
    AssertObject {
        entity: String,
        object_id: Uuid,
        value: serde_json::Value,
        provenance: Provenance,
    },
    RetractObject {
        object_id: Uuid,
    },
    AssertMorphism {
        arrow: String,
        morphism_id: Uuid,
        source: Uuid,
        target: Uuid,
        provenance: Provenance,
    },
    RetractMorphism {
        morphism_id: Uuid,
    },
    AssertMapping {
        name: String,
        spec: serde_json::Value,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: String,
    operation_id: Uuid,
    workspace_id: Uuid,
    branch_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent_version: Option<Uuid>,
    actor: String,
    timestamp: i64,
    operation: Operation,
}
pub(super) fn extend(parent: &str, input: &str) -> Option<String> {
    let parsed: Envelope = serde_json::from_str(input).ok()?;
    if parsed.version != "catdb.ir/v1" {
        return None;
    }
    let canonical = serde_json::to_vec(&parsed).ok()?;
    let mut hash = blake3::Hasher::new();
    hash.update(parent.as_bytes());
    hash.update(&[0]);
    hash.update(&canonical);
    Some(hash.finalize().to_hex().to_string())
}
