//! Read-only observations of already published device documents.
//! No migrations, cache creation, recovery, network or source admission.
mod model;
pub use model::*;
mod membership;
use super::{schema::readonly, DeviceDocument, DeviceRead, Error, ReaderStore, Result};
pub use membership::{document_in, page_in, read_in, state_in};
use rusqlite::{params, OptionalExtension};

fn snapshot(store: &ReaderStore) -> Result<Option<Snapshot>> {
    let db = store.device_connection();
    let version = db.query_row("PRAGMA data_version", [], |r| r.get(0))?;
    let Some(ns) = readonly::namespace(db)? else {
        return Ok(None);
    };
    readonly::validate_rows(db, ns)?;
    crate::store::catalog::schema::validate(db, false).map_err(Error::from)?;
    let state = match ns {
        readonly::Namespace::Current => "d.state_generation,d.retired,coalesce(d.retirement,'')",
        readonly::Namespace::Original => "0,0,''",
    };
    let sql = format!(
        "SELECT d.id,d.name,d.source_id,d.revision,d.bytes,d.pinned,d.line,d.indexed,{state},s.enabled,s.path,s.identity,a.epoch FROM device_documents_v1 d JOIN sources s ON s.id=d.source_id JOIN reader_catalog_authorities_v1 a ON a.source_id=d.source_id ORDER BY d.id LIMIT 1001"
    );
    let mut query = db.prepare(&sql)?;
    let mut rows = query.query([])?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"mdreader-device-catalog-v1\0");
    let mut count = 0;
    while let Some(row) = rows.next()? {
        count += 1;
        if count > super::MAX_DOCUMENTS {
            return Err(Error::InvalidSchema);
        }
        for column in 0..15 {
            use rusqlite::types::ValueRef;
            match row.get_ref(column)? {
                ValueRef::Text(value) | ValueRef::Blob(value) => {
                    if value.len() > 16384 {
                        return Err(Error::InvalidSchema);
                    }
                    hash.update(&(value.len() as u64).to_le_bytes());
                    hash.update(value);
                }
                ValueRef::Integer(value) => {
                    hash.update(&8u64.to_le_bytes());
                    hash.update(&value.to_le_bytes());
                }
                _ => return Err(Error::InvalidSchema),
            }
        }
    }
    drop(rows);
    drop(query);
    let expected_count: u64 =
        db.query_row("SELECT count(*) FROM device_documents_v1", [], |r| r.get(0))?;
    if count != expected_count {
        return Err(Error::InvalidSchema);
    }
    let final_version: i64 = db.query_row("PRAGMA data_version", [], |r| r.get(0))?;
    if version != final_version {
        return Err(Error::Conflict);
    }
    Ok(Some(Snapshot {
        owner: store.device_owner_session(),
        data_version: version,
        changes: db.total_changes(),
        digest: *hash.finalize().as_bytes(),
    }))
}

