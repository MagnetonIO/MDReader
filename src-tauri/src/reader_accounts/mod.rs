//! Thin account RPC projection. Native dialogs/custody belong to the service adapters.
use crate::reader::ReaderBackend;
use mdreader_service::Request;
use serde::Deserialize;
use tauri::State;
mod model;
mod projection;
pub use model::*;

#[derive(Deserialize)]
struct Envelope {
    result: Option<mdreader_service::accounts::rpc::AccountReply>,
    error: Option<mdreader_service::accounts::AccountError>,
}
fn request(reader: &ReaderBackend, request: AccountRequest) -> Result<AccountReply, AccountError> {
    let response: Envelope = reader
        .typed(Request::Account {
            request: request.service()?,
        })
        .map_err(|_| model::error("unavailable"))?;
    match (response.result, response.error) {
        (Some(value), None) => Ok(value.into()),
        (None, Some(error)) => Err(error.into()),
        _ => Err(model::error("unavailable")),
    }
}
#[tauri::command]
#[specta::specta]
pub async fn reader_account(
    operation: AccountRequest,
    reader: State<'_, ReaderBackend>,
) -> Result<AccountReply, AccountError> {
    let backend = reader.inner().clone();
    tauri::async_runtime::spawn_blocking(move || request(&backend, operation))
        .await
        .map_err(|_| model::error("unavailable"))?
}

#[cfg(test)]
mod tests;
