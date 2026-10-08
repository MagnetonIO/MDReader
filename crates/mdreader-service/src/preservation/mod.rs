//! Retained native service freeze control. No raw owner handles, database backup,
//! renderer API, all-participant readiness or selector permission is provided.
mod client;
#[cfg(feature = "sync-native")]
mod guards;
#[cfg(feature = "sync-native")]
mod session;
mod types;
pub use client::{Attempt, Client};
#[cfg(feature = "sync-native")]
pub(crate) use session::Session;
pub(crate) use types::MAX_CONTROL_BYTES;
pub use types::{
    BackupStatus, ControlReply, ControlRequest, Error, Phase, Result, Scope, SelectedRoot, Status,
};

pub(crate) fn dispatch(runtime: &crate::runtime::Runtime, request: ControlRequest) -> ControlReply {
    #[cfg(feature = "sync-native")]
    let result = runtime
        .preservation_session()
        .and_then(|session| session.execute(request));
    #[cfg(not(feature = "sync-native"))]
    let result: Result<ControlReply> = {
        let _ = (runtime, request);
        Err(Error::Unsupported)
    };
    result.unwrap_or_else(|error| ControlReply::Failure { error })
}
