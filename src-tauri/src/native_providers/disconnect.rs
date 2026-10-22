//! Renderer-visible outcome of removing a source and its native sign-in.
//! The service owns custody deletion and provider revocation.
use super::*;

/// What happened to the stored sign-in on this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SignInDisposalDto {
    Deleted,
    /// Another source still uses it, so it was kept and not revoked.
    Shared,
}
/// Best-effort provider revocation after the local removal committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum RevocationDto {
    Revoked,
    Failed,
    NotAttempted,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    disconnected: bool,
    credential: SignInDisposalDto,
    revocation: RevocationDto,
}

pub(super) async fn disconnect(rpc: &dyn Rpc, source_id: String) -> Result<NativeProviderResult> {
    let source_id = valid_source(&source_id)?;
    let reply: Reply = decode(
        rpc.provider(ProviderRequest::Disconnect { source_id })
            .await?,
    )?;
    // A shared sign-in is never revoked; any other combination is a protocol fault.
    if !reply.disconnected
        || reply.credential == SignInDisposalDto::Shared
            && reply.revocation != RevocationDto::NotAttempted
    {
        return Err(NativeProviderError::InvalidResponse);
    }
    Ok(NativeProviderResult::Disconnect {
        credential: reply.credential,
        revocation: reply.revocation,
    })
}
