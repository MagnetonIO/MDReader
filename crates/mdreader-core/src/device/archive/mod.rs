//! Explicit archival aliases for newly materialized reader copies. These are
//! relationships between exact already granted documents, never old path grants.
use super::{catalog, DeviceDocument, Error, ReaderStore, Result};
use rusqlite::{params, OptionalExtension};
use std::path::Path;
pub mod history;
const SQL: &str = include_str!("schema.sql");
pub struct Alias {
    pub receipt: catalog::Receipt,
    pub artifact_index: u32,
    pub legacy_id: String,
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn schema(c: &rusqlite::Connection, create: bool) -> Result<bool> {
    let exists: u32 = c.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE name GLOB 'reader_import_alias*'",
        [],
        |r| r.get(0),
    )?;
    if exists == 0 {
        if !create {
            return Ok(false);
        }
        c.execute_batch(SQL)?;
        c.execute("INSERT INTO reader_import_alias_schema_v1 VALUES(1,1)", [])?;
    }
    if c.query_row::<u32, _, _>(
        "SELECT count(*) FROM sqlite_schema WHERE name GLOB 'reader_import_alias*'",
        [],
        |r| r.get(0),
    )? != 2
    {
        return Err(Error::UnsupportedSchema);
    }
    for sql in SQL.split(';').filter(|s| !s.trim().is_empty()) {
        let name = sql
            .split_whitespace()
            .nth(2)
            .and_then(|s| s.split('(').next())
            .ok_or(Error::InvalidSchema)?;
        let actual: Option<String> = c
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
                [name],
                |r| r.get(0),
            )
            .optional()?;
        if actual.as_deref().map(str::trim) != Some(sql.trim()) {
            return Err(Error::UnsupportedSchema);
        }
    }
    let triggers:u32=c.query_row("SELECT count(*) FROM sqlite_schema WHERE type='trigger' AND tbl_name IN('reader_import_alias_schema_v1','reader_import_aliases_v1')",[],|r|r.get(0))?;
    if triggers != 0 {
        return Err(Error::UnsupportedSchema);
    }
    let versions: Vec<i64> = {
        let mut s =
            c.prepare("SELECT version FROM reader_import_alias_schema_v1 WHERE singleton=1")?;
        let rows = s
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    if versions != [1] {
        return Err(Error::UnsupportedSchema);
    }
    let invalid:u64=c.query_row("SELECT count(*) FROM reader_import_aliases_v1 WHERE length(document_id)!=36 OR length(source_id)!=64 OR length(revision)!=64 OR length(archive_id)!=36 OR length(archive_sha256)!=64 OR artifact_index<0 OR artifact_index>127 OR length(legacy_id)=0 OR length(legacy_id)>16384",[],|r|r.get(0))?;
    if invalid != 0 {
        return Err(Error::InvalidSchema);
    }
    Ok(true)
}
/// Used only after explicit selected archive conversion. Each alias binds an
/// already published native document at the exact retained owner/root/revision.
pub fn record(
    store: &mut ReaderStore,
    directory: &Path,
    archive_id: &str,
    archive_sha256: &str,
    aliases: &[Alias],
) -> Result<()> {
    if uuid::Uuid::parse_str(archive_id).is_err() || !hash(archive_sha256) || aliases.len() > 1000 {
        return Err(Error::InvalidSelection);
    }
    for alias in aliases {
        if alias.artifact_index > 127 || alias.legacy_id.is_empty() || alias.legacy_id.len() > 16384
        {
            return Err(Error::InvalidSelection);
        }
        catalog::read_in(store, directory, &alias.receipt)?;
    }
    let roots = aliases
        .iter()
        .map(|a| {
            store
                .device_source_root(&a.receipt.source_id)
                .map_err(Error::from)
        })
        .collect::<Result<Vec<_>>>()?;
    store.with_library_connection(|db|->Result<()> {
        let tx=db.transaction()?;
        schema(&tx,true)?;
        for alias in aliases {
            let r=&alias.receipt;
            tx.execute("INSERT INTO reader_import_aliases_v1 VALUES(?1,?2,?3,?4,?5,?6,?7)",params![r.document_id,r.source_id,r.revision,archive_id,archive_sha256,alias.artifact_index,alias.legacy_id])?;
        }
        for (alias,root) in aliases.iter().zip(&roots) {
            let r=&alias.receipt;
            let current:u32=tx.query_row("SELECT count(*) FROM device_documents_v1 WHERE id=?1 AND source_id=?2 AND revision=?3 AND retired=0",params![r.document_id,r.source_id,r.revision],|r|r.get(0))?;
            if current!=1 {return Err(Error::Conflict);}
            super::source_authority(&tx,&r.source_id).map_err(Error::from)?;
            root.verify().map_err(Error::from)?;
        }
        tx.commit()?;Ok(())
    })?;
    Ok(())
}
/// No initialization/migration, network, original path interpretation, or
/// missing-source regrant. Only generated relative archive links are supported.
pub fn resolve(
    store: &ReaderStore,
    directory: &Path,
    origin: &catalog::Receipt,
    kind: &str,
    target: &str,
) -> Result<Vec<DeviceDocument>> {
    if kind != "relative" || target.len() > 512 {
        return Err(Error::InvalidSelection);
    }
    let fields = target.split('/').collect::<Vec<_>>();
    if fields.len() != 3 || fields[0] != "legacy" {
        return Ok(vec![]);
    }
    let artifact: u32 = fields[1].parse().map_err(|_| Error::InvalidSelection)?;
    if artifact > 127 || artifact.to_string() != fields[1] {
        return Err(Error::InvalidSelection);
    }
    let id = fields[2]
        .strip_suffix(".md")
        .ok_or(Error::InvalidSelection)?;
    let uuid = uuid::Uuid::parse_str(id).map_err(|_| Error::InvalidSelection)?;
    if uuid.to_string() != id {
        return Err(Error::InvalidSelection);
    }
    let body = catalog::read_in(store, directory, origin)?;
    let parsed = crate::parse_markdown(&body.markdown, &body.document.name);
    if !parsed
        .sections
        .iter()
        .flat_map(|s| &s.links)
        .any(|l| l.raw_target == target)
    {
        return Ok(vec![]);
    }
    let db = store.device_connection();
    if !schema(db, false)? {
        return Ok(vec![]);
    }
    let identity:Option<(String,String)>=db.query_row(
        "SELECT archive_id,archive_sha256 FROM reader_import_aliases_v1 WHERE document_id=?1 AND source_id=?2 AND revision=?3",
        params![origin.document_id,origin.source_id,origin.revision],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((archive, sha)) = identity else {
        return Ok(vec![]);
    };
    let receipt:Option<catalog::Receipt>=db.query_row(
        "SELECT document_id,source_id,revision FROM reader_import_aliases_v1 WHERE archive_id=?1 AND archive_sha256=?2 AND artifact_index=?3 AND legacy_id=?4",
        params![archive,sha,artifact,id],|r|Ok(catalog::Receipt{document_id:r.get(0)?,source_id:r.get(1)?,revision:r.get(2)?})).optional()?;
    let mut found = vec![];
    if let Some(receipt) = receipt {
        found.push(catalog::read_in(store, directory, &receipt)?.document);
    }
    if catalog::read_in(store, directory, origin)?.markdown != body.markdown {
        return Err(Error::Conflict);
    }
    Ok(found)
}
