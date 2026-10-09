//! Typed archival copies only. Opaque legacy identities never grant source access.
use crate::{records::write_record, Error, KnownRecord, Limits, Result, SqliteInfo, Unavailable};
use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::fs::File;

const CACHE: &str = "mdreader_cloud_body_cache";
const REPAIR: &str = "mdreader_cloud_write_repair";
const MAX_BODY: usize = 2 * 1024 * 1024;

/// Each row is a separate retained version, including unpublished accepted writes.
/// No path is interpreted, credential restored, or provider mutation replayed.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchivedMarkdown {
    pub table: String,
    pub row_id: String,
    pub source_id_hex: String,
    pub source_version_hex: Option<String>,
    pub record_key_hex: String,
    pub revision_hex: String,
    pub operation_id_hex: Option<String>,
    pub acceptance_state: Option<String>,
    pub expected_revision: Option<String>,
    pub opened_body_hash_hex: Option<String>,
    pub recorded_at_ms: String,
    pub modified_at_ms: Option<String>,
    pub body_blake3: String,
    /// Exact UTF-8, including BOM and original newline bytes.
    pub markdown: String,
}

impl std::fmt::Debug for ArchivedMarkdown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ArchivedMarkdown { [redacted] }")
    }
}

/// Read one typed JSONL record from a caller-verified archive member. This is
/// archival content, never proof of a current provider revision or source grant.
pub fn decode_record(bytes: &[u8], limits: &Limits) -> Result<Option<ArchivedMarkdown>> {
    limits.validate()?;
    if bytes.len() > limits.max_row_bytes {
        return Err(Error::Bounds);
    }
    Ok(match serde_json::from_slice(bytes)? {
        KnownRecord::CloudMarkdown { record } => Some(*record),
        _ => None,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptV1 {
    version: u8,
    expected_revision: String,
    opened_body_hash: [u8; 32],
    content: String,
    modified_at_ms: Option<i64>,
    size_bytes: u64,
}

fn exact_schema(c: &Connection, table: &str, expected: &[(&str, &str, i64)]) -> Result<bool> {
    let sql: Option<String> = c
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
            [table],
            |r| r.get(0),
        )
        .optional()?;
    let Some(sql) = sql else {
        return Ok(false);
    };
    if sql.to_ascii_uppercase().contains("WITHOUT ROWID") {
        return Ok(false);
    }
    let mut statement =
        c.prepare("SELECT name,type,pk,hidden FROM pragma_table_xinfo(?1) ORDER BY cid")?;
    let columns = statement
        .query_map([table], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(columns.len() == expected.len()
        && columns
            .iter()
            .zip(expected)
            .all(|((name, kind, pk, hidden), (n, t, p))| {
                name == n && kind.eq_ignore_ascii_case(t) && pk == p && *hidden == 0
            }))
}

fn opaque(row: &Row<'_>, index: usize) -> Option<String> {
    let value = row.get_ref(index).ok()?.as_blob().ok()?;
    (1..=4096).contains(&value.len()).then(|| crate::hex(value))
}
fn body(row: &Row<'_>, data: usize, hash: usize, size: usize) -> Option<(String, String)> {
    let value = row.get_ref(data).ok()?.as_blob().ok()?;
    let expected = row.get_ref(hash).ok()?.as_blob().ok()?;
    if value.len() > MAX_BODY
        || row.get::<_, i64>(size).ok()? != value.len() as i64
        || expected != blake3::hash(value).as_bytes()
    {
        return None;
    }
    Some((
        std::str::from_utf8(value).ok()?.into(),
        crate::hex(expected),
    ))
}

fn cache(row: &Row<'_>) -> Option<ArchivedMarkdown> {
    let (markdown, body_blake3) = body(row, 5, 6, 7)?;
    Some(ArchivedMarkdown {
        table: CACHE.into(),
        row_id: row.get::<_, i64>(0).ok()?.to_string(),
        source_id_hex: opaque(row, 1)?,
        source_version_hex: Some(opaque(row, 2)?),
        record_key_hex: opaque(row, 3)?,
        revision_hex: opaque(row, 4)?,
        operation_id_hex: None,
        acceptance_state: None,
        expected_revision: None,
        opened_body_hash_hex: None,
        recorded_at_ms: row.get::<_, i64>(8).ok()?.to_string(),
        modified_at_ms: None,
        body_blake3,
        markdown,
    })
}
fn repair(row: &Row<'_>) -> Option<ArchivedMarkdown> {
    let raw = row.get_ref(6).ok()?.as_blob().ok()?;
    let receipt: ReceiptV1 = serde_json::from_slice(raw).ok()?;
    let hash = row.get_ref(5).ok()?.as_blob().ok()?;
    let state: String = row.get(7).ok()?;
    if receipt.version != 1
        || receipt.content.len() > MAX_BODY
        || receipt.size_bytes != receipt.content.len() as u64
        || receipt.expected_revision.is_empty()
        || receipt.expected_revision.len() > 4096
        || hash != blake3::hash(receipt.content.as_bytes()).as_bytes()
        || !matches!(state.as_str(), "provider-accepted" | "published")
    {
        return None;
    }
    Some(ArchivedMarkdown {
        table: REPAIR.into(),
        row_id: row.get::<_, i64>(0).ok()?.to_string(),
        source_id_hex: opaque(row, 2)?,
        source_version_hex: None,
        record_key_hex: opaque(row, 3)?,
        revision_hex: opaque(row, 4)?,
        operation_id_hex: Some(opaque(row, 1)?),
        acceptance_state: Some(state),
        expected_revision: Some(receipt.expected_revision),
        opened_body_hash_hex: Some(crate::hex(&receipt.opened_body_hash)),
        recorded_at_ms: row.get::<_, i64>(8).ok()?.to_string(),
        modified_at_ms: receipt.modified_at_ms.map(|value| value.to_string()),
        body_blake3: crate::hex(hash),
        markdown: receipt.content,
    })
}

pub(crate) fn convert(
    c: &Connection,
    output: &mut File,
    limits: &Limits,
    info: &SqliteInfo,
) -> Result<Option<u64>> {
    let cache_columns = [
        ("source_id", "BLOB", 1),
        ("source_version_id", "BLOB", 2),
        ("record_key", "BLOB", 3),
        ("revision", "BLOB", 4),
        ("body", "BLOB", 0),
        ("body_hash", "BLOB", 0),
        ("size_bytes", "INTEGER", 0),
        ("last_access_ms", "INTEGER", 0),
    ];
    let repair_columns = [
        ("operation_id", "BLOB", 1),
        ("source_id", "BLOB", 0),
        ("record_key", "BLOB", 0),
        ("accepted_revision", "BLOB", 0),
        ("body_hash", "BLOB", 0),
        ("receipt_json", "BLOB", 0),
        ("state", "TEXT", 0),
        ("created_at_ms", "INTEGER", 0),
    ];
    let supported = [
        (CACHE, exact_schema(c, CACHE, &cache_columns)?),
        (REPAIR, exact_schema(c, REPAIR, &repair_columns)?),
    ];
    if !supported.iter().any(|(_, yes)| *yes) {
        return Ok(None);
    }
    let mut used = 0;
    let mut count = 1;
    write_record(
        output,
        &KnownRecord::CloudArchiveScope {
            supported_tables: supported
                .iter()
                .filter(|(_, yes)| *yes)
                .map(|(name, _)| (*name).into())
                .collect(),
            raw_only_tables: info
                .tables
                .iter()
                .filter(|t| !supported.iter().any(|(name, yes)| *yes && *name == t.name))
                .map(|t| t.name.clone())
                .collect(),
            source_binding: "not_performed".into(),
            write_replay: "forbidden".into(),
        },
        &mut used,
        limits,
    )?;
    c.set_limit(
        rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
        limits.max_row_bytes as i32,
    )?;
    for (table, yes) in supported {
        if !yes {
            continue;
        }
        // Oversized values stay in the raw snapshot, never loaded into a typed row.
        let sql = if table == CACHE {
            "SELECT rowid,CASE WHEN length(source_id)<=4096 THEN source_id END,CASE WHEN length(source_version_id)<=4096 THEN source_version_id END,CASE WHEN length(record_key)<=4096 THEN record_key END,CASE WHEN length(revision)<=4096 THEN revision END,CASE WHEN length(body)<=?1 THEN body END,CASE WHEN length(body_hash)=32 THEN body_hash END,size_bytes,last_access_ms FROM mdreader_cloud_body_cache ORDER BY rowid"
        } else {
            "SELECT rowid,CASE WHEN length(operation_id)<=4096 THEN operation_id END,CASE WHEN length(source_id)<=4096 THEN source_id END,CASE WHEN length(record_key)<=4096 THEN record_key END,CASE WHEN length(accepted_revision)<=4096 THEN accepted_revision END,CASE WHEN length(body_hash)=32 THEN body_hash END,CASE WHEN length(receipt_json)<=?1 THEN receipt_json END,CASE WHEN length(state)<=32 THEN state END,created_at_ms FROM mdreader_cloud_write_repair ORDER BY rowid"
        };
        let mut statement = c.prepare(sql)?;
        let bound = if table == CACHE {
            MAX_BODY
        } else {
            MAX_BODY + 65536
        };
        let mut rows = statement.query([bound.min(limits.max_row_bytes.saturating_sub(32768))])?;
        while let Some(row) = rows.next()? {
            count += 1;
            if count > limits.max_rows {
                return Err(Error::Bounds);
            }
            let decoded = if table == CACHE {
                cache(row)
            } else {
                repair(row)
            };
            let mut record = decoded.map(|record| KnownRecord::CloudMarkdown {
                record: Box::new(record),
            });
            if record.as_ref().is_some_and(|r| {
                serde_json::to_vec(r).map_or(true, |b| b.len() > limits.max_row_bytes)
            }) {
                record = None;
            }
            let record = record.unwrap_or(KnownRecord::CloudUnavailable {
                table: table.into(),
                row_id: Some(row.get::<_, i64>(0)?.to_string()),
                reason: Unavailable::UnsupportedRecord,
            });
            write_record(output, &record, &mut used, limits)?;
        }
    }
    Ok(Some(count))
}
