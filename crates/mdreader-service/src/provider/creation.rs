//! Durable create-only workflow. Never resend an uncertain creation.
use super::*;
use mdreader_provider::creation as journal;
use serde_json::{Value, json};

fn now_ms() -> Result<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ProviderError::Protocol)?
        .as_millis()
        .try_into()
        .map_err(|_| ProviderError::Protocol)
}
fn allowed(descriptor: &SourceDescriptor) -> Result<()> {
    if !descriptor.capabilities().guarded_write {
        return Err(ProviderError::Unauthorized);
    }
    if !matches!(
        descriptor.scope(),
        SelectedScope::DriveFolder { .. } | SelectedScope::ScopedRoot { .. }
    ) {
        return Err(ProviderError::Unsupported);
    }
    if descriptor.health() != SourceHealth::Ready {
        return Err(ProviderError::ReconnectRequired);
    }
    Ok(())
}
pub(super) fn inspect(
    c: &mut Connection,
    binding: &SourceBinding,
    id: Uuid,
    projection: &dyn ProjectionAdmission,
) -> Result<Value> {
    revalidate(c, binding)?;
    journal::initialize(c)?;
    let Some(mut pending) = journal::load(c, binding, id)? else {
        return Ok(json!({"operation_id":id,"state":"absent","projection":null}));
    };
    if pending.state == mdreader_provider::journal::MutationState::Accepted {
        // Local publication can be retried; this path performs no remote write.
        let _ = registry::publish_created(c, binding, id, projection, now_ms()?);
        pending = journal::load(c, binding, id)?.ok_or(ProviderError::Persistence)?;
    }
    let projection = pending
        .projection
        .map(|text| registry::decode::<ProjectionReceipt>(&text, 16384))
        .transpose()?;
    Ok(json!({"operation_id":id,"state":pending.state,"projection":projection}))
}
pub(super) async fn create<O: Owner, P: DocumentProvider + ?Sized>(
    owner: &O,
    provider: &P,
    id: Uuid,
    filename: String,
    markdown: String,
    projection: &dyn ProjectionAdmission,
    context: &IoContext,
) -> Result<Value> {
    let descriptor = provider.descriptor();
    allowed(descriptor)?;
    let binding = descriptor.binding();
    let body = MarkdownBytes::new(markdown.into_bytes())?;
    let prior = owner.with_connection(|c| {
        revalidate(c, binding)?;
        allowed(&registry::descriptor_at(c, binding, true)?)?;
        journal::prepare(c, binding, id, &filename, &body)?;
        journal::load(c, binding, id)?.ok_or(ProviderError::Persistence)
    })?;
    if prior.state != mdreader_provider::journal::MutationState::Prepared {
        return owner.with_connection(|c| inspect(c, binding, id, projection));
    }
    context.check()?;
    if owner.with_connection(|c| Ok(metadata_observation::snapshot(c, binding)?.id.is_none()))? {
        let checkpoint = if descriptor.capabilities().changes {
            Some(provider.start_changes(context).await?)
        } else {
            None
        };
        context.check()?;
        owner.with_connection(|c| {
            revalidate(c, binding)?;
            if metadata_observation::snapshot(c, binding)?.id.is_none() {
                begin_snapshot(c, binding, checkpoint.as_ref())?;
            }
            Ok(())
        })?;
    }
    let permit = owner.with_connection(|c| {
        revalidate(c, binding)?;
        allowed(&registry::descriptor_at(c, binding, true)?)?;
        journal::dispatch(c, binding, id)
    })?;
    match provider.guarded_create(permit, &body, context).await {
        Ok(metadata) => {
            let verified = metadata
                .version
                .content_hash
                .as_ref()
                .map(|hash| verify_content_hash(body.as_bytes(), hash))
                .transpose();
            if verified.is_err()
                || owner
                    .with_connection(|c| journal::accept(c, binding, id, &metadata))
                    .is_err()
            {
                owner.with_connection(|c| journal::uncertain(c, binding, id))?;
            }
        }
        Err(
            ProviderError::Conflict
            | ProviderError::Unauthorized
            | ProviderError::Unsupported
            | ProviderError::Invalid,
        ) => {
            owner.with_connection(|c| journal::reject(c, binding, id))?;
        }
        Err(_) => {
            owner.with_connection(|c| journal::uncertain(c, binding, id))?;
        }
    }
    owner.with_connection(|c| inspect(c, binding, id, projection))
}
