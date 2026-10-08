use super::*;

/// Validate bounded durable metadata only. Never open paths or infer consent.
pub(super) fn rows(tx: &rusqlite::Transaction<'_>, intents_present: bool) -> Result<()> {
    let count: u64 = tx
        .query_row("SELECT count(*) FROM device_documents_v1", [], |r| r.get(0))
        .map_err(|_| Error::InvalidSchema)?;
    if count > MAX_DOCUMENTS {
        return Err(Error::InvalidSchema);
    }
    let bad: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM device_documents_v1 d LEFT JOIN sources s ON s.id=d.source_id WHERE typeof(d.id)!='text' OR length(CAST(d.id AS BLOB))>64 OR typeof(d.name)!='text' OR length(CAST(d.name AS BLOB))>240 OR typeof(d.source_id)!='text' OR length(CAST(d.source_id AS BLOB))>256 OR typeof(d.revision)!='text' OR length(d.revision)!=64 OR typeof(d.bytes)!='integer' OR d.bytes NOT BETWEEN 0 AND 1048576 OR typeof(d.pinned)!='integer' OR d.pinned NOT IN(0,1) OR typeof(d.line)!='integer' OR d.line NOT BETWEEN 1 AND 4294967295 OR typeof(d.indexed)!='integer' OR d.indexed NOT IN(0,1) OR s.id IS NULL)",[],|r|r.get(0)).map_err(|_|Error::InvalidSchema)?;
    if bad {
        return Err(Error::InvalidSchema);
    }
    let mut query = tx
        .prepare("SELECT id,name,revision FROM device_documents_v1")
        .map_err(|_| Error::InvalidSchema)?;
    let records = query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|_| Error::InvalidSchema)?;
    for record in records {
        let (id, name, revision) = record.map_err(|_| Error::InvalidSchema)?;
        valid(&id, &name, &revision)?;
    }
    if intents_present {
        intents(tx)?;
    }
    let bytes:u64=if intents_present {
        tx.query_row("SELECT coalesce(sum(bytes),0) FROM (SELECT bytes FROM device_documents_v1 UNION ALL SELECT bytes FROM device_import_intents_v1 WHERE state='reserved')",[],|r|r.get(0))
    } else { tx.query_row("SELECT coalesce(sum(bytes),0) FROM device_documents_v1",[],|r|r.get(0)) }.map_err(|_|Error::InvalidSchema)?;
    if bytes > MAX_LIBRARY_BYTES {
        return Err(Error::InvalidSchema);
    }
    Ok(())
}
fn intents(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    let count: u64 = tx
        .query_row("SELECT count(*) FROM device_import_intents_v1", [], |r| {
            r.get(0)
        })
        .map_err(|_| Error::InvalidSchema)?;
    if count > MAX_DOCUMENTS {
        return Err(Error::InvalidSchema);
    }
    let bad:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM device_import_intents_v1 WHERE typeof(id)!='text' OR length(CAST(id AS BLOB))>64 OR typeof(name)!='text' OR length(CAST(name AS BLOB))>240 OR typeof(revision)!='text' OR length(revision)!=64 OR typeof(bytes)!='integer' OR bytes NOT BETWEEN 0 AND 1048576 OR typeof(parent_identity)!='text' OR length(CAST(parent_identity AS BLOB))>16384 OR (root_identity IS NOT NULL AND (typeof(root_identity)!='text' OR length(CAST(root_identity AS BLOB))>16384)) OR state NOT IN('reserved','published') OR (state='published' AND root_identity IS NULL))",[],|r|r.get(0)).map_err(|_|Error::InvalidSchema)?;
    if bad {
        return Err(Error::InvalidSchema);
    }
    let mut query = tx
        .prepare(
            "SELECT id,name,revision,parent_identity,root_identity FROM device_import_intents_v1",
        )
        .map_err(|_| Error::InvalidSchema)?;
    let records = query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|_| Error::InvalidSchema)?;
    for record in records {
        let (id, name, revision, parent, root) = record.map_err(|_| Error::InvalidSchema)?;
        valid(&id, &name, &revision)?;
        serde_json::from_str::<SourceRootIdentity>(&parent).map_err(|_| Error::InvalidSchema)?;
        if let Some(root) = root {
            serde_json::from_str::<SourceRootIdentity>(&root).map_err(|_| Error::InvalidSchema)?;
        }
    }
    let reserved: u64 = tx
        .query_row(
            "SELECT count(*) FROM device_import_intents_v1 WHERE state='reserved'",
            [],
            |r| r.get(0),
        )
        .map_err(|_| Error::InvalidSchema)?;
    let documents: u64 = tx
        .query_row("SELECT count(*) FROM device_documents_v1", [], |r| r.get(0))
        .map_err(|_| Error::InvalidSchema)?;
    if documents + reserved > MAX_DOCUMENTS {
        return Err(Error::InvalidSchema);
    }
    Ok(())
}
fn valid(id: &str, name: &str, revision: &str) -> Result<()> {
    if uuid::Uuid::parse_str(id).is_err()
        || name.is_empty()
        || !name.to_ascii_lowercase().ends_with(".md")
        || name
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
        || !revision
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(Error::InvalidSchema);
    }
    Ok(())
}
