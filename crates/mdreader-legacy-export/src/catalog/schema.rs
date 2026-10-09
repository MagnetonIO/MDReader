use crate::Result;
use rusqlite::{Connection, OptionalExtension};

pub(super) const HISTORY: &[&str] = &[
    "catalog_versions",
    "schema_defs",
    "mapping_defs",
    "realizations",
    "catalog_schema_contents",
    "catalog_schema_versions",
    "catalog_mapping_versions",
    "catalog_source_versions",
    "catalog_policy_versions",
    "catalog_revisions",
    "catalog_revision_schema_versions",
    "catalog_revision_mapping_versions",
    "catalog_revision_source_versions",
    "catalog_revision_policy_versions",
    "catalog_head",
    "catalog_revision_seals",
    "catalog_provenance_rule_versions",
    "catalog_revision_provenance_rule_versions",
    "database_identity",
    "control_plane_reviews",
    "control_plane_decisions",
    "operation_log_anchor",
];
pub(super) fn columns(c: &Connection, table: &str) -> Result<Option<Vec<String>>> {
    let sql: Option<String> = c
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
            [table],
            |r| r.get(0),
        )
        .optional()?;
    let Some(sql) = sql else {
        return Ok(None);
    };
    let upper = sql.to_ascii_uppercase();
    if upper.contains("WITHOUT ROWID") || upper.trim_start().starts_with("CREATE VIRTUAL TABLE") {
        return Ok(None);
    }
    let mut s =
        c.prepare("SELECT name,hidden FROM pragma_table_xinfo(?1) ORDER BY cid LIMIT 257")?;
    let fields = s
        .query_map([table], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if fields.is_empty()
        || fields.len() > 256
        || fields.iter().any(|(name, hidden)| {
            *hidden != 0
                || matches!(
                    name.to_ascii_lowercase().as_str(),
                    "rowid" | "_rowid_" | "oid"
                )
        })
    {
        return Ok(None);
    }
    Ok(Some(fields.into_iter().map(|(name, _)| name).collect()))
}
pub(super) fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
