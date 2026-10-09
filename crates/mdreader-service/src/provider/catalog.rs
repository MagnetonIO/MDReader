//! Bounded observed metadata, without opening cache bodies or using a vault.
use super::*;
use crate::catalog::{CatalogDocument, CatalogOpen};
use rusqlite::{params, OptionalExtension};
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CatalogSnapshot {
    pub binding: SourceBinding,
    pub epoch: String,
    observed: super::metadata_observation::Observation,
}
pub(crate) struct CatalogPage {
    pub snapshot: CatalogSnapshot,
    pub documents: Vec<CatalogDocument>,
    pub next_after: Option<String>,
}
fn snapshot(c: &mut Connection, source: &SourceId) -> Result<CatalogSnapshot> {
    let b = configuration(c, source)?.descriptor.binding().clone();
    let epoch = super::catalog_epoch::epoch(c, &b)?;
    let observed = super::metadata_observation::snapshot(c, &b)?;
    Ok(CatalogSnapshot {
        binding: b,
        epoch,
        observed,
    })
}
/// Snapshot metadata and per-row cache receipts are observations. Exact core
/// Read remains the authority for cached body bytes and current enabled binding.
pub(crate) fn catalog_page(
    c: &mut Connection,
    source: &SourceId,
    after: Option<&str>,
    expected: Option<&CatalogSnapshot>,
    limit: usize,
) -> Result<CatalogPage> {
    catalog_page_checked(c, source, after, expected, limit, |_| Ok(()))
}
fn catalog_page_checked(
    c: &mut Connection,
    source: &SourceId,
    after: Option<&str>,
    expected: Option<&CatalogSnapshot>,
    limit: usize,
    observed: impl FnOnce(&mut Connection) -> Result<()>,
) -> Result<CatalogPage> {
    if !(1..=100).contains(&limit) || after.is_some_and(|key| !validate_record_key(key)) {
        return Err(ProviderError::Invalid);
    }
    super::coordinator::initialize_client(c)?;
    let before = snapshot(c, source)?;
    if expected.is_some_and(|e| e != &before) || after.is_some() && expected.is_none() {
        return Err(ProviderError::Conflict);
    }
    let descriptor = registry::descriptor_at(c, &before.binding, true)?;
    let after_time = if let Some(key) = after {
        let row = registry::metadata_in(
            c,
            before
                .observed
                .id
                .as_deref()
                .ok_or(ProviderError::Conflict)?,
            &RecordKey::new(key)?,
        )?
        .ok_or(ProviderError::Conflict)?;
        row.modified_at_ms.unwrap_or(i64::MIN)
    } else {
        i64::MAX
    };
    let raw = {
        let mut statement=registry::db(c.prepare("SELECT substr(metadata,1,32769),substr(digest,1,33) FROM mdreader_provider_registry_rows WHERE snapshot=?1 AND (?2='' OR COALESCE(json_extract(metadata,'$.modified_at_ms'),-9223372036854775808)<?3 OR (COALESCE(json_extract(metadata,'$.modified_at_ms'),-9223372036854775808)=?3 AND key>?2)) ORDER BY COALESCE(json_extract(metadata,'$.modified_at_ms'),-9223372036854775808) DESC,key LIMIT ?4"))?;
        let rows = registry::db(statement.query_map(
            params![
                before.observed.id,
                after.unwrap_or(""),
                after_time,
                limit + 1
            ],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)),
        ))?;
        rows.map(registry::db).collect::<Result<Vec<_>>>()?
    };
    let more = raw.len() > limit;
    let mut documents = Vec::with_capacity(raw.len().min(limit));
    let mut last = None;
    for (text, digest) in raw.into_iter().take(limit) {
        if body_hash(text.as_bytes()).as_slice() != digest {
            return Err(ProviderError::Persistence);
        }
        let metadata: Metadata = registry::decode(&text, 32 * 1024)?;
        if !super::links::allowed(&descriptor, &metadata) {
            return Err(ProviderError::Unauthorized);
        }
        last = Some(metadata.key.as_str().to_owned());
        let id = super::coordinator::handle_for(c, &before.binding, &metadata.key)?;
        let cache:Option<(Vec<u8>,i64,Option<String>)>=registry::db(c.query_row("SELECT substr(version,1,33),observed,substr(job,1,32769) FROM mdreader_provider_registry_cache WHERE source=?1 AND binding=?2 AND key=?3",params![registry::identity(&before.binding),before.binding.digest().as_slice(),metadata.key.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional())?;
        if cache
            .as_ref()
            .is_some_and(|(version, _, _)| version.len() != 32)
        {
            return Err(ProviderError::Persistence);
        }
        let cached = cache.filter(|(version, _, _)| {
            version.as_slice() == registry::version_digest(&metadata.version)
        });
        if cached.as_ref().is_some_and(|(_, at, _)| *at < 0) {
            return Err(ProviderError::Persistence);
        }
        let receipt: Option<ProjectionReceipt> = cached
            .as_ref()
            .and_then(|(_, _, job)| job.as_ref())
            .map(|j| registry::decode(j, 32768))
            .transpose()?;
        if receipt.as_ref().is_some_and(|r| {
            r.reader_source_id.as_str() != format!("provider:{}", hex(&before.binding.digest()))
                || !r
                    .path
                    .as_str()
                    .strip_prefix("records/")
                    .and_then(|p| p.strip_suffix(".md"))
                    .is_some_and(|p| p.len() == 64 && p.bytes().all(|b| b.is_ascii_hexdigit()))
                || r.revision.as_str().len() != 64
                || !r.revision.as_str().bytes().all(|b| b.is_ascii_hexdigit())
        }) {
            return Err(ProviderError::Persistence);
        }
        let open = match receipt {
            Some(r) => CatalogOpen::Exact {
                source_id: r.reader_source_id.as_str().into(),
                path: r.path.as_str().into(),
                revision: r.revision.as_str().into(),
            },
            None => CatalogOpen::SelectedUncached {
                source_id: source.as_str().into(),
                reader_source_id: format!("provider:{}", hex(&before.binding.digest())),
                document_id: id.to_string(),
            },
        };
        documents.push(CatalogDocument {
            name: metadata.name.as_str().into(),
            display_path: metadata.display_path.as_str().into(),
            modified_at_ms: metadata.modified_at_ms,
            size_bytes: metadata.size_bytes,
            observed_at_ms: cached.as_ref().map(|(_, at, _)| *at),
            observation: if matches!(&open, CatalogOpen::Exact { .. }) {
                "cached_provider_revision"
            } else if before.observed.partial {
                "partial_provider_metadata"
            } else {
                "published_provider_metadata"
            }
            .into(),
            open,
        });
    }
    observed(c)?;
    if snapshot(c, source)? != before {
        return Err(ProviderError::Conflict);
    }
    Ok(CatalogPage {
        snapshot: before,
        documents,
        next_after: if more { last } else { None },
    })
}
/// Corrupt or unsupported source metadata is isolated to a bounded row; valid
/// folders/providers remain visible. Private descriptors never leave this file.
pub(crate) fn catalog_sources(c: &mut Connection) -> Result<Vec<crate::catalog::CatalogSource>> {
    super::catalog_epoch::initialize(c)?;
    let library = library_id(c)?;
    let raw = {
        let mut statement=registry::db(c.prepare("SELECT substr(descriptor,1,65537),substr(digest,1,33),enabled,substr(coverage,1,32769) FROM mdreader_provider_registry_sources ORDER BY rowid LIMIT 129"))?;
        let rows = registry::db(statement.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, bool>(2)?,
                r.get::<_, String>(3)?,
            ))
        }))?;
        rows.collect::<Vec<_>>()
    };
    if raw.len() > 128 {
        return Err(ProviderError::TooLarge);
    }
    let mut sources = Vec::new();
    for (index, row) in raw.into_iter().enumerate() {
        let parsed = (|| {
            let (text, digest, enabled, coverage) = registry::db(row)?;
            if body_hash(text.as_bytes()).as_slice() != digest {
                return Err(ProviderError::Persistence);
            }
            let descriptor: SourceDescriptor = registry::decode(&text, 64 * 1024)?;
            if descriptor.binding().library_id() != &library {
                return Err(ProviderError::Unauthorized);
            }
            let coverage: Coverage = registry::decode(&coverage, 32768)?;
            let (coverage, observed_at_ms) = match coverage {
                Coverage::NeverIndexed => ("never_indexed", None),
                Coverage::Partial { .. } => ("partial_selected_metadata", None),
                Coverage::MetadataCompleteSelected { observed_at_ms }
                | Coverage::CompleteSelected { observed_at_ms } => {
                    ("selected_metadata", Some(observed_at_ms))
                }
            };
            Ok(crate::catalog::CatalogSource {
                catalog_id: format!("remote:{}", descriptor.binding().source_id().as_str()),
                name: descriptor.name().as_str().into(),
                kind: match descriptor.kind() {
                    ProviderKind::GoogleDrive => "google_drive",
                    ProviderKind::Dropbox => "dropbox",
                }
                .into(),
                enabled,
                coverage: coverage.into(),
                observed_at_ms,
                failure: None,
            })
        })();
        sources.push(parsed.unwrap_or_else(|_| crate::catalog::CatalogSource {
            catalog_id: format!("unavailable-provider:{index}"),
            name: "Unavailable cloud source".into(),
            kind: "provider".into(),
            enabled: false,
            coverage: "unavailable".into(),
            observed_at_ms: None,
            failure: Some(crate::catalog::CatalogError::Persistence),
        }));
    }
    Ok(sources)
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
