use super::*;
use crate::{provider::ProviderRequest, Request};
use mdreader_provider::{OpaqueValue, RecordKey, SourceId};
fn result(value: Value) -> Result<Value> {
    if !value["error"].is_null() {
        return Err(ReaderError::Unavailable);
    }
    value.get("result").cloned().ok_or(ReaderError::Unavailable)
}
fn save(
    runtime: &Runtime,
    fence: &WriteFence,
    id: &str,
    remote: &Value,
    value: Value,
) -> Result<Value> {
    let state = value["state"].as_str().unwrap_or("unknown");
    let saved = state == "published" && !value["projection"].is_null();
    let reply = json!({"operation_id":id,"state":if saved{"saved"}else{"pending"},"document":value["projection"]});
    let mut store = runtime.store()?;
    check(&mut store, fence)?;
    ledger::update(
        &mut store,
        &fence.grant_id,
        id,
        if saved { "saved" } else { "pending" },
        Some(remote),
        if saved { Some(&reply) } else { None },
    )?;
    Ok(reply)
}
pub(super) fn inspect(
    runtime: &Runtime,
    fence: &WriteFence,
    id: &str,
    operation: &ledger::Operation,
) -> Result<Value> {
    let Some(remote) = operation.remote.as_ref() else {
        return Ok(json!({"operation_id":id,"state":operation.state}));
    };
    let source = SourceId::new(
        remote["source_id"]
            .as_str()
            .ok_or(ReaderError::Unavailable)?
            .to_owned(),
    )
    .map_err(|_| invalid())?;
    let operation_id = uuid::Uuid::parse_str(
        remote["operation_id"]
            .as_str()
            .ok_or(ReaderError::Unavailable)?,
    )
    .map_err(|_| invalid())?;
    let request = if remote["kind"] == "create" {
        ProviderRequest::InspectCreate {
            source_id: source,
            operation_id,
        }
    } else {
        ProviderRequest::InspectSave {
            source_id: source,
            operation_id,
        }
    };
    let value = result(runtime.execute(Request::Provider { request })?)?;
    save(runtime, fence, id, remote, value)
}
pub(super) fn write(
    runtime: &Runtime,
    fence: &WriteFence,
    id: &str,
    operation: &ledger::Operation,
) -> Result<Value> {
    let (source, path, _, expected, markdown) = fields(&operation.request)?;
    if operation.state == "pending" || operation.state == "dispatched" {
        return inspect(runtime, fence, id, operation);
    }
    let remote = if let Some(remote) = &operation.remote {
        remote.clone()
    } else {
        let remote = if let Some(revision) = expected {
            let edit = result(runtime.execute(Request::Provider {
                request: ProviderRequest::BeginEditExact {
                    reader_source_id: SourceId::new(source.to_owned()).map_err(|_| invalid())?,
                    path: RecordKey::new(path.to_owned()).map_err(|_| invalid())?,
                    revision: OpaqueValue::new(revision.to_owned()).map_err(|_| invalid())?,
                },
            })?)?;
            json!({"source_id":edit["write_source_id"],"operation_id":edit["edit_id"],"kind":"update"})
        } else {
            // Provider registry IDs stay opaque; use only an exact owner mapping.
            let source_id = runtime
                .store()?
                .with_library_connection(|db| crate::provider::agent_source_id(db, source))
                .map_err(|_| ReaderError::Unavailable)?;
            json!({"source_id":source_id,"operation_id":id,"kind":"create"})
        };
        let mut store = runtime.store()?;
        check(&mut store, fence)?;
        ledger::update(
            &mut store,
            &fence.grant_id,
            id,
            "prepared",
            Some(&remote),
            None,
        )?;
        remote
    };
    let source_id = SourceId::new(
        remote["source_id"]
            .as_str()
            .ok_or(ReaderError::Unavailable)?
            .to_owned(),
    )
    .map_err(|_| invalid())?;
    let operation_id = uuid::Uuid::parse_str(
        remote["operation_id"]
            .as_str()
            .ok_or(ReaderError::Unavailable)?,
    )
    .map_err(|_| invalid())?;
    if expected.is_some() {
        runtime.agent_provider(
            ProviderRequest::PrepareSave {
                source_id: source_id.clone(),
                edit_id: operation_id,
                markdown: markdown.into(),
            },
            fence.clone(),
        )?;
    }
    {
        let mut store = runtime.store()?;
        check(&mut store, fence)?;
        ledger::update(
            &mut store,
            &fence.grant_id,
            id,
            "dispatched",
            Some(&remote),
            None,
        )?;
    }
    let request = if expected.is_some() {
        ProviderRequest::DispatchSave {
            source_id,
            operation_id,
        }
    } else {
        if path.contains('/') {
            return Err(invalid());
        }
        ProviderRequest::CreateDocument {
            source_id,
            operation_id,
            filename: path.into(),
            markdown: markdown.into(),
        }
    };
    match runtime.agent_provider(request, fence.clone()) {
        Ok(value) => save(runtime, fence, id, &remote, value),
        Err(ReaderError::Conflict) => {
            ledger::update(
                &mut *runtime.store()?,
                &fence.grant_id,
                id,
                "conflict",
                None,
                None,
            )?;
            Err(ReaderError::Conflict)
        }
        Err(_) => {
            ledger::update(
                &mut *runtime.store()?,
                &fence.grant_id,
                id,
                "pending",
                None,
                None,
            )?;
            Ok(json!({"operation_id":id,"state":"pending"}))
        }
    }
}
