//! Decoding and round-trip validation for retired connection metadata.
//! These records cannot create a runtime connection or grant. Versions 7–9
//! retain their original wire shapes for offline preservation and rollback.
use super::{
    valid_library_id, LibraryError, LibraryIndexPolicy, LibrarySourceCapabilities,
    LibrarySourceContract,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const MAX_CONNECTED_CATDB_SOURCES: usize = 128;

pub(super) fn legacy_contract(catalog_revision_id: uuid::Uuid) -> LibrarySourceContract {
    LibrarySourceContract {
        connector_version: "connected-catdb/v1".to_string(),
        capabilities: LibrarySourceCapabilities {
            read: true,
            write: false,
            search: true,
            watch: false,
            open_original: false,
            download: false,
        },
        // The remote revision owns the mapping choice. MDReader records the
        // exact revision it pins rather than fabricating a local mapping.
        mapping_version: catalog_revision_id.to_string(),
        index_policy: LibraryIndexPolicy::ReadThrough,
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ConnectedCatdbSourceDescriptor {
    pub source_id: uuid::Uuid,
    pub source_version_id: uuid::Uuid,
    pub runtime_bound: bool,
    pub connector_id: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ConnectedCatdbSourceRecord {
    pub(super) id: String,
    pub(super) label: String,
    pub(super) server_origin: String,
    pub(super) credential_reference: String,
    pub(super) deployment_id: uuid::Uuid,
    pub(super) workspace_id: uuid::Uuid,
    pub(super) catalog_revision_id: uuid::Uuid,
    pub(super) default_schema_version_id: uuid::Uuid,
    pub(super) permitted_sources: Vec<ConnectedCatdbSourceDescriptor>,
    pub(super) selected_source_ids: Vec<uuid::Uuid>,
    pub(super) contract: LibrarySourceContract,
}

impl std::fmt::Debug for ConnectedCatdbSourceRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConnectedCatdbSourceRecord")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("server_origin", &self.server_origin)
            .field("credential_reference", &"[native-secure-reference]")
            .field("deployment_id", &self.deployment_id)
            .field("workspace_id", &self.workspace_id)
            .field("catalog_revision_id", &self.catalog_revision_id)
            .field("default_schema_version_id", &self.default_schema_version_id)
            .field("permitted_sources", &self.permitted_sources)
            .field("selected_source_ids", &self.selected_source_ids)
            .field("contract", &self.contract)
            .finish()
    }
}

impl ConnectedCatdbSourceRecord {
    pub(super) fn refresh_contract(&mut self) {
        self.contract = legacy_contract(self.catalog_revision_id);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConnectedCatdbSourceRecordV7 {
    pub(super) id: String,
    label: String,
    server_origin: String,
    credential_reference: String,
    deployment_id: uuid::Uuid,
    workspace_id: uuid::Uuid,
    catalog_revision_id: uuid::Uuid,
    permitted_sources: Vec<ConnectedCatdbSourceDescriptor>,
    selected_source_ids: Vec<uuid::Uuid>,
    contract: LibrarySourceContract,
}

fn valid_connected_text(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn valid_connected_origin(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    (url.scheme() == "https" || (url.scheme() == "http" && loopback))
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path() == "/"
}

pub(super) fn validate_connected_catdb_source(
    source: &ConnectedCatdbSourceRecord,
) -> Result<(), LibraryError> {
    if !valid_library_id(&source.id)
        || !valid_connected_text(&source.label, 200)
        || !valid_connected_origin(&source.server_origin)
        || !valid_connected_text(&source.credential_reference, 256)
        || source.deployment_id.is_nil()
        || source.workspace_id.is_nil()
        || source.catalog_revision_id.is_nil()
        || source.default_schema_version_id.is_nil()
        || source.permitted_sources.len() > MAX_CONNECTED_CATDB_SOURCES
        || source.selected_source_ids.len() > MAX_CONNECTED_CATDB_SOURCES
        || source.contract != legacy_contract(source.catalog_revision_id)
    {
        return Err(LibraryError::CorruptRegistry);
    }
    let mut permitted = HashSet::new();
    let mut permitted_versions = HashSet::new();
    for descriptor in &source.permitted_sources {
        if descriptor.source_id.is_nil()
            || descriptor.source_version_id.is_nil()
            || !permitted.insert(descriptor.source_id)
            || !permitted_versions.insert(descriptor.source_version_id)
            || descriptor
                .connector_id
                .as_deref()
                .is_some_and(|connector| !valid_connected_text(connector, 200))
        {
            return Err(LibraryError::CorruptRegistry);
        }
    }
    let mut selected = HashSet::new();
    for source_id in &source.selected_source_ids {
        let Some(descriptor) = source
            .permitted_sources
            .iter()
            .find(|descriptor| descriptor.source_id == *source_id)
        else {
            return Err(LibraryError::CorruptRegistry);
        };
        if !descriptor.runtime_bound || !selected.insert(*source_id) {
            return Err(LibraryError::CorruptRegistry);
        }
    }
    Ok(())
}

pub(super) fn validate_connected_catdb_source_v7(
    source: &ConnectedCatdbSourceRecordV7,
) -> Result<(), LibraryError> {
    if !valid_library_id(&source.id)
        || !valid_connected_text(&source.label, 200)
        || !valid_connected_origin(&source.server_origin)
        || !valid_connected_text(&source.credential_reference, 256)
        || source.deployment_id.is_nil()
        || source.workspace_id.is_nil()
        || source.catalog_revision_id.is_nil()
        || source.permitted_sources.len() > MAX_CONNECTED_CATDB_SOURCES
        || source.selected_source_ids.len() > MAX_CONNECTED_CATDB_SOURCES
        || source.contract != legacy_contract(source.catalog_revision_id)
    {
        return Err(LibraryError::CorruptRegistry);
    }
    let mut permitted = HashSet::new();
    let mut permitted_versions = HashSet::new();
    for descriptor in &source.permitted_sources {
        if descriptor.source_id.is_nil()
            || descriptor.source_version_id.is_nil()
            || !permitted.insert(descriptor.source_id)
            || !permitted_versions.insert(descriptor.source_version_id)
            || descriptor
                .connector_id
                .as_deref()
                .is_some_and(|connector| !valid_connected_text(connector, 200))
        {
            return Err(LibraryError::CorruptRegistry);
        }
    }
    let mut selected = HashSet::new();
    for source_id in &source.selected_source_ids {
        let Some(descriptor) = source
            .permitted_sources
            .iter()
            .find(|descriptor| descriptor.source_id == *source_id)
        else {
            return Err(LibraryError::CorruptRegistry);
        };
        if !descriptor.runtime_bound || !selected.insert(*source_id) {
            return Err(LibraryError::CorruptRegistry);
        }
    }
    Ok(())
}
