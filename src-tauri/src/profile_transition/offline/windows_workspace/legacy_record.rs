use super::*;
use rusqlite::{params, Connection, OptionalExtension};
pub(super) const NAME: &str = "native_windows_legacy_v1";
const DDL: &str = "CREATE TABLE native_windows_legacy_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1),record TEXT NOT NULL CHECK(length(CAST(record AS BLOB))<=262144))";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    pub(super) operation: Uuid,
    pub(super) launch: mdreader_profile::legacy_windows::LaunchRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) draft: Option<Draft>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Draft {
    pub(super) native_operation: Uuid,
    pub(super) lock: mdreader_capture::WindowsObjectStamp,
    pub(super) conversion: Option<Conversion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) selected: Option<Vec<Selection>>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Conversion {
    pub(super) relative: String,
    pub(super) identity: mdreader_legacy_export::reviews::ArchiveIdentity,
}
fn validate_draft(draft: &Draft) -> Result<()> {
    if draft.native_operation.get_version_num() != 4 {
        return Err(Error::Integrity);
    }
    if let Some(selected) = &draft.selected {
        selections(selected)?;
    }
    if let Some(conversion) = &draft.conversion {
        let name = &conversion.relative;
        if !(name == "conversion" || name.starts_with("conversion-"))
            || name.len() > 64
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || conversion.identity.export_id.get_version_num() != 4
            || conversion.identity.manifest_sha256.len() != 64
            || !conversion
                .identity
                .manifest_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err(Error::Integrity);
        }
    }
    Ok(())
}
fn schema(db: &Connection) -> Result<bool> {
    let definition: Option<String> = db
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
            [NAME],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| Error::Storage)?;
    let normal = |s: &str| {
        s.chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect::<String>()
    };
    if definition
        .as_ref()
        .is_some_and(|s| normal(s) != normal(DDL))
    {
        return Err(Error::Conflict);
    }
    for namespace in ["sqlite_schema", "sqlite_temp_schema"] {
        let query = format!("SELECT EXISTS(SELECT 1 FROM {namespace} WHERE (name GLOB 'native_windows_legacy_*' OR tbl_name GLOB 'native_windows_legacy_*') AND NOT(?2='sqlite_schema' AND type='table' AND name=?1))");
        if db
            .query_row(&query, [NAME, namespace], |r| r.get::<_, bool>(0))
            .map_err(|_| Error::Storage)?
        {
            return Err(Error::Conflict);
        }
    }
    Ok(definition.is_some())
}
pub(super) fn read(db: &Connection) -> Result<Option<Record>> {
    if !schema(db)? {
        return Ok(None);
    }
    let count: u64 = db
        .query_row("SELECT count(*) FROM native_windows_legacy_v1", [], |r| {
            r.get(0)
        })
        .map_err(|_| Error::Storage)?;
    if count != 1 {
        return Err(Error::Conflict);
    }
    let text: String = db.query_row("SELECT record FROM native_windows_legacy_v1 WHERE singleton=1 AND version=1 AND length(CAST(record AS BLOB))<=262144", [], |r| r.get(0)).map_err(|_| Error::Conflict)?;
    let record: Record = serde_json::from_str(&text).map_err(|_| Error::Integrity)?;
    if record.operation.get_version_num() != 4 {
        return Err(Error::Integrity);
    }
    record.launch.validate()?;
    if let Some(draft) = &record.draft {
        validate_draft(draft)?;
    }
    Ok(Some(record))
}
pub(super) fn record(
    db: &mut Connection,
    operation: Uuid,
    launch: &mdreader_profile::legacy_windows::LaunchRecord,
    draft: Draft,
) -> Result<()> {
    launch.validate()?;
    validate_draft(&draft)?;
    let record = Record {
        operation,
        launch: launch.clone(),
        draft: Some(draft),
    };
    let text = serde_json::to_string(&record).map_err(|_| Error::Invalid)?;
    if text.len() > 262144 {
        return Err(Error::Bounds);
    }
    let tx = db.transaction().map_err(|_| Error::Storage)?;
    if !schema(&tx)? {
        tx.execute_batch(DDL).map_err(|_| Error::Storage)?;
    }
    tx.execute(
        "INSERT INTO native_windows_legacy_v1 VALUES(1,1,?1)",
        params![text],
    )
    .map_err(|_| Error::Storage)?;
    let persisted = read(&tx)?.ok_or(Error::Conflict)?;
    if serde_json::to_vec(&persisted).map_err(|_| Error::Invalid)?
        != serde_json::to_vec(&record).map_err(|_| Error::Invalid)?
    {
        return Err(Error::Conflict);
    }
    tx.commit().map_err(|_| Error::ReconciliationRequired)
}

