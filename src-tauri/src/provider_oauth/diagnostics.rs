//! Per-attempt diagnostics contain only closed public protocol vocabulary.
//! Never retain request URLs, identities, callback parameters or response text.
use super::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OAuthPhase {
    Preparation,
    Browser,
    Callback,
    TokenRequest,
    TokenPayload,
    RefreshTokenMissing,
    TokenPermissions,
    AccountRequest,
    AccountPayload,
    Grant,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OAuthRejection {
    InvalidClient,
    InvalidGrant,
    UnauthorizedClient,
    InvalidRequest,
    AccessDenied,
    Other,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OAuthDiagnostic {
    pub phase: OAuthPhase,
    pub response_status: Option<u16>,
    pub rejection: Option<OAuthRejection>,
}
#[derive(Debug)]
pub(crate) struct AuthorizationFailure {
    pub(crate) error: ProviderError,
    pub(crate) diagnostic: OAuthDiagnostic,
}
impl From<ProviderError> for AuthorizationFailure {
    fn from(error: ProviderError) -> Self {
        failure(OAuthPhase::Preparation, error)
    }
}
pub(super) fn failure(phase: OAuthPhase, error: ProviderError) -> AuthorizationFailure {
    AuthorizationFailure {
        error,
        diagnostic: OAuthDiagnostic {
            phase,
            response_status: None,
            rejection: None,
        },
    }
}
pub(super) fn rejected(
    phase: OAuthPhase,
    status: u16,
    bytes: &[u8],
    error: ProviderError,
) -> AuthorizationFailure {
    // Unknown provider text is reduced to Other, including nested Drive errors.
    let value = serde_json::from_slice::<serde_json::Value>(bytes).ok();
    let code = match value
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(|v| v.as_str())
    {
        Some("invalid_client") => OAuthRejection::InvalidClient,
        Some("invalid_grant") => OAuthRejection::InvalidGrant,
        Some("unauthorized_client") => OAuthRejection::UnauthorizedClient,
        Some("invalid_request") => OAuthRejection::InvalidRequest,
        Some("access_denied") => OAuthRejection::AccessDenied,
        _ => OAuthRejection::Other,
    };
    AuthorizationFailure {
        error,
        diagnostic: OAuthDiagnostic {
            phase,
            response_status: Some(status),
            rejection: Some(code),
        },
    }
}
