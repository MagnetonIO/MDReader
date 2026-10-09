//! Phase-only diagnostics. Never print request URLs, responses, identities,
//! callback parameters, token values or provider error descriptions.
use super::*;
pub(super) fn failure(
    provider: ProviderKind,
    phase: &'static str,
    error: ProviderError,
) -> ProviderError {
    let kind = match &error {
        ProviderError::Protocol => "protocol",
        ProviderError::Unauthorized => "unauthorized",
        ProviderError::IdentityMismatch => "identity_mismatch",
        ProviderError::ReconnectRequired => "reconnect_required",
        ProviderError::Timeout => "timeout",
        ProviderError::Cancelled => "cancelled",
        ProviderError::TooLarge => "limit",
        _ => "unavailable",
    };
    eprintln!("MDReader provider OAuth: provider={provider:?} phase={phase} kind={kind}");
    error
}
pub(super) fn rejected(status: u16, bytes: &[u8]) {
    // Only fixed public protocol codes are permitted. Unknown provider text is
    // reduced to 'other'; do not log even an unrecognized error field.
    let value = serde_json::from_slice::<serde_json::Value>(bytes).ok();
    let code = match value
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(|v| v.as_str())
    {
        Some("invalid_client") => "invalid_client",
        Some("invalid_grant") => "invalid_grant",
        Some("unauthorized_client") => "unauthorized_client",
        Some("invalid_request") => "invalid_request",
        Some("access_denied") => "access_denied",
        _ => "other",
    };
    eprintln!("MDReader provider OAuth: response_status={status} rejection={code}");
}