/// Commit the exact finished conversion association only after verification.
pub(super) fn conversion(db: &mut Connection, operation: Uuid, value: Conversion) -> Result<()> {
    let tx = db.transaction().map_err(|_| Error::Storage)?;
    let mut record = read(&tx)?.ok_or(Error::Conflict)?;
    if record.operation != operation {
        return Err(Error::Conflict);
    }
    let draft = record.draft.as_mut().ok_or(Error::Unsupported)?;
    if draft.conversion.is_some() {
        return Err(Error::Conflict);
    }
    draft.conversion = Some(value);
    validate_draft(draft)?;
    let text = serde_json::to_string(&record).map_err(|_| Error::Invalid)?;
    if text.len() > 262144 {
        return Err(Error::Bounds);
    }
    if tx
        .execute(
            "UPDATE native_windows_legacy_v1 SET record=?1 WHERE singleton=1 AND version=1",
            params![text],
        )
        .map_err(|_| Error::Storage)?
        != 1
    {
        return Err(Error::Conflict);
    }
    if serde_json::to_vec(&read(&tx)?.ok_or(Error::Conflict)?).map_err(|_| Error::Invalid)?
        != serde_json::to_vec(&record).map_err(|_| Error::Invalid)?
    {
        return Err(Error::Conflict);
    }
    tx.commit().map_err(|_| Error::ReconciliationRequired)
}

fn selections(selected: &[Selection]) -> Result<()> {
    let mut ids = std::collections::BTreeSet::new();
    if selected.is_empty() || selected.len() > 1000 {
        return Err(Error::Bounds);
    }
    for selection in selected {
        if selection.document_id.is_empty()
            || selection.document_id.len() > 16384
            || !ids.insert(&selection.document_id)
            || selection.revision.len() != 64
            || !selection
                .revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Invalid);
        }
    }
    Ok(())
}
/// Commit the exact user selection before candidate creation. Retrying a
/// completed candidate may reconcile only this same selection, never another.
pub(super) fn select(
    db: &mut Connection,
    operation: Uuid,
    selected: &[Selection],
    candidate_exists: bool,
) -> Result<()> {
    selections(selected)?;
    let mut selected = selected.to_vec();
    selected.sort_by(|a, b| a.document_id.cmp(&b.document_id));
    let encoded = serde_json::to_vec(&selected).map_err(|_| Error::Invalid)?;
    let tx = db.transaction().map_err(|_| Error::Storage)?;
    let mut record = read(&tx)?.ok_or(Error::Conflict)?;
    if record.operation != operation {
        return Err(Error::Conflict);
    }
    let draft = record.draft.as_mut().ok_or(Error::Unsupported)?;
    let same = draft
        .selected
        .as_ref()
        .map(|previous| {
            serde_json::to_vec(previous)
                .map(|previous| previous == encoded)
                .map_err(|_| Error::Integrity)
        })
        .transpose()?
        .unwrap_or(false);
    if !same {
        if candidate_exists {
            return Err(Error::Conflict);
        }
        draft.selected = Some(selected);
        validate_draft(draft)?;
        let text = serde_json::to_string(&record).map_err(|_| Error::Invalid)?;
        if text.len() > 262144 {
            return Err(Error::Bounds);
        }
        if tx
            .execute(
                "UPDATE native_windows_legacy_v1 SET record=?1 WHERE singleton=1 AND version=1",
                params![text],
            )
            .map_err(|_| Error::Storage)?
            != 1
        {
            return Err(Error::Conflict);
        }
    }
    let actual = read(&tx)?.ok_or(Error::Conflict)?;
    if serde_json::to_vec(&actual).map_err(|_| Error::Integrity)?
        != serde_json::to_vec(&record).map_err(|_| Error::Integrity)?
    {
        return Err(Error::Conflict);
    }
    tx.commit().map_err(|_| Error::ReconciliationRequired)
}
