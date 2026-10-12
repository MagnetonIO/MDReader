//! Explicit document writes; every replay is scoped to the issuing connection.
use super::{
    invalid, operation_store as ledger,
    projects::{self, AgentRequest},
    WriteFence,
};
use crate::runtime::Runtime;
use mdreader_core::{ReaderError, ReaderStore, Result};
use serde_json::{json, Value};
#[cfg(feature = "sync-native")]
#[path = "document_device.rs"]
mod device;
#[path = "document_provider.rs"]
mod provider;
pub(super) fn editable(store: &mut ReaderStore, source: &str) -> Result<bool> {
    if store.chosen_local_root(source).is_ok() {
        return Ok(true);
    }
    if source.starts_with("provider:") {
        return Ok(store
            .with_library_connection(|db| crate::provider::agent_source_id(db, source))
            .is_ok());
    }
    #[cfg(feature = "sync-native")]
    {
        let present:bool=store.with_library_connection(|db|db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='device_documents_v1' AND type='table')",[],|r|r.get(0)))?;
        if present {
            return Ok(mdreader_core::device::list(store)
                .map_err(|_| ReaderError::Unavailable)?
                .iter()
                .any(|d| d.source_id == source));
        }
    }
    Ok(false)
}
pub(super) fn fence(store: &mut ReaderStore, grant: &str, source: &str) -> Result<WriteFence> {
    let (generation, _, permissions) = projects::authorized(store, grant)?;
    if !editable(store, source)?
        || !permissions.edits.iter().any(|id| id == source)
        || !store
            .list_sources()?
            .iter()
            .any(|s| s.enabled && s.id == source)
    {
        return Err(ReaderError::Unavailable);
    }
    Ok(WriteFence {
        grant_id: grant.into(),
        source_id: source.into(),
        generation,
    })
}
pub(super) fn check(store: &mut ReaderStore, fence: &WriteFence) -> Result<()> {
    store
        .with_library_connection(|db| fence.check(db))
        .map_err(|_| ReaderError::Cancelled)
}
pub(super) fn fields(request: &AgentRequest) -> Result<(&str, &str, &str, Option<&str>, &str)> {
    match request {
        AgentRequest::CreateDocument {
            source_id,
            path,
            operation_id,
            markdown,
        } => Ok((source_id, path, operation_id, None, markdown)),
        AgentRequest::UpdateDocument {
            source_id,
            path,
            operation_id,
            expected_revision,
            markdown,
        } => Ok((
            source_id,
            path,
            operation_id,
            Some(expected_revision),
            markdown,
        )),
        _ => Err(invalid()),
    }
}
pub(super) fn execute(runtime: &Runtime, grant: &str, request: AgentRequest) -> Result<Value> {
    if let AgentRequest::InspectOperation { operation_id } = &request {
        validate_id(operation_id)?;
        let prior = ledger::get(&mut *runtime.store()?, grant, operation_id)?
            .ok_or(ReaderError::Unavailable)?;
        #[cfg(feature = "sync-native")]
        if let Some(result) = &prior.result {
            if !result["document"]["receipt"].is_null() {
                device::terminal(&mut *runtime.store()?, grant, result)?;
                return Ok(result.clone());
            }
        }
        let (source, path, _, _, markdown) = fields(&prior.request)?;
        let grant_fence = fence(&mut *runtime.store()?, grant, source)?;
        if let Some(result) = prior.result {
            return Ok(result);
        }
        if source.starts_with("provider:") {
            return provider::inspect(runtime, &grant_fence, operation_id, &prior);
        }
        if let Ok(root) = runtime.store()?.chosen_local_root(source) {
            if prior.state == "dispatched"
                && root
                    .read_relative(std::path::Path::new(path))
                    .is_ok_and(|r| r.bytes == markdown.as_bytes())
            {
                let revision = mdreader_core::local_write::body_revision(markdown);
                let result = json!({"operation_id":operation_id,"state":"saved","source_id":source,"path":path,"revision":revision});
                let mut store = runtime.store()?;
                check(&mut store, &grant_fence)?;
                ledger::update(
                    &mut store,
                    grant,
                    operation_id,
                    "saved",
                    None,
                    Some(&result),
                )?;
                return Ok(result);
            }
        }
        return Ok(json!({"operation_id":operation_id,"state":prior.state}));
    }
    let (source, path, id, expected, markdown) = fields(&request)?;
    validate_id(id)?;
    if markdown.len() > mdreader_core::local_write::MAX_BYTES
        || !mdreader_core::agent_sessions::markdown_path(path)
        || expected.is_some_and(|r| r.len() != 64 || !r.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(invalid());
    }
    let (grant_fence, prior, root) = {
        let mut store = runtime.store()?;
        projects::authorized(&mut store, grant)?;
        #[cfg(feature = "sync-native")]
        if let Some(prior) = ledger::get(&mut store, grant, id)? {
            if let Some(result) = &prior.result {
                if !result["document"]["receipt"].is_null() {
                    if serde_json::to_string(&prior.request).map_err(|_| invalid())?
                        != serde_json::to_string(&request).map_err(|_| invalid())?
                    {
                        return Err(ReaderError::Conflict);
                    }
                    device::terminal(&mut store, grant, result)?;
                    return Ok(result.clone());
                }
            }
        }
        let grant_fence = fence(&mut store, grant, source)?;
        let prior = ledger::prepare(&mut store, grant, id, &request)?;
        let root = store.chosen_local_root(source).ok();
        (grant_fence, prior, root)
    };
    if let Some(result) = prior.result {
        return Ok(result);
    }
    if source.starts_with("provider:") {
        return provider::write(runtime, &grant_fence, id, &prior);
    }
    if let Some(root) = root {
        if prior.state != "prepared" {
            return execute(
                runtime,
                grant,
                AgentRequest::InspectOperation {
                    operation_id: id.into(),
                },
            );
        }
        let staged =
            mdreader_core::local_write::PreparedWrite::stage(root, path, expected, markdown)?;
        let mut store = runtime.store()?;
        check(&mut store, &grant_fence)?;
        ledger::update(&mut store, grant, id, "dispatched", None, None)?;
        let revision = staged.commit(|| check(&mut store, &grant_fence))?;
        check(&mut store, &grant_fence)?;
        let result = json!({"operation_id":id,"state":"saved","source_id":source,"path":path,"revision":revision});
        ledger::update(&mut store, grant, id, "saved", None, Some(&result))?;
        store.queue_folder_changes(source, &[path.into()])?;
        return Ok(result);
    }
    #[cfg(feature = "sync-native")]
    {
        device::write(runtime, &grant_fence, id, &prior)
    }
    #[cfg(not(feature = "sync-native"))]
    {
        Err(ReaderError::Unavailable)
    }
}
fn validate_id(id: &str) -> Result<()> {
    if uuid::Uuid::parse_str(id).map_or(true, |id| id.get_version_num() != 4) {
        Err(invalid())
    } else {
        Ok(())
    }
}
