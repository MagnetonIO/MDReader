//! Explicit unresolved source-document references, separate from session history.
use crate::{
    catalog::ArchivedRecord, history::Entry, reviews::ArchiveIdentity, Error, KnownRecord, Limits,
    Manifest, Result, Role,
};
use std::{
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};
pub(crate) fn append(
    archive: &Path,
    identity: &ArchiveIdentity,
    manifest: &Manifest,
    limits: &Limits,
    result: &mut Vec<Entry>,
) -> Result<()> {
    let mut used = serde_json::to_vec(result)?.len();
    for (index, artifact) in manifest
        .artifacts
        .iter()
        .enumerate()
        .filter(|(_, a)| matches!(a.role, Role::Catalog | Role::CloudCatalog))
    {
        let Some(name) = &artifact.records else {
            continue;
        };
        let member = manifest
            .members
            .iter()
            .find(|m| &m.name == name)
            .ok_or(Error::Integrity)?;
        let mut file = crate::unixfs::Source::open(&archive.join(name), false)?;
        file.validate(&member.sha256, limits.max_records_bytes)?;
        file.file.seek(SeekFrom::Start(0))?;
        let mut reader = BufReader::new(&mut file.file);
        loop {
            let mut line = Vec::new();
            if Read::by_ref(&mut reader)
                .take(limits.max_row_bytes as u64 + 2)
                .read_until(b'\n', &mut line)?
                == 0
            {
                break;
            }
            if line.len() > limits.max_row_bytes + 1 || line.last() != Some(&b'\n') {
                return Err(Error::Bounds);
            }
            let KnownRecord::Catalog { record } = serde_json::from_slice(&line)? else {
                continue;
            };
            let ArchivedRecord::Object {
                object_id,
                entity,
                value,
                ..
            } = record.as_ref()
            else {
                continue;
            };
            if entity.rsplit(['.', ':', '/']).next() != Some("Document")
                || value.get("file_path").and_then(|v| v.as_str()).is_none()
            {
                continue;
            }
            if result.len() >= 1000 {
                return Err(Error::Bounds);
            }
            let raw = serde_json::to_value(&record)?;
            used = used
                .checked_add(serde_json::to_vec(&raw)?.len())
                .ok_or(Error::Bounds)?;
            if used > 64 * 1024 * 1024 {
                return Err(Error::Bounds);
            }
            let title = value
                .get("title")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    value
                        .get("file_path")
                        .and_then(|v| v.as_str())
                        .and_then(|p| p.rsplit(['/', '\\']).next())
                })
                .unwrap_or("Original document reference");
            let mut title = title
                .chars()
                .filter(|c| !c.is_control())
                .take(512)
                .collect::<String>();
            while title.len() > 512 {
                title.pop();
            }
            result.push(Entry {
                entry_id: uuid::Uuid::new_v5(
                    &identity.export_id,
                    format!("source-document:{index}:{object_id}").as_bytes(),
                )
                .to_string(),
                title,
                kind: "local".into(),
                context: "source_document".into(),
                archived_position: false,
                raw,
            });
        }
        file.validate(&member.sha256, limits.max_records_bytes)?;
    }
    Ok(())
}
