//! Readable recorded review decisions. Never current evidence or replayable edits.
use super::reader::{code, label, ReaderDocument};
use crate::{
    reviews::{ArchiveIdentity, RawReference},
    Artifact, Member, Result, SqlValue,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(super) struct Source<'a> {
    pub identity: &'a ArchiveIdentity,
    pub index: usize,
    pub artifact: &'a Artifact,
    pub records: &'a Member,
    pub raw: &'a Member,
}
pub(super) fn document(
    source: Source<'_>,
    row_id: i64,
    fields: BTreeMap<String, SqlValue>,
    bytes: &[u8],
) -> Result<ReaderDocument> {
    let Source {
        identity,
        index,
        artifact,
        records,
        raw,
    } = source;
    let reference = RawReference {
        archive: identity.clone(),
        artifact_index: index,
        raw_member: artifact.raw.clone(),
        raw_sha256: raw.sha256.clone(),
        records_sha256: records.sha256.clone(),
        row_id: row_id.to_string(),
        row_sha256: crate::hex(&Sha256::digest(bytes)),
    };
    let record_id = crate::hex(&Sha256::digest(serde_json::to_vec(&(
        identity, index, row_id,
    ))?));
    let review = crate::reviews::interpret(&fields, reference, record_id);
    let legacy_id = uuid::Uuid::new_v5(
        &identity.export_id,
        format!("review:{index}:{row_id}").as_bytes(),
    )
    .to_string();
    let name = crate::reviews::text(&fields, "kind")
        .or_else(|| crate::reviews::text(&fields, "schema"))
        .unwrap_or_else(|| "Recorded decision".into());
    let title = format!("Archived review: {}", label(&name));
    let mut markdown = format!("# {}\n\nThis is the recorded review from the original library. It does not approve current content, establish current source access, or replay a write. The actual archived graph is preserved separately.\n\n## Recorded review\n\n{}", label(&title), code(&serde_json::to_value(&review)?)?);
    // Decode ordinary textual fields for reading, while the exact SQL cell
    // bytes and every unknown field remain in the structured/raw archive.
    let mut readable = serde_json::Map::new();
    for (key, cell) in &fields {
        readable.insert(
            key.clone(),
            match cell {
                SqlValue::TextBytes(_) => crate::reviews::text(&fields, key)
                    .map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null),
                SqlValue::Integer(value) => serde_json::Value::String(value.clone()),
                _ => serde_json::to_value(cell)?,
            },
        );
    }
    markdown.push_str(&format!(
        "\n## Original attributes\n\n{}",
        code(&readable.into())?
    ));
    if markdown.len() > 1024 * 1024 {
        return Err(crate::Error::Bounds);
    }
    Ok(ReaderDocument {
        id: uuid::Uuid::new_v5(
            &identity.export_id,
            format!("{index}:{legacy_id}").as_bytes(),
        )
        .to_string(),
        artifact_index: index,
        legacy_id,
        entity: "ArchivedReview".into(),
        title,
        markdown,
        relationships: vec![],
    })
}
