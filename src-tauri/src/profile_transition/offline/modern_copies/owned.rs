//! Exact existing profile-owned bytes. No source grants or external paths.
use super::*;
use mdreader_core::source_root::file_stamp::single_link_stamp;
use std::os::unix::ffi::OsStringExt;

pub(super) fn source_root(
    owner: &mut ReaderStore,
    source: &str,
    expected: &Path,
) -> Result<SourceRoot> {
    let (path, identity): (Vec<u8>, String) = owner.with_library_connection(|db| db.query_row(
        "SELECT CASE WHEN length(path)<=16384 THEN path END,CASE WHEN length(identity)<=16384 THEN identity END FROM sources WHERE id=?1", [source], |r| Ok((r.get(0)?,r.get(1)?))))
        .map_err(|_|Error::Conflict)?;
    let path = PathBuf::from(std::ffi::OsString::from_vec(path));
    if path != expected {
        return Err(Error::Conflict);
    }
    SourceRoot::restore(
        path,
        serde_json::from_str(&identity).map_err(|_| Error::Integrity)?,
    )
    .map_err(|_| Error::Conflict)
}
pub(super) fn read(root: &SourceRoot, path: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut file = root
        .open_optional_file_relative(path)
        .map_err(|_| Error::Conflict)?
        .ok_or(Error::Conflict)?;
    let before = single_link_stamp(&file).map_err(|_| Error::Conflict)?;
    if file.metadata().map_err(|_| Error::Storage)?.len() > limit as u64 {
        return Err(Error::Bounds);
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Storage)?;
    if bytes.len() > limit {
        return Err(Error::Bounds);
    }
    let named = root
        .open_optional_file_relative(path)
        .map_err(|_| Error::Conflict)?
        .ok_or(Error::Conflict)?;
    if before != single_link_stamp(&file).map_err(|_| Error::Conflict)?
        || before != single_link_stamp(&named).map_err(|_| Error::Conflict)?
    {
        return Err(Error::Conflict);
    }
    root.verify().map_err(|_| Error::Conflict)?;
    Ok(bytes)
}
pub(super) fn push(
    documents: &mut Vec<Document>,
    name: &str,
    markdown: String,
    lineage: Lineage,
    line: u32,
) -> Result<()> {
    let current = documents
        .iter()
        .try_fold(0usize, |n, d| {
            n.checked_add(d.markdown.len()).and_then(|n| {
                d.assets
                    .iter()
                    .try_fold(n, |n, a| n.checked_add(a.base64.len()))
            })
        })
        .ok_or(Error::Bounds)?;
    if markdown.len() > 1024 * 1024
        || documents.len() >= 1000
        || current
            .checked_add(markdown.len())
            .is_none_or(|n| n > MAX_BYTES)
    {
        return Err(Error::Bounds);
    }
    let id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        &serde_json::to_vec(&lineage).map_err(|_| Error::Integrity)?,
    )
    .to_string();
    if documents.iter().any(|d| d.id == id) {
        return Err(Error::Conflict);
    }
    let name = self::name(name);
    let line_end = markdown.lines().count().max(1) as u32;
    if line == 0 || line > line_end {
        return Err(Error::Integrity);
    }
    documents.push(Document {
        id,
        source_id: lineage.source_id.clone(),
        name,
        revision: blake3::hash(markdown.as_bytes()).to_hex().to_string(),
        markdown,
        pinned: false,
        line,
        assets: vec![],
        lineage: Some(lineage),
    });
    Ok(())
}
pub(super) fn table(owner: &mut ReaderStore, name: &str) -> Result<bool> {
    owner
        .with_library_connection(|db| {
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
                [name],
                |r| r.get(0),
            )
        })
        .map_err(|_| Error::Integrity)
}

pub(super) fn name(name: &str) -> String {
    let mut name: String = name
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if matches!(c, '/' | '\\') { '_' } else { c })
        .collect();
    while name.len() > 215 {
        name.pop();
    }
    if name.is_empty() {
        name = "Recovered document".into();
    }
    if !name.to_ascii_lowercase().ends_with(".md") {
        name.push_str(".md");
    }
    name
}
