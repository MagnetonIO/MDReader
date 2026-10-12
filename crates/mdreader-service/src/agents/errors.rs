//! Fixed public setup failures; never exposes client configuration or OS paths.
use mdreader_core::ReaderError;
use mdreader_integrations::IntegrationError;

const TOO_LARGE: &str = "Agent configuration is too large for automatic setup.";
const CONFLICT: &str =
    "Agent integration conflicts with existing settings. Existing files were kept.";
const INVALID: &str = "Agent configuration could not be read. Existing files were kept.";
const BUSY: &str = "Agent setup is busy. Try again.";
const UNSUPPORTED: &str = "This agent version or platform does not support the integration.";
const FAILED: &str = "Agent configuration could not be updated. Existing files were kept.";

pub(super) fn integration_error(error: IntegrationError) -> ReaderError {
    let message = match error {
        IntegrationError::Bounds => TOO_LARGE,
        IntegrationError::Conflict => CONFLICT,
        IntegrationError::Invalid => INVALID,
        IntegrationError::Busy => BUSY,
        IntegrationError::Unsupported => UNSUPPORTED,
        _ => FAILED,
    };
    ReaderError::Invalid(message.into())
}

pub(crate) fn public_error(message: &str) -> Option<&'static str> {
    [TOO_LARGE, CONFLICT, INVALID, BUSY, UNSUPPORTED, FAILED]
        .into_iter()
        .find(|known| *known == message)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn setup_failures_cross_ipc_as_fixed_public_messages() {
        for error in [
            IntegrationError::Bounds,
            IntegrationError::Conflict,
            IntegrationError::Invalid,
            IntegrationError::Busy,
        ] {
            let reply = crate::wire_error(integration_error(error));
            assert_eq!(reply.code, "agent_setup");
            assert!(public_error(&reply.message).is_some());
            assert_ne!(reply.message, "Document revision changed");
        }
    }
    #[test]
    fn unknown_paths_and_os_errors_are_never_public_setup_details() {
        let reply = crate::wire_error(ReaderError::Invalid("/private/personal/config".into()));
        assert_eq!(reply.message, "Invalid reader request");
        let error = IntegrationError::Io(std::io::Error::other("/private/personal/config"));
        let reply = crate::wire_error(integration_error(error));
        assert_eq!(reply.message, FAILED);
        assert!(!reply.message.contains("/private"));
    }
}
