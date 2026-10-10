//! Offline semantic archival conversion. Decoding never applies operations,
//! restores grants, or treats proposal decisions as proof of graph membership.
mod chain;
mod model;
pub mod reader;
mod schema;
use crate::{records::write_record, Error, KnownRecord, Limits, Result, SqliteInfo};
pub use model::{ArchivedRecord, Cell, ChainStatus};
use rusqlite::{types::ValueRef, Connection};
use std::{collections::BTreeMap, fs::File, io::Seek};

pub fn decode_record(bytes: &[u8], limits: &Limits) -> Result<Option<ArchivedRecord>> {
    if bytes.len() > limits.max_row_bytes {
        return Err(Error::Bounds);
    }
    Ok(match serde_json::from_slice(bytes)? {
        KnownRecord::Catalog { record } => Some(*record),
        _ => None,
    })
}
fn text<'a>(row: &'a rusqlite::Row<'_>, columns: &[String], name: &str) -> Option<&'a str> {
    row.get_ref(columns.iter().position(|c| c == name)? + 1)
        .ok()?
        .as_str()
        .ok()
}
fn uuid(value: &str) -> Option<String> {
    let id = uuid::Uuid::parse_str(value).ok()?;
    (id.to_string() == value).then(|| value.to_owned())
}
fn project(
    row: &rusqlite::Row<'_>,
    table: &str,
    columns: &[String],
    fields: BTreeMap<String, Cell>,
) -> Option<ArchivedRecord> {
    let row_id = row.get::<_, i64>(0).ok()?.to_string();
    let json = |name| serde_json::from_str::<serde_json::Value>(text(row, columns, name)?).ok();
    Some(match table {
        "objects" => ArchivedRecord::Object {
            row_id,
            object_id: uuid(text(row, columns, "object_id")?)?,
            entity: text(row, columns, "entity")?.to_owned(),
            value: json("value")?,
            provenance: json("provenance")?,
            fields,
        },
        "morphisms" => ArchivedRecord::Morphism {
            row_id,
            morphism_id: uuid(text(row, columns, "morphism_id")?)?,
            arrow: text(row, columns, "arrow")?.to_owned(),
            source: uuid(text(row, columns, "source")?)?,
            target: uuid(text(row, columns, "target")?)?,
            provenance: json("provenance")?,
            fields,
        },
        "operation_log" => ArchivedRecord::Operation {
            row_id,
            envelope: text(row, columns, "envelope")?.to_owned(),
            fields,
        },
        _ => ArchivedRecord::History {
            table: table.to_owned(),
            row_id,
            fields,
        },
    })
}
pub(crate) fn convert(
    c: &Connection,
    output: &mut File,
    limits: &Limits,
    info: &SqliteInfo,
) -> Result<Option<u64>> {
    let mut supported = Vec::new();
    for table in ["objects", "morphisms", "operation_log"]
        .iter()
        .copied()
        .chain(schema::HISTORY.iter().copied())
    {
        let Some(columns) = schema::columns(c, table)? else {
            continue;
        };
        let required: &[&str] = match table {
            "objects" => &["object_id", "entity", "value", "provenance"],
            "morphisms" => &["morphism_id", "arrow", "source", "target", "provenance"],
            "operation_log" => &["envelope"],
            _ => &[],
        };
        if required.iter().all(|n| columns.iter().any(|c| c == n)) {
            supported.push((table, columns));
        }
    }
    if supported.is_empty() {
        return Ok(None);
    }
    let mut used = output.stream_position()?;
    let mut count = 0u64;
    let mut emit = |record: ArchivedRecord| -> Result<()> {
        count = count.checked_add(1).ok_or(Error::Bounds)?;
        if count > limits.max_rows {
            return Err(Error::Bounds);
        }
        write_record(
            output,
            &KnownRecord::Catalog {
                record: Box::new(record),
            },
            &mut used,
            limits,
        )
    };
    emit(ArchivedRecord::Scope {
        supported_tables: supported.iter().map(|(n, _)| n.to_string()).collect(),
        raw_only_tables: info
            .tables
            .iter()
            .filter(|t| !supported.iter().any(|(n, _)| *n == t.name))
            .map(|t| t.name.clone())
            .collect(),
    })?;
    let mut head = Some("0".repeat(64));
    let mut ops = 0u64;
    let mut stored = None;
    for (table, columns) in supported {
        // Bound the sum BEFORE loading any cell. Invalid/oversized rows remain
        // exact in the raw snapshot and produce an explicit unavailable marker.
        let total = columns
            .iter()
            .map(|n| format!("coalesce(length(CAST({} AS BLOB)),0)", schema::quote(n)))
            .collect::<Vec<_>>()
            .join("+");
        let good = format!("({total})<=?1");
        let selection = columns
            .iter()
            .map(|n| format!("CASE WHEN {good} THEN {} END", schema::quote(n)))
            .collect::<Vec<_>>()
            .join(",");
        let mut s = c.prepare(&format!(
            "SELECT rowid,{selection},({good}) FROM {} ORDER BY rowid LIMIT ?2",
            schema::quote(table)
        ))?;
        let mut rows = s.query(rusqlite::params![
            limits.max_row_bytes / 4,
            limits.max_rows + 1
        ])?;
        while let Some(row) = rows.next()? {
            let row_id = row.get::<_, i64>(0)?.to_string();
            if table == "operation_log" {
                ops += 1;
            }
            if row.get::<_, i64>(columns.len() + 1)? != 1 {
                if table == "operation_log" {
                    head = None;
                }
                emit(ArchivedRecord::Unavailable {
                    table: table.into(),
                    row_id,
                })?;
                continue;
            }
            let mut fields = BTreeMap::new();
            for (i, name) in columns.iter().enumerate() {
                fields.insert(
                    name.clone(),
                    match row.get_ref(i + 1)? {
                        ValueRef::Null => Cell::Null,
                        ValueRef::Integer(n) => Cell::Integer(n.to_string()),
                        ValueRef::Real(n) => Cell::RealBits(format!("{:016x}", n.to_bits())),
                        ValueRef::Text(b) => Cell::TextBytes(crate::hex(b)),
                        ValueRef::Blob(b) => Cell::BlobBytes(crate::hex(b)),
                    },
                );
            }
            if table == "operation_log" {
                head = head
                    .as_deref()
                    .and_then(|parent| chain::extend(parent, text(row, &columns, "envelope")?));
            }
            if table == "operation_log_anchor" {
                let n = columns
                    .iter()
                    .position(|n| n == "op_count")
                    .and_then(|i| row.get::<_, i64>(i + 1).ok());
                let h = text(row, &columns, "head_digest");
                if stored.is_some()
                    || n.is_none_or(|n| n < 0)
                    || h.is_none_or(|h| {
                        h.len() != 64
                            || !h
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    })
                {
                    head = None;
                } else {
                    stored = n.zip(h.map(str::to_owned));
                }
            }
            emit(
                project(row, table, &columns, fields).unwrap_or(ArchivedRecord::Unavailable {
                    table: table.into(),
                    row_id,
                }),
            )?;
        }
    }
    let status = match (&head, &stored) {
        (None, _) => ChainStatus::UnsupportedEnvelope,
        (Some(_), None) => ChainStatus::NoIndependentAnchor,
        (Some(h), Some((n, s))) if *n as u64 == ops && h == s => ChainStatus::MatchesStoredAnchor,
        _ => ChainStatus::Mismatch,
    };
    emit(ArchivedRecord::Chain {
        observed_count: ops.to_string(),
        computed_digest: head,
        stored_count: stored.as_ref().map(|(n, _)| n.to_string()),
        stored_digest: stored.map(|(_, h)| h),
        status,
    })?;
    Ok(Some(count))
}

#[cfg(all(test, unix))]
mod tests;

#[cfg(any(unix, windows))]
mod review_document;
