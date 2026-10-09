//! Known visible capture output, not agent histories or a reactivated policy.
use super::*;
pub(super) fn capture(
    owner: &mut ReaderStore,
    profile: &SourceRoot,
    documents: &mut Vec<Document>,
) -> Result<()> {
    type Row = (String, String, String, String);
    let rows:Vec<Row>=owner.with_library_connection(|db| {
        let mut query=db.prepare("SELECT CASE WHEN length(id)=64 THEN id END,CASE WHEN length(source_id)<=256 THEN source_id END,CASE WHEN length(path)<=4096 THEN path END,CASE WHEN length(revision)=64 THEN revision END FROM captures ORDER BY id LIMIT 1001")?;
        let rows=query.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok::<_,rusqlite::Error>(rows)
    }).map_err(|_|Error::Integrity)?;
    if rows.len() > 1000 {
        return Err(Error::Bounds);
    }
    let mut admitted = std::collections::HashSet::new();
    for (id, source, path, revision) in rows {
        let root = owned::source_root(
            owner,
            &source,
            &profile.path.join("reader-v1/captures/markdown"),
        )?;
        let bytes = owned::read(&root, Path::new(&path), 1024 * 1024)?;
        if blake3::hash(&bytes).to_hex().as_str() != revision {
            return Err(Error::Conflict);
        }
        let (metadata, _) = mdreader_capture::parse_projection_bytes(&bytes, &Default::default())
            .map_err(|_| Error::Integrity)?;
        if path != format!("{}.md", metadata.content_id)
            || !metadata.occurrences.iter().any(|p| p.occurrence_id == id)
        {
            return Err(Error::Conflict);
        }
        if !admitted.insert(path.clone()) {
            continue;
        }
        owned::push(
            documents,
            "Agent output",
            String::from_utf8(bytes).map_err(|_| Error::Integrity)?,
            Lineage {
                kind: "capture".into(),
                source_id: source,
                path: path.clone(),
                revision,
                document_id: None,
                operation_id: None,
                state: Some("admitted".into()),
            },
            1,
        )?;
    }
    // Already projected but not admitted output remains an explicit recovered
    // copy; no spool open, replay, new source or project-root traversal.
    for (relative, pending) in [
        ("reader-v1/captures/markdown", false),
        ("reader-v1/captures/pending", true),
    ] {
        if !profile
            .path
            .join(relative)
            .try_exists()
            .map_err(|_| Error::Storage)?
        {
            continue;
        }
        let mut entries = profile
            .read_dir_relative_bounded(Path::new(relative), 1000)
            .map_err(|_| Error::Conflict)?;
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        for entry in entries {
            let name = entry.name.to_str().ok_or(Error::Unsupported)?;
            if !pending && admitted.contains(name) {
                continue;
            }
            if entry.kind != mdreader_core::source_root::SourceEntryKind::File {
                return Err(Error::Unsupported);
            }
            let bytes = owned::read(profile, &Path::new(relative).join(&entry.name), 1024 * 1024)?;
            let (body, id) = if pending {
                let record: mdreader_capture::CaptureRecord =
                    serde_json::from_slice(&bytes).map_err(|_| Error::Integrity)?;
                mdreader_capture::validate_record(&record).map_err(|_| Error::Integrity)?;
                if name != format!("{}.json", record.id) {
                    return Err(Error::Conflict);
                }
                (record.body, record.id)
            } else {
                let (metadata, _) =
                    mdreader_capture::parse_projection_bytes(&bytes, &Default::default())
                        .map_err(|_| Error::Integrity)?;
                if name != format!("{}.md", metadata.content_id) {
                    return Err(Error::Conflict);
                }
                (
                    String::from_utf8(bytes).map_err(|_| Error::Integrity)?,
                    metadata.content_id,
                )
            };
            let revision = blake3::hash(body.as_bytes()).to_hex().to_string();
            owned::push(
                documents,
                if pending {
                    "Recovered pending agent output"
                } else {
                    "Recovered agent output"
                },
                body,
                Lineage {
                    kind: "recovered_capture".into(),
                    source_id: String::new(),
                    path: format!("{relative}/{name}"),
                    revision,
                    document_id: None,
                    operation_id: Some(id),
                    state: Some(if pending { "pending" } else { "projected" }.into()),
                },
                1,
            )?;
        }
    }
    Ok(())
}
