use crate::*;

/// Metadata and exact visible Markdown are verified before acknowledgment/retrieval.
pub fn read_projection(path: &Path, limits: &Limits) -> Result<ProjectionMetadata> {
    parse_projection(path, limits).map(|(metadata, _)| metadata)
}
pub(crate) fn parse_projection(
    path: &Path,
    limits: &Limits,
) -> Result<(ProjectionMetadata, String)> {
    let bytes = read_bounded(path, limits.max_projection_bytes)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| CaptureError::Invalid)?;
    let rest = text.strip_prefix(HEADER).ok_or(CaptureError::Invalid)?;
    let (json, body) = rest.split_once("\n---\n\n").ok_or(CaptureError::Invalid)?;
    let metadata: ProjectionMetadata =
        serde_json::from_str(json).map_err(|_| CaptureError::Invalid)?;
    if metadata.schema_version != 1
        || !metadata.capture_is_not_approval
        || !valid_id(&metadata.content_id)
        || metadata.occurrences.is_empty()
        || metadata.occurrences.len() > limits.max_occurrences_per_body
        || metadata.content_revision != blake3::hash(body.as_bytes()).to_hex().as_str()
    {
        return Err(CaptureError::Invalid);
    }
    for occurrence in &metadata.occurrences {
        if !valid_id(&occurrence.occurrence_id)
            || metadata.content_id != hash(&(&occurrence.project_id, body))?
            || occurrence.occurrence_id != occurrence_identity(&metadata.content_id, occurrence)?
        {
            return Err(CaptureError::Invalid);
        }
    }
    Ok((metadata, body.into()))
}
