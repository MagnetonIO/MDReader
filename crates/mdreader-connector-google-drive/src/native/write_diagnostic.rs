//! Bounded public Drive write failure diagnostics, never response dumps.
use super::Response;
/// Static failure phases only: no provider payload, header, token, path or error
/// formatting is accepted by this diagnostic boundary.
#[derive(Clone, Copy)]
pub(super) enum WritePhase {
    PreflightRead,
    PreflightRevision,
    PreflightValidator,
    PutTransport,
    PutStatus,
    ResponseDecode,
    ResponseChecksumMissing,
    ResponseChecksumMismatch,
    ResponseRevisionSize,
    ConfirmationRead,
    ConfirmationMismatch,
    CreateCurrentProofConfirmed,
}
fn event(
    operation: uuid::Uuid,
    phase: WritePhase,
    status: Option<u16>,
    reason: Option<&'static str>,
) -> String {
    let phase = match phase {
        WritePhase::PreflightRead => "preflight_read",
        WritePhase::PreflightRevision => "preflight_revision",
        WritePhase::PreflightValidator => "preflight_validator",
        WritePhase::PutTransport => "put_transport",
        WritePhase::PutStatus => "put_status",
        WritePhase::ResponseDecode => "response_decode",
        WritePhase::ResponseChecksumMissing => "response_checksum_missing",
        WritePhase::ResponseChecksumMismatch => "response_checksum_mismatch",
        WritePhase::ResponseRevisionSize => "response_revision_size",
        WritePhase::ConfirmationRead => "confirmation_read",
        WritePhase::ConfirmationMismatch => "confirmation_mismatch",
        WritePhase::CreateCurrentProofConfirmed => "create_current_proof_confirmed",
    };
    // A UUID is already the public operation identity. Status is numeric only.
    format!(
        "MDREADER_DRIVE_WRITE_V1 operation={operation} phase={phase} status={} reason={}",
        status
            .map(|v| v.to_string())
            .unwrap_or_else(|| "absent".into()),
        reason.unwrap_or("absent")
    )
}

pub(super) fn failure(operation: uuid::Uuid, phase: WritePhase, status: Option<u16>) {
    eprintln!("{}", event(operation, phase, status, None));
}
/// Only equality booleans are accepted: no raw observed or expected fields.
pub(super) fn comparison(
    operation: uuid::Uuid,
    revision: bool,
    hash: bool,
    size: bool,
    name: bool,
) {
    eprintln!(
        "MDREADER_DRIVE_COMPARISON_V1 operation={operation} revision_equal={revision} hash_equal={hash} size_equal={size} name_equal={name}"
    );
}
pub(super) fn refused(operation: uuid::Uuid, response: &Response) {
    let reason = matches!(response.status(), 400 | 403).then(|| public_reason(response));
    eprintln!(
        "{}",
        event(
            operation,
            WritePhase::PutStatus,
            Some(response.status()),
            reason
        )
    );
}
fn public_reason(response: &Response) -> &'static str {
    if response.body().len() > 16384 {
        return "other";
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(response.body()) else {
        return "other";
    };
    let Some(reason) = v.pointer("/error/errors/0/reason").and_then(|v| v.as_str()) else {
        return "other";
    };
    match reason {
        "insufficientPermissions" => "insufficient_permissions",
        "insufficientFilePermissions" => "insufficient_file_permissions",
        "badRequest" => "bad_request",
        "invalid" => "invalid",
        "invalidParameter" => "invalid_parameter",
        "authError" => "auth_error",
        "forbidden" => "forbidden",
        "rateLimitExceeded" => "rate_limit_exceeded",
        "userRateLimitExceeded" => "user_rate_limit_exceeded",
        "dailyLimitExceeded" => "daily_limit_exceeded",
        "quotaExceeded" => "quota_exceeded",
        _ => "other",
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[test]
    fn allowlisted_public_reason_never_formats_response_messages_or_unknown_values() {
        let id = uuid::Uuid::parse_str("40000000-0000-4000-8000-000000000001").unwrap();
        for (reason, expected) in [
            ("insufficientPermissions", "insufficient_permissions"),
            ("badRequest", "bad_request"),
            ("secret-sentinel-token", "other"),
        ] {
            let body=serde_json::to_vec(&serde_json::json!({"error":{"message":"secret-sentinel-body","errors":[{"reason":reason,"message":"secret-sentinel-path"}]}})).unwrap();
            let response = Response::new(
                403,
                BTreeMap::from([("etag".into(), "secret-sentinel-tag".into())]),
                body,
            )
            .unwrap();
            assert_eq!(public_reason(&response), expected);
            let line = event(
                id,
                WritePhase::PutStatus,
                Some(response.status()),
                Some(public_reason(&response)),
            );
            assert!(line.contains("phase=put_status status=403"));
            assert!(!line.contains("secret-sentinel"));
        }
        for body in [b"malformed secret-sentinel".to_vec(), vec![b'a'; 16385]] {
            assert_eq!(
                public_reason(&Response::new(400, BTreeMap::new(), body).unwrap()),
                "other"
            );
        }
    }
}
