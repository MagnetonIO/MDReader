use super::*;
const MIGRATION: &str = include_str!("migrations/001-incoming-intents.sql");

fn definition(tx: &Transaction<'_>, name: &str) -> Result<Option<String>> {
    let row: Option<(String, Option<String>)> = tx
        .query_row(
            "SELECT type,sql FROM sqlite_schema WHERE name=?1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some((kind, Some(sql))) if kind == "table" => Ok(Some(sql)),
        _ => Err(Error::InvalidSchema),
    }
}
fn normalized(sql: &str) -> String {
    let mut result = String::new();
    let mut quoted = false;
    for c in sql.trim().trim_end_matches(';').chars() {
        if c == '\'' {
            quoted = !quoted;
            result.push(c);
        } else if quoted {
            result.push(c);
        } else if !c.is_ascii_whitespace() {
            result.push(c.to_ascii_lowercase());
        }
    }
    result
}
fn check(tx: &Transaction<'_>) -> Result<bool> {
    let marker = definition(tx, "device_incoming_schema_v1")?;
    let intents = definition(tx, "device_incoming_intents_v1")?;
    if marker.is_none() && intents.is_none() {
        return Ok(false);
    }
    let expected: Vec<_> = MIGRATION.split(';').collect();
    if marker.as_deref().map(normalized) != Some(normalized(expected[0]))
        || intents.as_deref().map(normalized) != Some(normalized(expected[1]))
    {
        return Err(Error::InvalidSchema);
    }
    let (count, version): (u64, Option<i64>) = tx.query_row(
        "SELECT count(*),max(CASE WHEN singleton=1 THEN version END) FROM device_incoming_schema_v1", [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != 1 || version.is_none() {
        return Err(Error::InvalidSchema);
    }
    if version != Some(1) {
        return Err(Error::UnsupportedSchema);
    }
    Ok(true)
}
#[cfg(feature = "device-assets")]
pub(super) fn initialize(tx: &Transaction<'_>) -> Result<()> {
    if !check(tx)? {
        tx.execute_batch(MIGRATION)?;
    }
    Ok(())
}
/// Validate before counting: future/malformed incoming metadata never silently
/// disappears from local quotas. Does not create or migrate an absent namespace.
pub(crate) fn reserved_usage(tx: &Transaction<'_>) -> Result<(u64, u64)> {
    if !check(tx)? {
        return Ok((0, 0));
    }
    let usage = tx.query_row(
        "SELECT count(*),coalesce(sum(bytes),0) FROM device_incoming_intents_v1 WHERE state IN('reserved','dropping')", [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(usage)
}
