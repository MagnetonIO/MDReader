//! Durable provider cache evidence becomes independent offline Markdown. No
//! remote fetch, retained credential, source grant, or mutation is imported.
use super::*;

pub(super) fn capture(
    owner: &mut ReaderStore,
    profile: &SourceRoot,
    documents: &mut Vec<Document>,
) -> Result<()> {
    if !owned::table(owner, "core_provider_schema")? {
        return Ok(());
    }
    type Row = (String, String, String, String, String, String, bool, String);
    let rows:Vec<Row> = owner.with_library_connection(|db| -> rusqlite::Result<_> {
        let version: i64 = db.query_row("SELECT version FROM core_provider_schema",[],|r|r.get(0))?;
        if version != 1 { return Err(rusqlite::Error::InvalidQuery); }
        let mut q=db.prepare("SELECT CASE WHEN length(p.reader_source_id)<=256 THEN p.reader_source_id END,CASE WHEN length(p.path)<=4096 THEN p.path END,CASE WHEN length(p.revision)=64 THEN p.revision END,CASE WHEN length(p.body_hash)=64 THEN p.body_hash END,CASE WHEN length(p.cache_path)<=4096 THEN p.cache_path END,CASE WHEN length(p.metadata_json)<=32768 THEN p.metadata_json END,coalesce((g.enabled=1 AND granted.enabled=1),0),CASE WHEN length(s.binding_digest)=64 THEN s.binding_digest END FROM core_provider_records p JOIN core_provider_sources s ON s.reader_source_id=p.reader_source_id LEFT JOIN core_provider_slots g ON g.library_id=s.library_id AND g.service_source_id=s.service_source_id AND g.binding_digest=s.binding_digest JOIN sources granted ON granted.id=s.reader_source_id ORDER BY p.reader_source_id,p.record_key LIMIT 1001")?;
        let rows=q.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let count:usize=db.query_row("SELECT count(*) FROM core_provider_records",[],|r|r.get(0))?;
        if count != rows.len() {return Err(rusqlite::Error::InvalidQuery);}
        Ok(rows)
    }).map_err(|_|Error::Integrity)?;
    if rows.len() > 1000 {
        return Err(Error::Bounds);
    }
    for (source, path, revision, hash, cache, metadata, enabled, binding) in rows {
        let metadata: mdreader_provider::Metadata =
            serde_json::from_str(&metadata).map_err(|_| Error::Integrity)?;
        let root = owned::source_root(
            owner,
            &source,
            &profile.path.join("reader-v1/provider-cache"),
        )?;
        let key_hash = blake3::hash(metadata.key.as_str().as_bytes())
            .to_hex()
            .to_string();
        if source != format!("provider:{binding}")
            || cache != format!("{binding}/{key_hash}/{revision}.md")
        {
            return Err(Error::Conflict);
        }
        let bytes = owned::read(&root, Path::new(&cache), 1024 * 1024)?;
        let body_hash = blake3::hash(&bytes).to_hex().to_string();
        let expected = blake3::hash(
            &serde_json::to_vec(&(&metadata.version, &body_hash)).map_err(|_| Error::Integrity)?,
        )
        .to_hex()
        .to_string();
        if body_hash != hash
            || expected != revision
            || path
                != format!(
                    "records/{}.md",
                    blake3::hash(metadata.key.as_str().as_bytes()).to_hex()
                )
        {
            return Err(Error::Conflict);
        }
        let markdown = String::from_utf8(bytes).map_err(|_| Error::Integrity)?;
        if enabled {
            let current = owner
                .read_document(&source, &path, Some(&revision))
                .map_err(|_| Error::Conflict)?;
            if current.markdown != markdown {
                return Err(Error::Conflict);
            }
        }
        owned::push(
            documents,
            &format!(
                "{} {}",
                if enabled {
                    "Cached"
                } else {
                    "Recovered cached"
                },
                metadata.name.as_str()
            ),
            markdown,
            Lineage {
                kind: "provider".into(),
                source_id: source,
                path,
                revision,
                document_id: None,
                operation_id: None,
                state: Some(if enabled { "cached" } else { "disabled" }.into()),
            },
            1,
        )?;
    }
    Ok(())
}
