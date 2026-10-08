use super::*;

#[path = "migrations/validation.rs"]
mod validation;
const TABLES: &str = include_str!("migrations/001-device-documents.sql");
const ADOPTION: &str = include_str!("migrations/002-adopt-document-namespace.sql");
const STATE: &str = include_str!("migrations/003-device-state.sql");
const CURRENT_DOCUMENTS: &str = include_str!("migrations/document-layout-v2.sql");

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
    let current_documents = CURRENT_DOCUMENTS
        .split_once("CREATE TABLE")
        .map(|(_, tail)| format!("CREATE TABLE{tail}"))
        .ok_or(Error::InvalidSchema)?;
    if docs.as_ref().is_some_and(|sql| {
        normal(sql) != normal(expected[0]) && normal(sql) != normal(&current_documents)
    }) || intents
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
        if !matches!(version, Some(1 | 2)) {
            return Err(Error::UnsupportedSchema);
        }
        let v2 = docs
            .as_ref()
            .is_some_and(|sql| normal(sql) == normal(&current_documents));
        if v2 != (version == Some(2)) {
            return Err(Error::InvalidSchema);
        }
        if version == Some(1) {
            validation::rows(tx, true)?;
            tx.execute_batch(STATE)?;
        }
        return validate_state_rows(tx);
    }
    if docs.is_some() {
        if docs
            .as_ref()
            .is_some_and(|sql| normal(sql) != normal(expected[0]))
        {
            return Err(Error::InvalidSchema);
        }
        validation::rows(tx, intents.is_some())?;
    }
    // The original nontransactional initialization could leave only the
    // exact documents table. Add an empty intents table, never fake history.
    // All layout/version/data validation precedes any schema write.
    tx.execute_batch(TABLES)?;
    tx.execute_batch(ADOPTION)?;
    tx.execute_batch(STATE)?;
    validate_state_rows(tx)
}

fn validate_state_rows(db: &rusqlite::Connection) -> Result<()> {
    let count: u64 = db
        .query_row("SELECT count(*) FROM device_documents_v1", [], |r| r.get(0))
        .map_err(|_| Error::InvalidSchema)?;
    if count > MAX_DOCUMENTS {
        return Err(Error::InvalidSchema);
    }
    let bad:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM device_documents_v1 WHERE typeof(state_generation)!='integer' OR state_generation NOT BETWEEN 0 AND 9007199254740991 OR retired NOT IN(0,1) OR (retired=0 AND retirement IS NOT NULL) OR (retired=1 AND retirement IS NULL) OR length(CAST(retirement AS BLOB))>2048)",[],|r|r.get(0)).map_err(|_|Error::InvalidSchema)?;
    if bad {
        return Err(Error::InvalidSchema);
    }
    let mut query = db
        .prepare("SELECT retirement FROM device_documents_v1 WHERE retired=1 LIMIT 1001")
        .map_err(|_| Error::InvalidSchema)?;
    let records = query
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|_| Error::InvalidSchema)?;
    for record in records {
        let value: super::state::RetirementRecord =
            serde_json::from_str(&record.map_err(|_| Error::InvalidSchema)?)
                .map_err(|_| Error::InvalidSchema)?;
        value.validate()?;
    }
    Ok(())
}

/// Read-only source gate. Ordinary local/provider sources are untouched; known
/// pre-transition device layouts can still be read without implicit migration.
pub(super) fn source_authority(db: &rusqlite::Connection, source: &str) -> Result<()> {
    if !owns_source(db, source)? {
        return Ok(());
    }
    let docs = table(db, "device_documents_v1")?.ok_or(Error::InvalidSchema)?;
    let expected_intents = TABLES.split(';').nth(1).ok_or(Error::InvalidSchema)?;
    let intents = table(db, "device_import_intents_v1")?;
    if intents
        .as_ref()
        .is_some_and(|sql| normal(sql) != normal(expected_intents))
    {
        return Err(Error::InvalidSchema);
    }
    let v1 = normal(TABLES.split(';').next().ok_or(Error::InvalidSchema)?);
    let v2 = normal(
        &CURRENT_DOCUMENTS
            .split_once("CREATE TABLE")
            .map(|(_, tail)| format!("CREATE TABLE{tail}"))
            .ok_or(Error::InvalidSchema)?,
    );
    let marker = table(db, "device_document_schema_v1")?;
    if marker
        .as_ref()
        .is_some_and(|sql| normal(sql) != normal(ADOPTION.split(';').next().unwrap_or("")))
    {
        return Err(Error::InvalidSchema);
    }
    if marker.is_some() {
        if intents.is_none() {
            return Err(Error::InvalidSchema);
        }
        let count: u64 = db
            .query_row("SELECT count(*) FROM device_document_schema_v1", [], |r| {
                r.get(0)
            })
            .map_err(|_| Error::InvalidSchema)?;
        if count != 1 {
            return Err(Error::InvalidSchema);
        }
    }
    let version: Option<i64> = if marker.is_some() {
        Some(
            db.query_row(
                "SELECT version FROM device_document_schema_v1 WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .map_err(|_| Error::InvalidSchema)?,
        )
    } else {
        None
    };
    match version {
        None | Some(1) if normal(&docs) == v1 => Ok(()),
        Some(2) if normal(&docs) == v2 => {
            let (retired,record,generation):(bool,Option<String>,u64)=db.query_row("SELECT retired,retirement,state_generation FROM device_documents_v1 WHERE source_id=?1 AND length(CAST(coalesce(retirement,'') AS BLOB))<=2048",[source],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_|Error::InvalidSchema)?;
            if retired {
                Err(Error::Unavailable)
            } else if record.is_some() || generation > super::state::MAX_GENERATION {
                Err(Error::InvalidSchema)
            } else {
                Ok(())
            }
        }
        Some(version) if version > 2 => Err(Error::UnsupportedSchema),
        _ => Err(Error::InvalidSchema),
    }
}

fn table(tx: &rusqlite::Connection, name: &str) -> Result<Option<String>> {
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

pub(super) fn owns_source(db: &rusqlite::Connection, source: &str) -> Result<bool> {
    if table(db, "device_documents_v1")?.is_none() {
        return Ok(false);
    }
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM device_documents_v1 WHERE source_id=?1)",
        [source],
        |r| r.get(0),
    )
    .map_err(|_| Error::InvalidSchema)
}