pub fn page(
    store: &ReaderStore,
    after: Option<&str>,
    expected: Option<&Snapshot>,
    limit: usize,
) -> Result<Page> {
    page_checked(store, after, expected, limit, || {})
}
fn page_checked(
    store: &ReaderStore,
    after: Option<&str>,
    expected: Option<&Snapshot>,
    limit: usize,
    observed: impl FnOnce(),
) -> Result<Page> {
    if !(1..=100).contains(&limit) || after.is_some_and(|id| uuid::Uuid::parse_str(id).is_err()) {
        return Err(Error::InvalidSelection);
    }
    let before = snapshot(store)?;
    if expected.is_some() && expected != before.as_ref() || after.is_some() && expected.is_none() {
        return Err(Error::Conflict);
    }
    let Some(ns) = readonly::namespace(store.device_connection())? else {
        if snapshot(store)?.is_some() {
            return Err(Error::Conflict);
        }
        return Ok(Page {
            documents: vec![],
            next_after: None,
            snapshot: None,
        });
    };
    let active = if ns == readonly::Namespace::Current {
        "AND d.retired=0"
    } else {
        ""
    };
    let mut documents = store.device_connection().prepare(&format!(
        "SELECT d.id,d.name,d.source_id,d.revision,d.bytes,d.pinned,d.line,d.indexed FROM device_documents_v1 d JOIN sources s ON s.id=d.source_id WHERE s.enabled=1 {active} AND d.id>?1 ORDER BY d.id LIMIT ?2"
    ))?.query_map(params![after.unwrap_or(""), limit + 1], super::documents::row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let next_after = if documents.len() > limit {
        documents.truncate(limit);
        documents.last().map(|d| d.id.clone())
    } else {
        None
    };
    observed();
    if snapshot(store)? != before {
        return Err(Error::Conflict);
    }
    Ok(Page {
        documents,
        next_after,
        snapshot: before,
    })
}

/// Offline export metadata, including disabled/retired rows. This never reads
/// bodies, regrants a source or changes retirement; the caller must hold closed
/// owner custody and explicitly choose each separately materialized copy.
pub struct ArchiveMetadata {
    pub document: DeviceDocument,
    pub enabled: bool,
    pub retired: bool,
}
pub fn archive_metadata(store: &ReaderStore) -> Result<Vec<ArchiveMetadata>> {
    let before = snapshot(store)?;
    let Some(ns) = readonly::namespace(store.device_connection())? else {
        return Ok(vec![]);
    };
    let retired = if ns == readonly::Namespace::Current {
        "d.retired"
    } else {
        "0"
    };
    let sql = format!("SELECT d.id,d.name,d.source_id,d.revision,d.bytes,d.pinned,d.line,d.indexed,s.enabled,{retired} FROM device_documents_v1 d JOIN sources s ON s.id=d.source_id ORDER BY d.id LIMIT 1001");
    let mut query = store.device_connection().prepare(&sql)?;
    let rows = query
        .query_map([], |r| {
            Ok(ArchiveMetadata {
                document: super::documents::row(r)?,
                enabled: r.get(8)?,
                retired: r.get(9)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len() > super::MAX_DOCUMENTS || snapshot(store)? != before {
        return Err(Error::Conflict);
    }
    Ok(rows)
}

/// Exact enabled, nonretired metadata only; known original layouts are readable
/// without silently adopting them into a newer schema.
pub fn document(store: &ReaderStore, id: &str) -> Result<DeviceDocument> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err(Error::InvalidSelection);
    }
    let db = store.device_connection();
    let ns = readonly::namespace(db)?.ok_or(Error::Unavailable)?;
    let active = if ns == readonly::Namespace::Current {
        "AND d.retired=0 AND d.retirement IS NULL AND d.state_generation BETWEEN 0 AND 9007199254740991"
    } else {
        ""
    };
    let document = db.query_row(&format!(
        "SELECT d.id,d.name,d.source_id,d.revision,d.bytes,d.pinned,d.line,d.indexed FROM device_documents_v1 d JOIN sources s ON s.id=d.source_id WHERE d.id=?1 AND s.enabled=1 {active} AND length(CAST(d.name AS BLOB))<=240 AND length(d.revision)=64 AND d.bytes BETWEEN 0 AND 1048576"
    ), [id], super::documents::row).optional()?.ok_or(Error::Unavailable)?;
    Ok(document)
}

pub fn read(store: &ReaderStore, expected: &Receipt) -> Result<DeviceRead> {
    if expected.revision.len() != 64
        || !expected
            .revision
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::InvalidSelection);
    }
    let before = read_fence(store)?;
    let document = document(store, &expected.document_id)?;
    if receipt(&document) != *expected {
        return Err(Error::Conflict);
    }
    let body = store.read_document(&document.source_id, "document.md", Some(&document.revision))?;
    let after = self::document(store, &expected.document_id)?;
    if body.markdown.len() != document.bytes as usize
        || receipt(&after) != *expected
        || read_fence(store)? != before
    {
        return Err(Error::Conflict);
    }
    Ok(DeviceRead {
        document,
        markdown: body.markdown,
    })
}

fn read_fence(store: &ReaderStore) -> Result<(i64, u64)> {
    let db = store.device_connection();
    Ok((
        db.query_row("PRAGMA data_version", [], |r| r.get(0))?,
        db.total_changes(),
    ))
}

/// Fence a live image lease against grant ABA and document-state transitions,
/// without invalidating it for unrelated account/progress writes.
pub(in crate::device) fn document_epoch(store: &ReaderStore, id: &str) -> Result<String> {
    let document = document(store, id)?;
    let db = store.device_connection();
    crate::store::catalog::schema::validate(db, false).map_err(Error::from)?;
    let ns = readonly::namespace(db)?.ok_or(Error::Unavailable)?;
    let state = if ns == readonly::Namespace::Current {
        "d.state_generation"
    } else {
        "0"
    };
    let (epoch, generation): (String, u64) = db.query_row(&format!(
        "SELECT a.epoch,{state} FROM device_documents_v1 d JOIN reader_catalog_authorities_v1 a ON a.source_id=d.source_id WHERE d.id=?1"
    ), [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let mut hash = blake3::Hasher::new();
    hash.update(&serde_json::to_vec(&document)?);
    hash.update(epoch.as_bytes());
    hash.update(&generation.to_le_bytes());
    Ok(hash.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests;
