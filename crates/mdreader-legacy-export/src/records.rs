use crate::{
    hex, Error, KnownRecord, Limits, Result, Role, SqlValue, SqliteInfo, SqliteIntegrity,
    TableInfo, Unavailable,
};
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::Path,
};
pub(crate) enum Converted {
    Known {
        rows: u64,
        sqlite: Option<SqliteInfo>,
    },
    Unavailable {
        reason: Unavailable,
        sqlite: Option<SqliteInfo>,
    },
}
pub(crate) fn convert(
    role: Role,
    path: &Path,
    output: &mut File,
    limits: &Limits,
) -> Result<Converted> {
    if role.sqlite() {
        return sqlite(role, path, output, limits);
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limits.max_json_bytes as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limits.max_json_bytes {
        return Err(Error::Bounds);
    }
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return Ok(Converted::Unavailable {
                reason: Unavailable::MalformedKnownRecord,
                sqlite: None,
            })
        }
    };
    let record = match role {
        Role::Library => {
            let version = value.get("version").and_then(|v| v.as_u64());
            if !matches!(version, Some(1..=9)) {
                return Ok(Converted::Unavailable {
                    reason: Unavailable::UnknownVersion,
                    sqlite: None,
                });
            }
            if !value.get("sources").is_some_and(|v| v.is_array()) {
                return Ok(Converted::Unavailable {
                    reason: Unavailable::MalformedKnownRecord,
                    sqlite: None,
                });
            }
            KnownRecord::Library {
                version: version.unwrap(),
                value,
            }
        }
        Role::Session => {
            let Some(value) = value.get("session").cloned() else {
                return Ok(Converted::Unavailable {
                    reason: Unavailable::UnknownSchema,
                    sqlite: None,
                });
            };
            if value.get("version").and_then(|v| v.as_u64()) != Some(1) {
                return Ok(Converted::Unavailable {
                    reason: Unavailable::UnknownVersion,
                    sqlite: None,
                });
            }
            if !value.get("tabs").is_some_and(|v| v.is_array()) {
                return Ok(Converted::Unavailable {
                    reason: Unavailable::MalformedKnownRecord,
                    sqlite: None,
                });
            }
            KnownRecord::Session { version: 1, value }
        }
        Role::Settings => {
            let Some(value) = value.get("settings").filter(|v| v.is_object()).cloned() else {
                return Ok(Converted::Unavailable {
                    reason: Unavailable::UnknownSchema,
                    sqlite: None,
                });
            };
            KnownRecord::Settings { value }
        }
        _ => {
            return Ok(Converted::Unavailable {
                reason: Unavailable::UnknownSchema,
                sqlite: None,
            })
        }
    };
    let mut used = 0;
    write_record(output, &record, &mut used, limits)?;
    Ok(Converted::Known {
        rows: 1,
        sqlite: None,
    })
}
pub(crate) fn write_record(
    output: &mut File,
    record: &KnownRecord,
    used: &mut u64,
    limits: &Limits,
) -> Result<()> {
    let bytes = serde_json::to_vec(record)?;
    if bytes.len() > limits.max_row_bytes {
        return Err(Error::Bounds);
    }
    *used = used
        .checked_add(bytes.len() as u64 + 1)
        .ok_or(Error::Bounds)?;
    if *used > limits.max_records_bytes {
        return Err(Error::Bounds);
    }
    output.write_all(&bytes)?;
    output.write_all(b"\n")?;
    Ok(())
}
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
fn sqlite(role: Role, path: &Path, output: &mut File, limits: &Limits) -> Result<Converted> {
    let c = open_snapshot(path)?;
    c.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")?;
    sqlite_connection(role, &c, output, limits)
}
pub(crate) fn open_snapshot(path: &Path) -> Result<Connection> {
    // Resolve system parent aliases (e.g. macOS /var -> /private/var) while
    // keeping the leaf subject to SQLite's NOFOLLOW check.
    let path = std::fs::canonicalize(path.parent().ok_or(Error::Invalid)?)?
        .join(path.file_name().ok_or(Error::Invalid)?);
    #[cfg(unix)]
    let native = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let native = path.to_str().ok_or(Error::Unsupported)?.as_bytes().to_vec();
    // Immutable is appropriate only for our private already-copied snapshot.
    // It prevents SQLite from creating WAL/SHM files even for a WAL-mode header.
    let encoded = native
        .iter()
        .map(|b| format!("%{b:02X}"))
        .collect::<String>();
    Ok(Connection::open_with_flags(
        format!("file:{encoded}?immutable=1&mode=ro"),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?)
}
fn sqlite_connection(
    role: Role,
    c: &Connection,
    output: &mut File,
    limits: &Limits,
) -> Result<Converted> {
    let integrity = match c.query_row::<String, _, _>("PRAGMA integrity_check(1)", [], |r| r.get(0))
    {
        Ok(check) if check == "ok" => SqliteIntegrity::Checked,
        Err(rusqlite::Error::SqliteFailure(code, Some(message)))
            if code.extended_code == 1 && message.starts_with("no such module:") =>
        {
            SqliteIntegrity::UnsupportedVirtualModule
        }
        Ok(_) => return Err(Error::Integrity),
        Err(error) => return Err(error.into()),
    };
    let user_version = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let schemas = {
        let mut s =
            c.prepare("SELECT type,name,sql FROM sqlite_schema ORDER BY type,name LIMIT 1025")?;
        let rows = s
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    if schemas.len() > 1024 {
        return Err(Error::Bounds);
    }
    let schema_bytes = serde_json::to_vec(&schemas)?;
    if schema_bytes.len() > limits.max_row_bytes {
        return Err(Error::Bounds);
    }
    let mut tables = Vec::new();
    for (kind, name, sql) in &schemas {
        if kind == "table" {
            // Querying an unavailable virtual table could load a module or fail;
            // its exact schema and shadow pages remain in the raw snapshot.
            let virtual_table = sql.as_ref().is_some_and(|s| {
                s.trim_start()
                    .to_ascii_uppercase()
                    .starts_with("CREATE VIRTUAL TABLE")
            });
            let rows: Option<u64> = if virtual_table {
                None
            } else {
                Some(
                    c.query_row(&format!("SELECT count(*) FROM {}", quote(name)), [], |r| {
                        r.get(0)
                    })?,
                )
            };
            if rows.is_some_and(|r| r > limits.max_rows) {
                return Err(Error::Bounds);
            }
            tables.push(TableInfo {
                name: name.clone(),
                rows,
            });
        }
    }
    let info = SqliteInfo {
        user_version,
        schema_sha256: hex(&Sha256::digest(&schema_bytes)),
        integrity,
        tables,
    };
    if matches!(role, Role::Catalog | Role::CloudCatalog) {
        if let Some(rows) = crate::cloud::convert(c, output, limits, &info)? {
            return Ok(Converted::Known {
                rows,
                sqlite: Some(info),
            });
        }
    }
    if role != Role::Proposals {
        return Ok(Converted::Unavailable {
            reason: Unavailable::UnknownSchema,
            sqlite: Some(info),
        });
    }
    if !matches!(user_version, 1 | 2) {
        return Ok(Converted::Unavailable {
            reason: Unavailable::UnknownVersion,
            sqlite: Some(info),
        });
    }
    let mut s = c.prepare("SELECT name FROM pragma_table_info('proposal') ORDER BY cid")?;
    let columns = s
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let required = [
        "fact_id",
        "kind",
        "schema_name",
        "extractor",
        "rule",
        "qualified",
        "source_path",
        "start_line",
        "end_line",
        "content_hash",
        "endpoint_src",
        "endpoint_dst",
        "state",
        "reviewer",
        "payload",
    ];
    if required.iter().any(|n| !columns.iter().any(|c| c == n))
        || user_version == 2 && !columns.iter().any(|c| c == "reviewed_hash")
        || columns.len() > 256
        || columns
            .iter()
            .any(|c| matches!(c.as_str(), "rowid" | "_rowid_" | "oid"))
    {
        return Ok(Converted::Unavailable {
            reason: Unavailable::UnknownSchema,
            sqlite: Some(info),
        });
    }
    c.set_limit(
        rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
        limits.max_row_bytes as i32,
    )?;
    let mut s = c.prepare("SELECT rowid,* FROM proposal ORDER BY rowid")?;
    let mut rows = s.query([])?;
    let mut count = 0u64;
    let mut used = 0u64;
    while let Some(row) = rows.next()? {
        count += 1;
        if count > limits.max_rows {
            return Err(Error::Bounds);
        }
        let mut fields = BTreeMap::new();
        let mut bytes = 0usize;
        for (i, name) in columns.iter().enumerate() {
            let value = match row.get_ref(i + 1)? {
                ValueRef::Null => SqlValue::Null,
                ValueRef::Integer(x) => SqlValue::Integer(x.to_string()),
                ValueRef::Real(x) => SqlValue::RealBits(format!("{:016x}", x.to_bits())),
                ValueRef::Text(x) => {
                    bytes = bytes.checked_add(x.len()).ok_or(Error::Bounds)?;
                    if bytes > limits.max_row_bytes / 2 {
                        return Err(Error::Bounds);
                    };
                    SqlValue::TextBytes(hex(x))
                }
                ValueRef::Blob(x) => {
                    bytes = bytes.checked_add(x.len()).ok_or(Error::Bounds)?;
                    if bytes > limits.max_row_bytes / 2 {
                        return Err(Error::Bounds);
                    };
                    SqlValue::BlobBytes(hex(x))
                }
            };
            fields.insert(name.clone(), value);
        }
        write_record(
            output,
            &KnownRecord::Proposal {
                row_id: row.get(0)?,
                fields,
            },
            &mut used,
            limits,
        )?;
    }
    Ok(Converted::Known {
        rows: count,
        sqlite: Some(info),
    })
}
