//! Bounded read-only post-effect confirmation for Drive representations.
//! The effect response must already pass its acceptance guards. No write is retried,
//! and a retry never weakens exact revision/hash/size or current membership.
use super::*;

pub(super) struct EffectObservation<'a> {
    pub key: &'a RecordKey,
    pub version: &'a DocumentVersion,
    pub body: &'a MarkdownBytes,
    pub filename: Option<&'a str>,
    pub allow_checksum_refinement: bool,
}
impl Client {
    pub(super) async fn confirm_effect(
        &self,
        effect: EffectObservation<'_>,
        operation: uuid::Uuid,
        context: &IoContext,
        spent: &mut usize,
    ) -> Result<(File, DocumentVersion)> {
        for attempt in 0..3 {
            context.check()?;
            let observed = self.fetch_metadata(effect.key, context, spent).await;
            let (phase, error) = match observed {
                Ok((file, version)) => {
                    let revision = version.provider_revision == effect.version.provider_revision;
                    let hash_equal = version.content_hash == effect.version.content_hash;
                    let hash = (effect.allow_checksum_refinement
                        && effect.version.content_hash.is_none())
                        || hash_equal;
                    let size = file.size.as_deref().and_then(|v| v.parse::<u64>().ok())
                        == Some(effect.body.as_bytes().len() as u64);
                    let name = effect.filename.is_none_or(|name| file.name == name);
                    let exact = revision && hash && size && name;
                    if !exact {
                        write_diagnostic::comparison(operation, revision, hash_equal, size, name);
                    }
                    if exact {
                        if let Some(hash) = &version.content_hash {
                            verify_content_hash(effect.body.as_bytes(), hash)?;
                        }
                        context.check()?;
                        return Ok((file, version));
                    }
                    (
                        write_diagnostic::WritePhase::ConfirmationMismatch,
                        ProviderError::Conflict,
                    )
                }
                Err(ProviderError::Conflict) => (
                    write_diagnostic::WritePhase::ConfirmationRead,
                    ProviderError::Conflict,
                ),
                Err(error) => {
                    write_diagnostic::failure(
                        operation,
                        write_diagnostic::WritePhase::ConfirmationRead,
                        None,
                    );
                    return Err(error);
                }
            };
            if attempt == 2 {
                write_diagnostic::failure(operation, phase, None);
                return Err(error);
            }
            // Give v3/v2 propagation bounded time, while honoring the original
            // operation deadline/cancellation and retaining its transfer budget.
            let pause = tokio::time::sleep(std::time::Duration::from_millis(100 * (attempt + 1)));
            tokio::select! { biased;
                _ = context.cancellation().cancelled() => return Err(ProviderError::Cancelled),
                result = tokio::time::timeout_at(tokio::time::Instant::from_std(context.deadline()),pause) => result.map_err(|_|ProviderError::Timeout)?,
            }
            context.check()?;
        }
        Err(ProviderError::Conflict)
    }
}
