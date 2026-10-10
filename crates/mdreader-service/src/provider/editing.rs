//! Exact selected-provider editing and readonly durable save reconciliation.
//! Cache paths identify existing admitted records; they never grant new scope.
use super::*;

pub(super) fn begin_edit_exact(
    c: &mut Connection,
    reader_source_id: &SourceId,
    path: &RecordKey,
    revision: &OpaqueValue,
) -> Result<Value> {
    let library = library_id(c)?;
    let source = list(c, &library)?
        .into_iter()
        .find(|s| s.enabled && s.reader_source_id.as_ref() == Some(reader_source_id))
        .ok_or(ProviderError::Unauthorized)?;
    let configuration = configuration(c, &source.source_id)?;
    let d = &configuration.descriptor;
    let b = d.binding();
    revalidate(c, b)?;
    if matches!(d.scope(), SelectedScope::ImportedLegacyBroad { .. }) {
        return Err(ProviderError::Unsupported);
    }
    // Require a unique exact admitted receipt, never a renderer-inferred remote ID.
    let keys: Vec<String> = {
        let mut query = registry::db(c.prepare("SELECT key FROM mdreader_provider_registry_cache WHERE source=?1 AND binding=?2 AND json_extract(job,'$.reader_source_id')=?3 AND json_extract(job,'$.path')=?4 AND json_extract(job,'$.revision')=?5 LIMIT 2"))?;
        let rows = registry::db(query.query_map(
            params![
                registry::identity(b),
                b.digest().as_slice(),
                reader_source_id.as_str(),
                path.as_str(),
                revision.as_str()
            ],
            |row| row.get(0),
        ))?;
        rows.map(registry::db).collect::<Result<_>>()?
    };
    if keys.len() != 1 {
        return Err(ProviderError::Conflict);
    }
    let key = RecordKey::new(keys[0].clone())?;
    let current = cached(c, b, &key)?.ok_or(ProviderError::Conflict)?;
    let receipt = current.projection.ok_or(ProviderError::Conflict)?;
    if !links::allowed(d, &current.metadata)
        || receipt.reader_source_id != *reader_source_id
        || receipt.path != *path
        || receipt.revision != *revision
    {
        return Err(ProviderError::Conflict);
    }
    let handle = handle_for(c, b, &key)?;
    let mut result = begin_edit(c, d, handle)?;
    let fields = result.as_object_mut().ok_or(ProviderError::Protocol)?;
    fields.insert("write_source_id".into(), json!(source.source_id));
    fields.insert("projection".into(), json!(receipt));
    Ok(result)
}

pub(super) fn inspect_save(c: &mut Connection, binding: &SourceBinding, id: Uuid) -> Result<Value> {
    revalidate(c, binding)?;
    let Some(pending) = journal::inspect(c, binding, id)? else {
        return Ok(json!({"operation_id":id,"state":"absent","projection":null}));
    };
    // An accepted historical write must not be relabeled with a newer cache head.
    let projection = if pending.state == journal::MutationState::Published {
        let accepted = pending.receipt.as_ref().ok_or(ProviderError::Persistence)?;
        cached(c, binding, &pending.key)?
            .filter(|cache| {
                cache.document.version == accepted.version && cache.document.body == pending.body
            })
            .and_then(|cache| cache.projection)
    } else {
        None
    };
    Ok(json!({"operation_id":id,"state":pending.state,"projection":projection}))
}

pub(super) fn begin_edit(c: &mut Connection, d: &SourceDescriptor, id: Uuid) -> Result<Value> {
    let b = d.binding();
    let key = key_for(c, b, id)?;
    if !d.capabilities().guarded_write {
        return Err(ProviderError::Unsupported);
    }
    let metadata = super::metadata_observation::metadata(c, b, &key)?;
    if metadata.version.conditional_write_token.is_none() {
        return Err(ProviderError::Unsupported);
    }
    let cache = cached(c, b, &key)?.ok_or(ProviderError::Unavailable)?;
    if cache.projection.is_none() {
        return Err(ProviderError::Unavailable);
    }
    let time = now()?;
    registry::db(c.execute(
        "DELETE FROM mdreader_provider_client_edits WHERE expires<?1",
        [time],
    ))?;
    let count: u64 = registry::db(c.query_row(
        "SELECT count(*) FROM mdreader_provider_client_edits",
        [],
        |r| r.get(0),
    ))?;
    if count >= 256 {
        return Err(ProviderError::TooLarge);
    }
    let edit = Uuid::new_v4();
    registry::db(c.execute(
        "INSERT INTO mdreader_provider_client_edits VALUES(?1,?2,?3,?4,?5,0)",
        params![
            edit.to_string(),
            b.digest().as_slice(),
            key.as_str(),
            registry::encode(&metadata.version, 32768)?,
            time + 10 * 60 * 1000
        ],
    ))?;
    Ok(
        json!({"edit_id":edit,"markdown":std::str::from_utf8(cache.document.body.as_bytes()).map_err(|_|ProviderError::InvalidUtf8)?,"expires_at_ms":time+10*60*1000}),
    )
}
pub(super) fn prepare_save(
    c: &mut Connection,
    d: &SourceDescriptor,
    id: Uuid,
    markdown: String,
) -> Result<Value> {
    let b = d.binding();
    revalidate(c, b)?;
    let row:Option<(String,String,i64,bool)>=registry::db(c.query_row("SELECT key,expected,expires,consumed FROM mdreader_provider_client_edits WHERE id=?1 AND binding=?2",params![id.to_string(),b.digest().as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional())?;
    let (key, expected, expires, consumed) = row.ok_or(ProviderError::Unauthorized)?;
    if expires < now()? {
        return Err(ProviderError::Unauthorized);
    }
    let key = RecordKey::new(key)?;
    let expected = registry::decode(&expected, 32768)?;
    let body = MarkdownBytes::new(markdown.into_bytes())?;
    // The edit UUID is also the durable operation UUID: a crash between journal
    // commit and ticket consumption cannot manufacture a second write intent.
    match journal::load(c, b, id) {
        Ok(pending) => {
            if pending.key != key || pending.expected != expected || pending.body != body {
                return Err(ProviderError::Conflict);
            }
        }
        Err(ProviderError::Unauthorized) => {
            if consumed {
                return Err(ProviderError::Unauthorized);
            }
            if !d.capabilities().guarded_write {
                return Err(ProviderError::Unsupported);
            }
            if super::metadata_observation::metadata(c, b, &key)?.version != expected {
                return Err(ProviderError::Conflict);
            }
            journal::prepare_with_id(c, b, &key, &expected, &body, id)?;
        }
        Err(error) => return Err(error),
    }
    registry::db(c.execute(
        "UPDATE mdreader_provider_client_edits SET consumed=1 WHERE id=?1",
        [id.to_string()],
    ))?;
    Ok(json!({"operation_id":id,"state":"prepared"}))
}

#[cfg(test)]
#[path="editing_tests.rs"]
mod tests;
