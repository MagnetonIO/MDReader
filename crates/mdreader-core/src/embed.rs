//! The existing BGE model contract, owned by MDReader.
use crate::{ReaderError, Result, EMBEDDING_DIM};
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use std::path::Path;

pub struct BgeEmbedder(TextEmbedding);
impl BgeEmbedder {
    /// Blocking initialization/inference belongs on the service model worker.
    pub fn open(cache: &Path) -> Result<Self> {
        let options = TextInitOptions::new(EmbeddingModel::BGESmallENV15)
            .with_cache_dir(cache.to_path_buf())
            .with_show_download_progress(false);
        Ok(Self(TextEmbedding::try_new(options).map_err(|e| {
            ReaderError::Invalid(format!("model unavailable: {e}"))
        })?))
    }
    /// Preserve fastembed BGE tokenizer, pooling and normalization. The current
    /// reader passes query text directly; no new query/passage prefix is added.
    pub fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let vectors = self
            .0
            .embed(texts, Some(64))
            .map_err(|e| ReaderError::Invalid(format!("embedding failed: {e}")))?;
        if vectors.len() != texts.len()
            || vectors
                .iter()
                .any(|v| v.len() != EMBEDDING_DIM || v.iter().any(|n| !n.is_finite()))
        {
            return Err(ReaderError::Invalid("embedding contract mismatch".into()));
        }
        Ok(vectors)
    }
}
