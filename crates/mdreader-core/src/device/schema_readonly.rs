//! Observe known document layouts without adoption, recovery or any DDL.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::device) enum Namespace {
    Original,
    Current,
}

/// An entirely absent namespace is distinct from a malformed partial namespace.
pub(in crate::device) fn namespace(db: &rusqlite::Connection) -> Result<Option<Namespace>> {
    #[cfg(windows)]
    super::super::windows_body::check(db)?;
    let docs = table(db, "device_documents_v1")?;
    let intents = table(db, "device_import_intents_v1")?;
    let marker = table(db, "device_document_schema_v1")?;
    let Some(docs) = docs else {
        return if intents.is_none() && marker.is_none() {
            Ok(None)
        } else {
            Err(Error::InvalidSchema)
        };
    };
    let original = TABLES.split(';').next().ok_or(Error::InvalidSchema)?;
    let expected_intents = TABLES.split(';').nth(1).ok_or(Error::InvalidSchema)?;
    if intents
        .as_ref()
        .is_some_and(|sql| normal(sql) != normal(expected_intents))
    {
        return Err(Error::InvalidSchema);
    }
    let current = CURRENT_DOCUMENTS
        .split_once("CREATE TABLE")
        .map(|(_, tail)| format!("CREATE TABLE{tail}"))
        .ok_or(Error::InvalidSchema)?;
    let version = if let Some(marker) = marker {
        if normal(&marker) != normal(ADOPTION.split(';').next().ok_or(Error::InvalidSchema)?)
            || intents.is_none()
        {
            return Err(Error::InvalidSchema);
        }
        let (count, version): (u64, Option<i64>) = db.query_row(
            "SELECT count(*),max(CASE WHEN singleton=1 THEN version END) FROM device_document_schema_v1",
            [], |r| Ok((r.get(0)?, r.get(1)?)),
        ).map_err(|_| Error::InvalidSchema)?;
        if count != 1 || version.is_none() {
            return Err(Error::InvalidSchema);
        }
        version
    } else {
        None
    };
    match version {
        None | Some(1) if normal(&docs) == normal(original) => Ok(Some(Namespace::Original)),
        Some(2) if normal(&docs) == normal(&current) => Ok(Some(Namespace::Current)),
        Some(version) if version > 2 => Err(Error::UnsupportedSchema),
        _ => Err(Error::InvalidSchema),
    }
}

pub(in crate::device) fn validate_rows(db: &rusqlite::Connection, ns: Namespace) -> Result<()> {
    validation::rows(db, table(db, "device_import_intents_v1")?.is_some())?;
    if ns == Namespace::Current {
        validate_state_rows(db)?;
    }
    Ok(())
}
