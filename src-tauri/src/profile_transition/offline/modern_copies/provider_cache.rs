//! Admitted registry bodies not yet projected (and retained older versions).
//! These are recovered snapshots, never live remote claims or source authority.
use super::*;
pub(super) fn capture(owner: &mut ReaderStore, documents: &mut Vec<Document>) -> Result<()> {
    if !owned::table(owner, "mdreader_provider_registry_meta")? {
        return Ok(());
    }
    type Row = (
        String,
        Vec<u8>,
        String,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        i64,
        Option<String>,
        Option<String>,
        Option<Vec<u8>>,
        Option<String>,
        Option<Vec<u8>>,
    );
    let rows:Vec<Row>=owner.with_library_connection(|db|->rusqlite::Result<_> {
        let version:i64=db.query_row("SELECT version FROM mdreader_provider_registry_meta WHERE id=1",[],|r|r.get(0))?;
        let count:u32=db.query_row("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name IN ('mdreader_provider_registry_meta','mdreader_provider_registry_sources','mdreader_provider_registry_snapshots','mdreader_provider_registry_rows','mdreader_provider_registry_seen','mdreader_provider_registry_state','mdreader_provider_registry_cache','mdreader_provider_registry_admissions')",[],|r|r.get(0))?;
        if !matches!((version,count),(1,7)|(2,8)) {return Err(rusqlite::Error::InvalidQuery);}
        let bytes:u64=db.query_row("SELECT coalesce(sum(length(body)),0) FROM mdreader_provider_registry_cache",[],|r|r.get(0))?;
        if bytes>MAX_BYTES as u64 {return Err(rusqlite::Error::InvalidQuery);}
        let mut q=db.prepare("SELECT CASE WHEN length(c.source)<=256 THEN c.source END,CASE WHEN length(c.binding)=32 THEN c.binding END,CASE WHEN length(c.key)<=4096 THEN c.key END,CASE WHEN length(c.version)=32 THEN c.version END,CASE WHEN length(c.body)<=1048576 THEN c.body END,CASE WHEN length(c.digest)=32 THEN c.digest END,c.observed,CASE WHEN length(c.job)<=32768 THEN c.job END,CASE WHEN length(s.descriptor)<=65536 THEN s.descriptor END,CASE WHEN length(s.digest)=32 THEN s.digest END,CASE WHEN length(r.metadata)<=65536 THEN r.metadata END,CASE WHEN length(r.digest)=32 THEN r.digest END FROM mdreader_provider_registry_cache c LEFT JOIN mdreader_provider_registry_sources s ON s.identity=c.source AND s.binding=c.binding LEFT JOIN mdreader_provider_registry_state t ON t.source=c.source LEFT JOIN mdreader_provider_registry_rows r ON r.snapshot=t.published AND r.key=c.key WHERE (c.job IS NULL OR length(c.job)<=32768) AND (s.descriptor IS NULL OR length(s.descriptor)<=65536) AND (r.metadata IS NULL OR length(r.metadata)<=65536) ORDER BY c.source,c.binding,c.key LIMIT 1001")?;
        let rows=q.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?,r.get(10)?,r.get(11)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let count:usize=db.query_row("SELECT count(*) FROM mdreader_provider_registry_cache",[],|r|r.get(0))?;
        if rows.len()!=count {return Err(rusqlite::Error::InvalidQuery);}
        Ok(rows)
    }).map_err(|_|Error::Integrity)?;
    if rows.len() > 1000 {
        return Err(Error::Bounds);
    }
    for (
        identity,
        binding,
        key,
        version,
        body,
        digest,
        observed,
        job,
        descriptor,
        descriptor_digest,
        metadata,
        metadata_digest,
    ) in rows
    {
        if binding.len() != 32
            || version.len() != 32
            || digest != Sha256::digest(&body).as_slice()
            || observed < 0
        {
            return Err(Error::Integrity);
        }
        let source = format!("provider:{}", mdreader_provider::hex(&binding));
        if let Some(descriptor) = descriptor {
            if descriptor_digest.as_deref()
                != Some(Sha256::digest(descriptor.as_bytes()).as_slice())
            {
                return Err(Error::Integrity);
            }
            let descriptor: mdreader_provider::SourceDescriptor =
                serde_json::from_str(&descriptor).map_err(|_| Error::Integrity)?;
            let actual = descriptor.binding();
            let expected = Sha256::digest(
                serde_json::to_vec(&(actual.library_id(), actual.source_id()))
                    .map_err(|_| Error::Integrity)?,
            );
            if actual.digest().as_slice() != binding
                || mdreader_provider::hex(&expected) != identity
            {
                return Err(Error::Integrity);
            }
        }
        if let Some(job) = job {
            let job: mdreader_core::ProviderProjectionReceipt =
                serde_json::from_str(&job).map_err(|_| Error::Integrity)?;
            if job.reader_source_id != source {
                return Err(Error::Integrity);
            }
            if documents.iter().any(|d| {
                d.markdown.as_bytes() == body
                    && d.lineage.as_ref().is_some_and(|l| {
                        l.kind == "provider"
                            && l.source_id == source
                            && l.path == job.path
                            && l.revision == job.revision
                    })
            }) {
                continue;
            }
        }
        let mut name = "Recovered cached provider document".to_owned();
        if let Some(metadata) = metadata {
            if metadata_digest.as_deref() != Some(Sha256::digest(metadata.as_bytes()).as_slice()) {
                return Err(Error::Integrity);
            }
            let metadata: mdreader_provider::Metadata =
                serde_json::from_str(&metadata).map_err(|_| Error::Integrity)?;
            if metadata.key.as_str() != key {
                return Err(Error::Integrity);
            }
            let metadata_version = Sha256::digest(
                serde_json::to_vec(&metadata.version).map_err(|_| Error::Integrity)?,
            );
            // A newer metadata row is not evidence for an older retained body.
            if metadata_version.as_slice() == version {
                name = format!("Recovered cached {}", metadata.name.as_str());
            }
        }
        let markdown = String::from_utf8(body).map_err(|_| Error::Integrity)?;
        owned::push(
            documents,
            &name,
            markdown,
            Lineage {
                kind: "provider_cache".into(),
                source_id: source,
                path: key,
                revision: mdreader_provider::hex(&version),
                document_id: None,
                operation_id: None,
                state: Some("recovered_cache".into()),
            },
            1,
        )?;
    }
    Ok(())
}
