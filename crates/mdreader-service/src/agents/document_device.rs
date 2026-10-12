use super::*;
use base64::Engine;
pub(super) fn write(
    runtime: &Runtime,
    fence: &WriteFence,
    id: &str,
    operation: &ledger::Operation,
) -> Result<Value> {
    let (source, path, _, expected, markdown) = fields(&operation.request)?;
    if path.contains('/') {
        return Err(invalid());
    }
    let document = {
        let mut store = runtime.store()?;
        check(&mut store, fence)?;
        mdreader_core::device::list(&mut store)
            .map_err(|_| ReaderError::Unavailable)?
            .into_iter()
            .find(|d| d.source_id == source)
            .ok_or(ReaderError::Unavailable)?
    };
    if expected.is_some_and(|r| r != document.revision) {
        return Err(ReaderError::Conflict);
    }
    let saved = if let Some(revision) = expected {
        runtime.agent_device_save(
            mdreader_core::device::catalog::Receipt {
                document_id: document.id,
                source_id: source.into(),
                revision: revision.into(),
            },
            id.into(),
            base64::engine::general_purpose::STANDARD.encode(markdown),
            fence.clone(),
        )?
    } else {
        runtime.agent_device_create(
            path.into(),
            uuid::Uuid::parse_str(id).map_err(|_| invalid())?,
            markdown.into(),
            fence.clone(),
        )?
    };
    let result = json!({"operation_id":id,"state":"saved","document":saved});
    let mut store = runtime.store()?;
    check(&mut store, fence)?;
    ledger::update(
        &mut store,
        &fence.grant_id,
        id,
        "saved",
        None,
        Some(&result),
    )?;
    Ok(result)
}
