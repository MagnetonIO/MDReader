//! Readable insight projections from VERIFIED archives. IDs are scoped by input
//! artifact, never joined across unrelated databases by matching names/UUIDs.
use super::ArchivedRecord;
use crate::{reviews::ArchiveIdentity, Error, KnownRecord, Limits, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::Path,
};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Relationship {
    pub id: String,
    pub arrow: String,
    pub source: String,
    pub target: String,
    pub provenance: serde_json::Value,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReaderDocument {
    /// UUIDv5 of export identity + input ordinal + stable original object ID.
    pub id: String,
    pub artifact_index: usize,
    pub legacy_id: String,
    pub entity: String,
    pub title: String,
    pub markdown: String,
    pub relationships: Vec<Relationship>,
}
impl std::fmt::Debug for ReaderDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReaderDocument { [redacted] }")
    }
}
fn useful(entity: &str) -> bool {
    matches!(
        entity.rsplit(['.', ':', '/']).next(),
        Some(
            "Artifact"
                | "Concept"
                | "Claim"
                | "Evidence"
                | "ContextBundle"
                | "ArtifactConcept"
                | "ArtifactLink"
                | "Decision"
                | "Architecture"
                | "Document"
        )
    )
}
fn title(entity: &str, value: &serde_json::Value) -> String {
    let raw = [
        "title",
        "name",
        "concept_name",
        "claim_text",
        "quote",
        "purpose",
        "summary",
        "decision",
        "uri",
        "file_path",
    ]
    .iter()
    .find_map(|k| value.get(k).and_then(|v| v.as_str()))
    .unwrap_or(entity);
    let clean = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(160)
        .collect::<String>();
    if clean.is_empty() {
        entity.to_owned()
    } else {
        clean
    }
}
pub(super) fn code(value: &serde_json::Value) -> Result<String> {
    // Indented code does not terminate early on arbitrary stored backticks.
    Ok(serde_json::to_string_pretty(value)?
        .lines()
        .map(|l| format!("    {l}\n"))
        .collect())
}
pub(super) fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if matches!(c, '[' | ']' | '\\' | '\n' | '\r') {
                ' '
            } else {
                c
            }
        })
        .take(180)
        .collect()
}
/// Fully validates archive members before exposing any readable projection.
/// Unknown objects remain structured/raw; no inferred graph approval/grants.
#[cfg(unix)]
pub fn documents(
    path: &Path,
    expected: &ArchiveIdentity,
    limits: &Limits,
) -> Result<Vec<ReaderDocument>> {
    use std::io::Read;
    limits.validate()?;
    let mut manifest_file = crate::unixfs::Source::open(&path.join("manifest.json"), false)?;
    manifest_file.validate(&expected.manifest_sha256, limits.max_manifest_bytes as u64)?;
    let manifest = crate::verify(path, limits)?;
    if manifest.export_id != expected.export_id {
        return Err(Error::Conflict);
    }
    let mut documents = Vec::new();
    let mut retained = 0u64;
    for (index, artifact) in manifest.artifacts.iter().enumerate() {
        let Some(name) = &artifact.records else {
            continue;
        };
        let member = manifest
            .members
            .iter()
            .find(|m| &m.name == name)
            .ok_or(Error::Integrity)?;
        let mut file = crate::unixfs::Source::open(&path.join(name), false)?;
        file.validate(&member.sha256, limits.max_records_bytes)?;
        file.file.seek(SeekFrom::Start(0))?;
        let mut reader = BufReader::new(&mut file.file);
        let mut objects = BTreeMap::new();
        let mut edges = Vec::new();
        let mut line = Vec::new();
        loop {
            line.clear();
            if std::io::Read::by_ref(&mut reader)
                .take(limits.max_row_bytes as u64 + 2)
                .read_until(b'\n', &mut line)?
                == 0
            {
                break;
            }
            if line.len() > limits.max_row_bytes + 1 || line.last() != Some(&b'\n') {
                return Err(Error::Bounds);
            }
            retained = retained
                .checked_add(line.len() as u64)
                .ok_or(Error::Bounds)?;
            // Native materialization is intentionally smaller than raw archival
            // export. Large archives stay exportable, never silently truncated.
            if retained > 64 * 1024 * 1024 {
                return Err(Error::Bounds);
            }
            match serde_json::from_slice::<KnownRecord>(&line)? {
                KnownRecord::Catalog { record } => match *record {
                    ArchivedRecord::Object {
                        object_id,
                        entity,
                        value,
                        provenance,
                        ..
                    } if useful(&entity) => {
                        if objects
                            .insert(object_id, (entity, value, provenance))
                            .is_some()
                        {
                            return Err(Error::Integrity);
                        }
                    }
                    ArchivedRecord::Morphism {
                        morphism_id,
                        arrow,
                        source,
                        target,
                        provenance,
                        ..
                    } => edges.push(Relationship {
                        id: morphism_id,
                        arrow,
                        source,
                        target,
                        provenance,
                    }),
                    _ => {}
                },
                KnownRecord::Proposal { row_id, fields } => {
                    let raw = manifest
                        .members
                        .iter()
                        .find(|m| m.name == artifact.raw)
                        .ok_or(Error::Integrity)?;
                    documents.push(super::review_document::document(
                        super::review_document::Source {
                            identity: expected,
                            index,
                            artifact,
                            records: member,
                            raw,
                        },
                        row_id,
                        fields,
                        line.strip_suffix(b"\n").ok_or(Error::Integrity)?,
                    )?);
                }
                KnownRecord::CloudMarkdown { record } => {
                    let legacy_id = format!(
                        "{}:{}:{}:{}",
                        record.table,
                        record.source_id_hex,
                        record.record_key_hex,
                        record.revision_hex
                    );
                    let id = uuid::Uuid::new_v5(
                        &expected.export_id,
                        format!("{index}:{legacy_id}").as_bytes(),
                    )
                    .to_string();
                    documents.push(ReaderDocument {
                        id,
                        artifact_index: index,
                        legacy_id,
                        entity: "RecoveredMarkdown".into(),
                        title: format!("Recovered document {}", documents.len() + 1),
                        markdown: record.markdown,
                        relationships: vec![],
                    });
                }
                _ => {}
            }
        }
        for (legacy_id, (entity, value, provenance)) in &objects {
            let mut markdown = format!(
                "# {}\n\nArchived {}\n\nLegacy record: `{}`\n\n",
                label(&title(entity, value)),
                label(entity),
                legacy_id
            );
            if entity.rsplit(['.', ':', '/']).next() == Some("Document")
                && value.get("file_path").is_some()
            {
                markdown.push_str("This is an archived reference, not the original Markdown body. Reconnect its folder explicitly and choose the exact current copy in Imported history to read the original. No original folder access was restored.\n\n");
            }
            for key in [
                "claim_text",
                "quote",
                "purpose",
                "summary",
                "description",
                "content",
                "decision",
                "rationale",
            ] {
                if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
                    markdown.push_str(&format!("## {}\n\n{}\n\n", key, text));
                }
            }
            let mut relationships = Vec::new();
            for edge in edges
                .iter()
                .filter(|e| e.source == *legacy_id || e.target == *legacy_id)
            {
                let other = if edge.source == *legacy_id {
                    &edge.target
                } else {
                    &edge.source
                };
                let direction = if edge.source == *legacy_id {
                    "to"
                } else {
                    "from"
                };
                if let Some((kind, data, _)) = objects.get(other) {
                    markdown.push_str(&format!(
                        "- {} {} [{}](legacy/{}/{other}.md)\n",
                        label(&edge.arrow),
                        direction,
                        label(&title(kind, data)),
                        index
                    ));
                } else {
                    markdown.push_str(&format!(
                        "- {} {} `{}` — target preserved in archive\n",
                        label(&edge.arrow),
                        direction,
                        other
                    ));
                }
                relationships.push(Relationship {
                    id: edge.id.clone(),
                    arrow: edge.arrow.clone(),
                    source: edge.source.clone(),
                    target: edge.target.clone(),
                    provenance: edge.provenance.clone(),
                });
            }
            markdown.push_str(&format!(
                "\n## Recorded attributes\n\n{}\n## Provenance\n\n{}",
                code(value)?,
                code(provenance)?
            ));
            if markdown.len() > 1024 * 1024 {
                return Err(Error::Bounds);
            }
            let id = uuid::Uuid::new_v5(
                &expected.export_id,
                format!("{index}:{legacy_id}").as_bytes(),
            )
            .to_string();
            documents.push(ReaderDocument {
                id,
                artifact_index: index,
                legacy_id: legacy_id.clone(),
                entity: entity.clone(),
                title: title(entity, value),
                markdown,
                relationships,
            });
        }
        file.validate(&member.sha256, limits.max_records_bytes)?;
        if documents.len() > 1000
            || documents.iter().map(|d| d.markdown.len()).sum::<usize>() > 64 * 1024 * 1024
        {
            return Err(Error::Bounds);
        }
    }
    // Verify the selected immutable anchor again after all bounded reads.
    manifest_file.validate(&expected.manifest_sha256, limits.max_manifest_bytes as u64)?;
    Ok(documents)
}
#[cfg(not(unix))]
pub fn documents(_: &Path, _: &ArchiveIdentity, _: &Limits) -> Result<Vec<ReaderDocument>> {
    Err(Error::Unsupported)
}
