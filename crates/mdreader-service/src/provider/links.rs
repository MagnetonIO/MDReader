//! Selected-provider links resolve against the current local metadata snapshot.
//! Cache paths are opaque physical materialization, never logical provider paths.
use super::*;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

const MAX_ROWS: usize = 16_384;
const MAX_MATCHES: usize = 16;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderLinkKind {
    Relative,
    Wikilink,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderLinkCandidate {
    pub source_id: SourceId,
    pub document_id: Uuid,
    pub title: DisplayText,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderLinkResolution {
    Resolved {
        candidate: ProviderLinkCandidate,
        projection: ProjectionReceipt,
        line: u32,
        heading: Option<String>,
    },
    SelectedUncached {
        candidate: ProviderLinkCandidate,
        heading: Option<String>,
    },
    Ambiguous {
        candidates: Vec<ProviderLinkCandidate>,
        heading: Option<String>,
    },
    Missing,
    HeadingMissing {
        candidate: ProviderLinkCandidate,
        heading: String,
    },
}
// URI decoding is deliberately applied exactly once, separately to path/fragment.
fn decode(value: &str) -> Result<String> {
    let mut output = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'%' {
            let pair = bytes
                .get(offset + 1..offset + 3)
                .ok_or(ProviderError::Invalid)?;
            let digit = |byte: u8| (byte as char).to_digit(16).ok_or(ProviderError::Invalid);
            output.push((digit(pair[0])? * 16 + digit(pair[1])?) as u8);
            offset += 3;
        } else {
            output.push(bytes[offset]);
            offset += 1;
        }
    }
    let text = String::from_utf8(output).map_err(|_| ProviderError::Invalid)?;
    if text.chars().any(char::is_control) {
        return Err(ProviderError::Invalid);
    }
    Ok(text)
}
fn canonical_path(path: &str) -> Result<String> {
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.contains(['\\', ':'])
        || path.chars().any(char::is_control)
    {
        return Err(ProviderError::Invalid);
    }
    let mut parts = vec![];
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop().ok_or(ProviderError::Invalid)?;
            }
            _ => parts.push(part),
        }
    }
    Ok(format!("/{}", parts.join("/")))
}
pub(super) fn allowed(d: &SourceDescriptor, row: &Metadata) -> bool {
    match d.scope() {
        SelectedScope::ExactDocuments { keys } => keys.contains(&row.key),
        SelectedScope::ScopedRoot { root, recursive } if d.kind() == ProviderKind::Dropbox => {
            let Ok(path) = canonical_path(row.display_path.as_str()) else {
                return false;
            };
            let Ok(root) = canonical_path(root.as_str()) else {
                return false;
            };
            let root = root.trim_end_matches('/');
            path.strip_prefix(&format!("{root}/"))
                .is_some_and(|suffix| !suffix.is_empty() && (*recursive || !suffix.contains('/')))
        }
        SelectedScope::ScopedRoot { root, recursive } if d.kind() == ProviderKind::GoogleDrive => {
            (*recursive && root.as_str() == "root")
                || row
                    .display_path
                    .as_str()
                    .starts_with(&format!("/{}/", root.as_str()))
        }
        _ => false,
    }
}
fn within_root(d: &SourceDescriptor, path: &str) -> bool {
    match d.scope() {
        SelectedScope::ScopedRoot { root, .. } if d.kind() == ProviderKind::GoogleDrive => {
            path.starts_with(&format!("/{}/", root.as_str()))
        }
        SelectedScope::ScopedRoot { root, .. } => canonical_path(root.as_str())
            .ok()
            .is_some_and(|root| path.starts_with(&format!("{}/", root.trim_end_matches('/')))),
        SelectedScope::ExactDocuments { .. } => true,
        _ => false,
    }
}
fn logical_target(d: &SourceDescriptor, from: &Metadata, target: &str) -> Result<String> {
    if target.contains(['\\', ':', '?']) || target.starts_with("//") {
        return Err(ProviderError::Invalid);
    }
    if d.kind() == ProviderKind::GoogleDrive {
        // Drive's narrow metadata records a filename, never its parent ancestry.
        if target.contains('/') || matches!(target, "." | "..") {
            return Err(ProviderError::Unsupported);
        }
        return Ok(if target.is_empty() {
            from.display_path.as_str()
        } else {
            target
        }
        .into());
    }
    if d.kind() != ProviderKind::Dropbox {
        return Err(ProviderError::Unsupported);
    }
    let path = if target.is_empty() {
        canonical_path(from.display_path.as_str())?
    } else if target.starts_with('/') {
        canonical_path(target)?
    } else {
        let parent = from
            .display_path
            .as_str()
            .rsplit_once('/')
            .map(|v| v.0)
            .unwrap_or("");
        canonical_path(&format!("{parent}/{target}"))?
    };
    if !within_root(d, &path) {
        return Err(ProviderError::Invalid);
    }
    Ok(path)
}
fn same_path(kind: ProviderKind, left: &str, right: &str) -> bool {
    if kind == ProviderKind::Dropbox {
        left.to_lowercase() == right.to_lowercase()
    } else {
        left == right
    }
}
fn slug(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ' '))
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}
fn heading_line(body: &MarkdownBytes, heading: Option<&str>) -> Result<Option<u32>> {
    let Some(heading) = heading else {
        return Ok(Some(1));
    };
    let text = std::str::from_utf8(body.as_bytes()).map_err(|_| ProviderError::InvalidUtf8)?;
    let parsed = mdreader_core::parse_markdown(text, "");
    // Core heading text/slugs and authoritative source lines. This deliberately
    // does not claim parity with every renderer's duplicate-ID slug convention.
    Ok(parsed
        .sections
        .iter()
        .filter(|section| section.depth > 0)
        .find(|section| {
            section
                .heading_path
                .last()
                .is_some_and(|text| text.eq_ignore_ascii_case(heading) || slug(text) == heading)
        })
        .map(|section| section.start_line))
}
fn candidate(
    c: &mut Connection,
    d: &SourceDescriptor,
    row: &Metadata,
) -> Result<ProviderLinkCandidate> {
    Ok(ProviderLinkCandidate {
        source_id: d.binding().source_id().clone(),
        document_id: coordinator::handle_for(c, d.binding(), &row.key)?,
        title: row.name.clone(),
    })
}
/// Local owner callback only: no factory, credentials, network or new metadata.
/// The exact admitted origin and enabled generation are checked before matching.
pub(super) fn resolve_link(
    c: &mut Connection,
    reader_source_id: &SourceId,
    path: &RecordKey,
    revision: &OpaqueValue,
    kind: ProviderLinkKind,
    target: &str,
) -> Result<ProviderLinkResolution> {
    if target.len() > 4096 {
        return Err(ProviderError::TooLarge);
    }
    let library = library_id(c)?;
    let source = list(c, &library)?
        .into_iter()
        .find(|source| source.enabled && source.reader_source_id.as_ref() == Some(reader_source_id))
        .ok_or(ProviderError::Unauthorized)?;
    let configuration = configuration(c, &source.source_id)?;
    let d = &configuration.descriptor;
    let b = d.binding();
    revalidate(c, b)?;
    if matches!(d.scope(), SelectedScope::ImportedLegacyBroad { .. }) {
        return Err(ProviderError::Unsupported);
    }
    let key:Option<String>=registry::db(c.query_row("SELECT key FROM mdreader_provider_registry_cache WHERE source=?1 AND binding=?2 AND job IS NOT NULL AND json_extract(job,'$.path')=?3 AND json_extract(job,'$.revision')=?4 LIMIT 1",params![registry::identity(b),b.digest().as_slice(),path.as_str(),revision.as_str()],|r|r.get(0)).optional())?;
    let origin = cached(c, b, &RecordKey::new(key.ok_or(ProviderError::Conflict)?)?)?
        .ok_or(ProviderError::Conflict)?;
    if !allowed(d, &origin.metadata)
        || origin.projection.as_ref().is_none_or(|receipt| {
            receipt.reader_source_id != *reader_source_id
                || receipt.path != *path
                || receipt.revision != *revision
        })
    {
        return Err(ProviderError::Conflict);
    }
    let (raw_path, raw_heading) = target
        .split_once('#')
        .map(|(p, h)| (p, Some(h)))
        .unwrap_or((target, None));
    let decoded = decode(raw_path)?;
    let heading = raw_heading
        .map(decode)
        .transpose()?
        .filter(|s| !s.is_empty());
    let logical = logical_target(d, &origin.metadata, &decoded)?;
    let mut rows = coordinator::published_metadata(c, b, 0, MAX_ROWS)?;
    // Existing registry quota is MAX_ROWS; do not silently resolve a partial set.
    let count:u64=registry::db(c.query_row("SELECT count(*) FROM mdreader_provider_registry_rows WHERE snapshot=(SELECT published FROM mdreader_provider_registry_state WHERE source=?1)",[registry::identity(b)],|r|r.get(0)))?;
    if count > MAX_ROWS as u64 {
        return Err(ProviderError::TooLarge);
    }
    rows.retain(|row| allowed(d, row));
    let mut matches: Vec<&Metadata> = rows
        .iter()
        .filter(|row| {
            same_path(d.kind(), row.display_path.as_str(), &logical)
                || same_path(
                    d.kind(),
                    row.display_path.as_str(),
                    &format!("{logical}.md"),
                )
        })
        .collect();
    if matches.is_empty()
        && matches!(kind, ProviderLinkKind::Wikilink)
        && !decoded.contains('/')
        && !matches!(decoded.as_str(), "" | "." | "..")
    {
        let alias = decoded
            .strip_suffix(".md")
            .unwrap_or(&decoded)
            .to_lowercase();
        matches = rows
            .iter()
            .filter(|row| {
                let basename = row.display_path.as_str().rsplit('/').next().unwrap_or("");
                basename
                    .strip_suffix(".md")
                    .unwrap_or(basename)
                    .to_lowercase()
                    == alias
                    || row
                        .name
                        .as_str()
                        .strip_suffix(".md")
                        .unwrap_or(row.name.as_str())
                        .to_lowercase()
                        == alias
            })
            .collect();
    }
    if matches.len() > MAX_MATCHES {
        return Err(ProviderError::TooLarge);
    }
    matches.sort_by(|a, b| {
        a.display_path
            .as_str()
            .cmp(b.display_path.as_str())
            .then(a.name.as_str().cmp(b.name.as_str()))
            .then(a.key.as_str().cmp(b.key.as_str()))
    });
    match matches.as_slice() {
        [] => Ok(ProviderLinkResolution::Missing),
        [row] => {
            let candidate = candidate(c, d, row)?;
            let Some(cached) = cached(c, b, &row.key)?.filter(|cached| cached.projection.is_some())
            else {
                return Ok(ProviderLinkResolution::SelectedUncached { candidate, heading });
            };
            let Some(line) = heading_line(&cached.document.body, heading.as_deref())? else {
                return Ok(ProviderLinkResolution::HeadingMissing {
                    candidate,
                    heading: heading.unwrap_or_default(),
                });
            };
            Ok(ProviderLinkResolution::Resolved {
                candidate,
                projection: cached.projection.ok_or(ProviderError::Persistence)?,
                line,
                heading,
            })
        }
        _ => Ok(ProviderLinkResolution::Ambiguous {
            candidates: matches
                .into_iter()
                .map(|row| candidate(c, d, row))
                .collect::<Result<_>>()?,
            heading,
        }),
    }
}
