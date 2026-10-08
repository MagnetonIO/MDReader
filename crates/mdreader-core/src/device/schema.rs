use super::*;

#[path = "migrations/validation.rs"]
mod validation;
const TABLES: &str = include_str!("migrations/001-device-documents.sql");
const ADOPTION: &str = include_str!("migrations/002-adopt-document-namespace.sql");

/// Adopt only the exact known layout, without opening files or changing grants.
/// The independent marker never changes library.sqlite's global user_version.
pub(super) fn initialize(store: &mut ReaderStore) -> Result<()> {
    store.with_library_connection(|db| {
        let tx = db.transaction()?;
        initialize_in(&tx)?;
        tx.commit()?;
        Ok(())
    })
}
pub(super) fn initialize_in(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    let docs = table(tx, "device_documents_v1")?;
    let intents = table(tx, "device_import_intents_v1")?;
    let marker = table(tx, "device_document_schema_v1")?;
    let expected: Vec<_> = TABLES.split(';').filter(|s| !s.trim().is_empty()).collect();
    if docs
        .as_ref()
        .is_some_and(|sql| normal(sql) != normal(expected[0]))
        || intents
            .as_ref()
            .is_some_and(|sql| normal(sql) != normal(expected[1]))
        || (docs.is_none() && intents.is_some())
    {
        return Err(Error::InvalidSchema);
    }
    if let Some(marker) = marker {
        if normal(&marker) != normal(ADOPTION.split(';').next().ok_or(Error::InvalidSchema)?)
            || docs.is_none()
            || intents.is_none()
        {
            return Err(Error::InvalidSchema);
        }
        let (count, version):(u64,Option<i64>) = tx.query_row(
        "SELECT count(*),max(CASE WHEN singleton=1 THEN version END) FROM device_document_schema_v1",[],|r|Ok((r.get(0)?,r.get(1)?))
    ).map_err(|_|Error::InvalidSchema)?;
        if count != 1 || version.is_none() {
            return Err(Error::InvalidSchema);
        }
        if version != Some(1) {
            return Err(Error::UnsupportedSchema);
        }
        return Ok(());
    }
    if docs.is_some() {
        validation::rows(tx, intents.is_some())?;
    }
    // The original nontransactional initialization could leave only the
    // exact documents table. Add an empty intents table, never fake history.
    // All layout/version/data validation precedes any schema write.
    tx.execute_batch(TABLES)?;
    tx.execute_batch(ADOPTION)?;
    Ok(())
}

fn table(tx: &rusqlite::Transaction<'_>, name: &str) -> Result<Option<String>> {
    let definition: Option<(String, Option<String>)> = tx
        .query_row(
            "SELECT type,sql FROM sqlite_schema WHERE name=?1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|_| Error::InvalidSchema)?;
    match definition {
        None => Ok(None),
        Some((kind, Some(sql))) if kind == "table" => Ok(Some(sql)),
        Some(_) => Err(Error::InvalidSchema),
    }
}
// Accept formatting differences in the actual fixed producer DDL, not arbitrary
// schema variants. Preserve string literal contents and doubled quote escapes.
fn normal(sql: &str) -> String {
    let mut normalized = String::new();
    let mut quoted = false;
    let mut chars = sql.trim().trim_end_matches(';').chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            normalized.push(c);
            if quoted && chars.peek() == Some(&'\'') {
                normalized.push(chars.next().unwrap_or('\''));
            } else {
                quoted = !quoted;
            }
        } else if quoted {
            normalized.push(c);
        } else if !c.is_ascii_whitespace() {
            normalized.push(c.to_ascii_lowercase());
        }
    }
    normalized.replacen("createtableifnotexists", "createtable", 1)
}
