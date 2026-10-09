//! Lossless session continuity. Old positions/references are archival data,
//! never translated into a current revision, line, source grant or approval.
use crate::{reviews::ArchiveIdentity, Error, KnownRecord, Limits, Result, Role};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read},
    path::Path,
};
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    pub entry_id: String,
    pub title: String,
    pub kind: String,
    pub context: String,
    pub archived_position: bool,
    /// Exact JSON subtree. The full original file is also retained raw.
    pub raw: serde_json::Value,
}
impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HistoryEntry { [redacted] }")
    }
}
#[cfg(unix)]
pub fn entries(archive: &Path, expected: &ArchiveIdentity, limits: &Limits) -> Result<Vec<Entry>> {
    if crate::migration::archive::identity(archive, limits)? != *expected {
        return Err(Error::Conflict);
    }
    let manifest = crate::verify(archive, limits)?;
    let mut result = Vec::new();
    for (index, artifact) in manifest
        .artifacts
        .iter()
        .enumerate()
        .filter(|(_, a)| a.role == Role::Session)
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
        let mut reader = BufReader::new(&mut file.file);
        let mut line = Vec::new();
        if Read::by_ref(&mut reader)
            .take(limits.max_row_bytes as u64 + 2)
            .read_until(b'\n', &mut line)?
            == 0
        {
            continue;
        }
        if line.len() > limits.max_row_bytes + 1 {
            return Err(Error::Bounds);
        }
        let KnownRecord::Session { value, .. } = serde_json::from_slice(&line)? else {
            return Err(Error::Integrity);
        };
        let mut push = |pointer: String, raw: serde_json::Value, position: bool| -> Result<()> {
            if result.len() >= 1000 {
                return Err(Error::Bounds);
            }
            let reference = raw.get("docRef").unwrap_or(&raw);
            let mut title: String = reference
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Imported reading entry")
                .chars()
                .filter(|c| !c.is_control())
                .take(180)
                .collect();
            while title.len() > 512 {
                title.pop();
            }
            let context = if pointer.starts_with("/recent/") {
                "recent"
            } else if pointer.contains("/history/") {
                "tab_history"
            } else if pointer.starts_with("/tabs/") {
                "tab"
            } else {
                "unknown"
            };
            let kind = if reference.get("kind").and_then(|v| v.as_str()) == Some("cloud") {
                "provider"
            } else if reference.get("path").is_some() {
                "local"
            } else {
                "unknown"
            };
            result.push(Entry {
                entry_id: uuid::Uuid::new_v5(
                    &expected.export_id,
                    format!("history:{index}:{pointer}").as_bytes(),
                )
                .to_string(),
                title,
                kind: kind.into(),
                context: context.into(),
                archived_position: position,
                raw,
            });
            Ok(())
        };
        if let Some(tabs) = value.get("tabs").and_then(|v| v.as_array()) {
            for (i, tab) in tabs.iter().enumerate() {
                push(
                    format!("/tabs/{i}"),
                    tab.clone(),
                    tab.get("scroll").is_some(),
                )?;
                if let Some(history) = tab.get("history").and_then(|v| v.as_array()) {
                    for (j, entry) in history.iter().enumerate() {
                        push(format!("/tabs/{i}/history/{j}"), entry.clone(), false)?;
                    }
                }
            }
        }
        if let Some(recent) = value.get("recent").and_then(|v| v.as_array()) {
            for (i, entry) in recent.iter().enumerate() {
                push(format!("/recent/{i}"), entry.clone(), false)?;
            }
        }
        file.validate(&member.sha256, limits.max_records_bytes)?;
    }
    if crate::migration::archive::identity(archive, limits)? != *expected {
        return Err(Error::Conflict);
    }
    Ok(result)
}
#[cfg(not(unix))]
pub fn entries(_: &Path, _: &ArchiveIdentity, _: &Limits) -> Result<Vec<Entry>> {
    Err(Error::Unsupported)
}
