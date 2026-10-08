use super::*;

/// Decode URL escapes exactly once; percent remnants are refused so a second
/// consumer cannot reinterpret a target. Qualified relative paths remain exact.
pub(super) fn canonical_target(raw: &str) -> Result<String> {
    if raw.is_empty() || raw.len() > 1536 {
        return Err(Error::InvalidTarget);
    }
    let input = raw.as_bytes();
    let mut bytes = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' {
            if i + 2 >= input.len() {
                return Err(unavailable());
            }
            let hex = std::str::from_utf8(&input[i + 1..i + 3]).map_err(|_| unavailable())?;
            bytes.push(u8::from_str_radix(hex, 16).map_err(|_| unavailable())?);
            i += 3;
        } else {
            bytes.push(input[i]);
            i += 1;
        }
    }
    let value = String::from_utf8(bytes).map_err(|_| unavailable())?;
    if value.len() > 512
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, ':' | '\\' | '?' | '#' | '%'))
        || value.starts_with('/')
    {
        return Err(Error::InvalidTarget);
    }
    let parts: Vec<_> = value.split('/').filter(|part| *part != ".").collect();
    if parts.is_empty()
        || parts.len() > 8
        || parts.iter().any(|part| part.is_empty() || *part == "..")
    {
        return Err(Error::InvalidTarget);
    }
    Ok(parts.join("/"))
}
pub(super) fn document(store: &mut ReaderStore, id: &str, revision: &str) -> Result<String> {
    valid_id(id)?;
    if revision.len() != 64
        || !revision
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(unavailable());
    }
    let metadata = crate::device::catalog::document(store, id).map_err(|error| match error {
        crate::device::Error::UnsupportedSchema => Error::UnsupportedDocumentSchema,
        _ => unavailable(),
    })?;
    crate::device::catalog::read(
        store,
        &crate::device::catalog::Receipt {
            document_id: id.into(),
            source_id: metadata.source_id,
            revision: revision.into(),
        },
    )
    .map(|body| body.markdown)
    .map_err(|error| match error {
        crate::device::Error::UnsupportedSchema => Error::UnsupportedDocumentSchema,
        _ => unavailable(),
    })
}
pub(super) fn image_targets(markdown: &str) -> Vec<String> {
    let parsed = crate::parse_markdown(markdown, "document.md");
    let mut targets: Vec<_> = parsed
        .sections
        .iter()
        .flat_map(|section| &section.links)
        .filter(|link| link.kind == "image")
        .filter_map(|link| canonical_target(&link.raw_target).ok())
        .collect();
    targets.sort();
    targets.dedup();
    targets.truncate(256);
    targets
}
