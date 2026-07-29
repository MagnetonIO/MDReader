use catdb_connectors::{SourceError, SourceId};

use super::{
    map_drive_document_error, map_dropbox_document_error, map_lifecycle_error, CloudDocumentError,
    MarkdownWriteCapability,
};
use crate::cloud_accounts::CloudSourceError;

#[test]
fn drive_adapter_maps_closed_document_failures_without_provider_detail() {
    let source_id = SourceId::new("drive-source");

    assert_eq!(
        map_drive_document_error(
            catdb_connector_google_drive::DriveDocumentError::Authentication {
                source_id: source_id.clone(),
            }
        ),
        CloudDocumentError::Unauthorized
    );
    assert_eq!(
        map_drive_document_error(catdb_connector_google_drive::DriveDocumentError::Conflict {
            source_id: source_id.clone(),
        }),
        CloudDocumentError::Conflict
    );
    assert_eq!(
        map_drive_document_error(catdb_connector_google_drive::DriveDocumentError::Source {
            error: SourceError::DeadlineExceeded,
        }),
        CloudDocumentError::Timeout
    );
    assert_eq!(
        map_drive_document_error(catdb_connector_google_drive::DriveDocumentError::Source {
            error: SourceError::Cancelled,
        }),
        CloudDocumentError::Cancelled
    );
}

#[test]
fn dropbox_adapter_maps_closed_document_failures_without_provider_detail() {
    let source_id = SourceId::new("dropbox-source");

    assert_eq!(
        map_dropbox_document_error(catdb_connector_dropbox::DropboxDocumentError::NotFound {
            source_id: source_id.clone(),
        }),
        CloudDocumentError::NotFound
    );
    assert_eq!(
        map_dropbox_document_error(catdb_connector_dropbox::DropboxDocumentError::InvalidUtf8 {
            source_id: source_id.clone(),
        }),
        CloudDocumentError::InvalidUtf8
    );
    assert_eq!(
        map_dropbox_document_error(catdb_connector_dropbox::DropboxDocumentError::RateLimited {
            source_id,
            retry_after_ms: Some(125),
        }),
        CloudDocumentError::RateLimited {
            retry_after_ms: Some(125),
        }
    );
}

#[test]
fn write_capability_stays_pending_until_live_provider_acceptance() {
    assert_eq!(
        MarkdownWriteCapability::PendingAcceptance.error(),
        CloudDocumentError::WriteCapabilityPendingAcceptance
    );
}

#[test]
fn lifecycle_pause_and_replacement_fail_closed_at_the_adapter_boundary() {
    assert_eq!(
        map_lifecycle_error(CloudSourceError::SourcePaused),
        CloudDocumentError::SourcePaused
    );
    assert_eq!(
        map_lifecycle_error(CloudSourceError::QueryRetired),
        CloudDocumentError::SourceDisconnected
    );
}
