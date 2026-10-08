use super::*;
use rusqlite::Connection;

pub(super) fn initialize(connection: &mut Connection) -> Result<Uuid> {
    let tx = connection.transaction().map_err(|_| ReviewError::Storage)?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='mdreader_review_owner')",
            [],
            |r| r.get(0),
        )
        .map_err(|_| ReviewError::Storage)?;
    if !exists {
        tx.execute_batch(include_str!("migrations/001-owner.sql"))
            .map_err(|_| ReviewError::Storage)?;
        tx.execute(
            "INSERT INTO mdreader_review_owner VALUES(1,1,?1)",
            [Uuid::new_v4().to_string()],
        )
        .map_err(|_| ReviewError::Storage)?;
    }
    let scope = read(&tx)?;
    tx.commit().map_err(|_| ReviewError::Storage)?;
    Ok(scope)
}
pub(super) fn read(connection: &Connection) -> Result<Uuid> {
    let (kind, sql): (String, String) = connection
        .query_row(
            "SELECT type,sql FROM sqlite_schema WHERE name='mdreader_review_owner'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| ReviewError::Storage)?;
    if kind != "table" || normal(&sql) != normal(include_str!("migrations/001-owner.sql")) {
        return Err(ReviewError::Unsupported);
    }
    let count: u64 = connection
        .query_row("SELECT count(*) FROM mdreader_review_owner", [], |r| {
            r.get(0)
        })
        .map_err(|_| ReviewError::Storage)?;
    if count != 1 {
        return Err(ReviewError::Unsupported);
    }

    let (version, scope): (u32, String) = connection
        .query_row(
            "SELECT version,owner_uuid FROM mdreader_review_owner WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| ReviewError::Storage)?;
    if version != 1 {
        return Err(ReviewError::Unsupported);
    }
    Uuid::parse_str(&scope)
        .ok()
        .filter(|id| id.get_version_num() == 4 && id.get_variant() == uuid::Variant::RFC4122)
        .ok_or(ReviewError::Storage)
}

// Match the fixed producer DDL, accepting formatting only. Preserve SQL string
// literals and doubled escapes, following the device namespace validation rule.
fn normal(sql: &str) -> String {
    let mut result = String::new();
    let mut quoted = false;
    let mut chars = sql.trim().trim_end_matches(';').chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            result.push(c);
            if quoted && chars.peek() == Some(&'\'') {
                result.push(chars.next().unwrap_or('\''));
            } else {
                quoted = !quoted;
            }
        } else if quoted {
            result.push(c);
        } else if !c.is_ascii_whitespace() {
            result.push(c.to_ascii_lowercase());
        }
    }
    result
}
