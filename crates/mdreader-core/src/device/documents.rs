use super::*;

pub(super) fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DeviceDocument> {
    Ok(DeviceDocument {
        id: r.get(0)?,
        name: r.get(1)?,
        source_id: r.get(2)?,
        revision: r.get(3)?,
        bytes: r.get(4)?,
        pinned: r.get(5)?,
        line: r.get(6)?,
        indexed: r.get(7)?,
    })
}
pub fn list(store: &mut ReaderStore) -> Result<Vec<DeviceDocument>> {
    schema(store)?;
    let mut documents = store.with_library_connection(|db| {
        db.prepare("SELECT d.id,d.name,d.source_id,d.revision,d.bytes,d.pinned,d.line,d.indexed FROM device_documents_v1 d JOIN sources s ON s.id=d.source_id WHERE s.enabled=1 AND d.retired=0 ORDER BY d.pinned DESC,d.rowid DESC LIMIT 1000")?
            .query_map([],row)?.collect::<rusqlite::Result<Vec<_>>>()
    }).map_err(Error::from)?;
    for document in &mut documents {
        document.indexed = store
            .index_status(std::slice::from_ref(&document.source_id))
            .is_ok_and(|status| status.documents == 1);
    }
    Ok(documents)
}
pub fn document(store: &mut ReaderStore, id: &str) -> Result<DeviceDocument> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err(Error::InvalidSelection);
    }
    schema(store)?;
    store.with_library_connection(|db| db.query_row("SELECT d.id,d.name,d.source_id,d.revision,d.bytes,d.pinned,d.line,d.indexed FROM device_documents_v1 d JOIN sources s ON s.id=d.source_id WHERE d.id=?1 AND s.enabled=1 AND d.retired=0",[id],row).optional())
        .map_err(Error::from)?.ok_or(Error::Unavailable)
}
pub fn read(store: &mut ReaderStore, id: &str, expected: Option<&str>) -> Result<DeviceRead> {
    let document = document(store, id)?;
    if expected.is_some_and(|value| value != document.revision) {
        return Err(Error::Conflict);
    }
    let body = store
        .read_document(&document.source_id, "document.md", Some(&document.revision))
        .map_err(Error::from)?;
    Ok(DeviceRead {
        document,
        markdown: body.markdown,
    })
}
pub fn search(store: &mut ReaderStore, query: &str) -> Result<Vec<DeviceHit>> {
    if query.len() > 1024 {
        return Err(Error::InvalidSelection);
    }
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let docs = list(store)?;
    if docs.is_empty() {
        return Ok(Vec::new());
    }
    let sources = docs
        .iter()
        .map(|doc| doc.source_id.clone())
        .collect::<Vec<_>>();
    let mut hits = Vec::new();
    // Keep each query below the core's 128-source scope bound.
    for selected in sources.chunks(64) {
        for evidence in store.search(query, selected, 50).map_err(Error::from)? {
            if let Some(document) = docs.iter().find(|doc| {
                doc.source_id == evidence.source_id && doc.revision == evidence.revision
            }) {
                hits.push((
                    evidence.score,
                    DeviceHit {
                        document: document.clone(),
                        line: evidence.line_start,
                        text: evidence.text,
                    },
                ));
            }
        }
    }
    hits.sort_by(|a, b| b.0.total_cmp(&a.0));
    hits.truncate(50);
    Ok(hits.into_iter().map(|(_, hit)| hit).collect())
}
pub fn position(store: &mut ReaderStore, id: &str, revision: &str, line: u32) -> Result<()> {
    let body = read(store, id, Some(revision))?;
    let max_line = body.markdown.lines().count().max(1) as u32;
    if line == 0 || line > max_line {
        return Err(Error::InvalidPosition);
    }
    super::state::local_update(store, id, Some(revision), None, Some(line))
}

pub fn pin(store: &mut ReaderStore, id: &str, pinned: bool) -> Result<()> {
    super::state::local_update(store, id, None, Some(pinned), None)
}
pub fn rebuild(store: &mut ReaderStore) -> Result<()> {
    for document in list(store)? {
        store.index_source(&document.source_id)?;
    }
    Ok(())
}
