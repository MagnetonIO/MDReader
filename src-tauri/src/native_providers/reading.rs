//! Explicit current *already admitted* provider head. This reads owned local
//! bytes only; exact-history reads retain their strict revision semantics.
use super::*;

pub(super) async fn current(
    rpc: &dyn Rpc,
    source_id: String,
    path: String,
    prior_revision: String,
) -> Result<NativeProviderResult> {
    let selected = ProjectionDto {
        job_id: String::new(),
        reader_source_id: source_id.clone(),
        path: path.clone(),
        revision: prior_revision,
    };
    validate_projection(&selected)?;
    // Owned core reading checks the current enabled binding, retained private
    // root, bounded bytes/hash and unchanged final head. No provider request or
    // credential resolution occurs; the prior receipt is not new authority.
    let document = rpc.read_current(&selected).await?;
    if document.source_id != source_id
        || document.path != path
        || document.markdown.len() > MAX_MARKDOWN
    {
        return Err(NativeProviderError::InvalidResponse);
    }
    validate_projection(&ProjectionDto {
        job_id: String::new(),
        reader_source_id: document.source_id.clone(),
        path: document.path.clone(),
        revision: document.revision.clone(),
    })
    .map_err(|_| NativeProviderError::InvalidResponse)?;
    let line_end = document
        .markdown
        .lines()
        .count()
        .max(1)
        .try_into()
        .map_err(|_| NativeProviderError::TooLarge)?;
    Ok(NativeProviderResult::ReadCurrentDocument {
        source_id: document.source_id,
        path: document.path,
        revision: document.revision,
        markdown: document.markdown,
        line_start: 1,
        line_end,
    })
}

#[cfg(test)]
#[path = "reading_tests.rs"]
mod tests;
