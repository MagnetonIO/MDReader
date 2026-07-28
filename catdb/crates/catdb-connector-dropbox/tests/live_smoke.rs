//! Opt-in Dropbox provider acceptance.
//!
//! The ignored test is intentionally not a fixture: selecting it requires a
//! pinned, non-secret source contract plus runtime-only credentials and reads
//! the configured provider record through the production connector.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use catdb_connector_dropbox::{
    DropboxAccessToken, DropboxChangeCursor, DropboxChangeOutcome, DropboxConnectorError,
    DropboxCursorKey, DropboxRuntimeSecrets, DropboxSecretReference, DropboxSecretResolver,
    DropboxSource, PinnedDropboxSourceSpec,
};
use catdb_connectors::{read, CancellationToken, ReadContext};
use catdb_query::{EntityQuery, EntityTarget};
use serde::Deserialize;
use zeroize::Zeroizing;

const CONFIG_ENV: &str = "CATDB_DROPBOX_LIVE_CONFIG_JSON";
const TOKEN_ENV: &str = "CATDB_DROPBOX_ACCESS_TOKEN";
const CURSOR_KEY_ENV: &str = "CATDB_DROPBOX_CURSOR_KEY_HEX";
const MAX_FIELDS: usize = 32;
const MAX_WORK_BUDGET: usize = 512;
const MAX_BYTE_BUDGET: usize = 16 * 1024 * 1024;
const MAX_DEADLINE_SECONDS: u64 = 120;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLiveConfig {
    pinned_spec: PinnedDropboxSourceSpec,
    credential_reference: String,
    entity: String,
    fields: Vec<String>,
    expected: RawExpectedRecord,
    #[serde(default = "default_deadline_seconds")]
    deadline_seconds: u64,
    #[serde(default = "default_work_budget")]
    work_budget: usize,
    #[serde(default = "default_byte_budget")]
    byte_budget: usize,
    #[serde(default = "default_page_size")]
    page_size: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExpectedRecord {
    key: String,
    field: String,
    value_blake3: String,
}

struct LiveConfig {
    pinned_spec: PinnedDropboxSourceSpec,
    credential_reference: DropboxSecretReference,
    entity: String,
    fields: Vec<String>,
    expected: ExpectedRecord,
    deadline: Duration,
    work_budget: usize,
    byte_budget: usize,
    page_size: usize,
}

struct ExpectedRecord {
    key: String,
    field: String,
    value_blake3: String,
}

struct EnvironmentResolver {
    expected_reference: DropboxSecretReference,
}

impl fmt::Debug for EnvironmentResolver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EnvironmentDropboxSecrets([redacted])")
    }
}

#[async_trait]
impl DropboxSecretResolver for EnvironmentResolver {
    async fn resolve(
        &self,
        reference: &DropboxSecretReference,
    ) -> Result<DropboxRuntimeSecrets, DropboxConnectorError> {
        if reference != &self.expected_reference {
            return Err(DropboxConnectorError::InvalidSecret);
        }
        let token = std::env::var(TOKEN_ENV).map_err(|_| DropboxConnectorError::InvalidSecret)?;
        let cursor_key = Zeroizing::new(
            std::env::var(CURSOR_KEY_ENV).map_err(|_| DropboxConnectorError::InvalidSecret)?,
        );
        Ok(DropboxRuntimeSecrets::new(
            DropboxAccessToken::new(token)?,
            DropboxCursorKey::new(decode_cursor_key(&cursor_key)?),
        ))
    }
}

#[tokio::test]
#[ignore = "requires an explicitly provisioned read-only Dropbox fixture account"]
async fn provisioned_read_only_account_returns_the_pinned_record_and_change_cursor() {
    let config_json = std::env::var(CONFIG_ENV)
        .unwrap_or_else(|_| panic!("set non-secret {CONFIG_ENV} for the live provider gate"));
    let config = parse_live_config_json(&config_json)
        .unwrap_or_else(|error| panic!("{CONFIG_ENV} is invalid: {error}"));
    let source = DropboxSource::open_pinned(
        config.pinned_spec,
        Arc::new(EnvironmentResolver {
            expected_reference: config.credential_reference,
        }),
    )
    .await
    .unwrap_or_else(|error| {
        panic!("open the configured Dropbox source with runtime-only credentials: {error}")
    });

    let context = read_context(
        config.deadline,
        config.work_budget,
        config.byte_budget,
        config.page_size,
    );
    let answer = read(
        &source,
        &EntityQuery {
            target: EntityTarget::Entity(config.entity),
            filters: Vec::new(),
        },
        &config.fields,
        &context,
    )
    .await
    .expect("execute the bounded read-only Dropbox query");
    assert!(
        answer.scanned > 0,
        "the live gate must physically examine provider records"
    );

    let matches = answer
        .rows
        .iter()
        .filter(|row| row.key == config.expected.key)
        .collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "the configured provider record identity must be observed exactly once"
    );
    let value = matches[0]
        .value
        .get(&config.expected.field)
        .expect("the configured expected field must be projected");
    assert_eq!(
        digest_value(value),
        config.expected.value_blake3,
        "the provider value digest must match independently pinned fixture evidence"
    );

    let change_context = read_context(
        config.deadline,
        config.work_budget,
        config.byte_budget,
        config.page_size,
    );
    let cursor = source
        .start_change_tracking(&change_context)
        .await
        .expect("obtain the provider's current Dropbox change position");
    let restored = DropboxChangeCursor::from_bytes(cursor.as_bytes().to_vec())
        .expect("restore the authenticated bounded Dropbox cursor");
    assert!(
        matches!(
            source
                .read_changes(&restored, &BTreeMap::new(), &change_context)
                .await
                .expect("read one bounded Dropbox change page"),
            DropboxChangeOutcome::Page { .. }
        ),
        "a freshly issued Dropbox cursor must produce a typed change page"
    );
}

fn parse_live_config_json(input: &str) -> Result<LiveConfig, String> {
    let raw: RawLiveConfig =
        serde_json::from_str(input).map_err(|error| format!("invalid JSON contract: {error}"))?;
    validate_atom("entity", &raw.entity, 256)?;
    if raw.fields.is_empty() || raw.fields.len() > MAX_FIELDS {
        return Err(format!(
            "fields must contain between 1 and {MAX_FIELDS} entries"
        ));
    }
    let mut unique_fields = BTreeSet::new();
    for field in &raw.fields {
        validate_atom("field", field, 256)?;
        if !unique_fields.insert(field.as_str()) {
            return Err("fields must not contain duplicates".to_string());
        }
    }
    validate_atom("expected.key", &raw.expected.key, 512)?;
    validate_atom("expected.field", &raw.expected.field, 256)?;
    if !unique_fields.contains(raw.expected.field.as_str()) {
        return Err("expected.field must be included in fields".to_string());
    }
    let value_blake3 = validate_blake3(&raw.expected.value_blake3)?;
    if !(1..=MAX_DEADLINE_SECONDS).contains(&raw.deadline_seconds) {
        return Err(format!(
            "deadline_seconds must be between 1 and {MAX_DEADLINE_SECONDS}"
        ));
    }
    if !(1..=MAX_WORK_BUDGET).contains(&raw.work_budget) {
        return Err(format!(
            "work_budget must be between 1 and {MAX_WORK_BUDGET}"
        ));
    }
    if !(1..=MAX_BYTE_BUDGET).contains(&raw.byte_budget) {
        return Err(format!(
            "byte_budget must be between 1 and {MAX_BYTE_BUDGET}"
        ));
    }
    if raw.page_size == 0
        || raw.page_size > catdb_connectors::MAX_PAGE_SIZE
        || raw.page_size > raw.work_budget
    {
        return Err(
            "page_size must be positive and no greater than the work budget or connector maximum"
                .to_string(),
        );
    }
    let credential_reference =
        DropboxSecretReference::new(raw.credential_reference).map_err(|error| error.to_string())?;
    DropboxSource::declared_model(&raw.pinned_spec).map_err(|error| error.to_string())?;

    Ok(LiveConfig {
        pinned_spec: raw.pinned_spec,
        credential_reference,
        entity: raw.entity,
        fields: raw.fields,
        expected: ExpectedRecord {
            key: raw.expected.key,
            field: raw.expected.field,
            value_blake3,
        },
        deadline: Duration::from_secs(raw.deadline_seconds),
        work_budget: raw.work_budget,
        byte_budget: raw.byte_budget,
        page_size: raw.page_size,
    })
}

fn read_context(
    deadline: Duration,
    work_budget: usize,
    byte_budget: usize,
    page_size: usize,
) -> ReadContext {
    ReadContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + deadline,
        work_budget,
        byte_budget,
        page_size,
        CancellationToken::new(),
    )
    .expect("validated live-smoke read bounds")
}

fn validate_atom(field: &str, value: &str, maximum: usize) -> Result<(), String> {
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        return Err(format!(
            "{field} is empty, oversized, or contains control characters"
        ));
    }
    Ok(())
}

fn validate_blake3(value: &str) -> Result<String, String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("expected.value_blake3 must be a 64-character hexadecimal digest".to_string());
    }
    Ok(value.to_ascii_lowercase())
}

fn decode_cursor_key(value: &str) -> Result<[u8; 32], DropboxConnectorError> {
    if value.len() != 64 {
        return Err(DropboxConnectorError::InvalidSecret);
    }
    let mut key = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        key[index] = (decode_hex_nibble(pair[0])? << 4) | decode_hex_nibble(pair[1])?;
    }
    Ok(key)
}

fn decode_hex_nibble(value: u8) -> Result<u8, DropboxConnectorError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(DropboxConnectorError::InvalidSecret),
    }
}

fn digest_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => blake3::hash(value.as_bytes()).to_hex().to_string(),
        value => blake3::hash(
            &serde_json::to_vec(value).expect("provider values are valid serializable JSON"),
        )
        .to_hex()
        .to_string(),
    }
}

const fn default_deadline_seconds() -> u64 {
    30
}

const fn default_work_budget() -> usize {
    64
}

const fn default_byte_budget() -> usize {
    2 * 1024 * 1024
}

const fn default_page_size() -> usize {
    16
}

#[cfg(test)]
fn valid_config_value() -> serde_json::Value {
    serde_json::json!({
        "pinned_spec": {
            "config": {
                "source_id": "dropbox-live-fixture",
                "catdb_schema": "dropbox",
                "catdb_entity": "Document",
                "selected_path": "/catdb-live",
                "authorization_mode": "full_dropbox",
                "credential_reference": "env://dropbox-live"
            },
            "schema_version": "dropbox-live-v1"
        },
        "credential_reference": "env://dropbox-live",
        "entity": "dropbox.Document",
        "fields": ["id", "name"],
        "expected": {
            "key": "/catdb-live/fixture.md",
            "field": "name",
            "value_blake3": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        }
    })
}

#[test]
fn live_config_rejects_embedded_secret_material() {
    let mut value = valid_config_value();
    value.as_object_mut().expect("object fixture").insert(
        "access_token".to_string(),
        serde_json::Value::String("must-not-be-in-non-secret-config".to_string()),
    );
    let error = parse_live_config_json(&value.to_string())
        .err()
        .expect("secret-bearing config must fail closed");

    assert!(
        error.contains("unknown field"),
        "the non-secret config boundary must reject credential-shaped fields: {error}"
    );
}

#[test]
fn live_config_requires_the_expected_field_in_the_projection() {
    let mut value = valid_config_value();
    value["expected"]["field"] = serde_json::Value::String("body".to_string());

    assert_eq!(
        parse_live_config_json(&value.to_string())
            .err()
            .expect("unprojected evidence must fail"),
        "expected.field must be included in fields"
    );
}

#[test]
fn live_config_and_resolver_debug_never_contain_secret_material() {
    let config =
        parse_live_config_json(&valid_config_value().to_string()).expect("valid non-secret config");
    let resolver = EnvironmentResolver {
        expected_reference: config.credential_reference,
    };
    let debug = format!("{resolver:?}");

    assert_eq!(debug, "EnvironmentDropboxSecrets([redacted])");
    assert!(!debug.contains("env://dropbox-live"));
    assert!(!debug.contains(TOKEN_ENV));
}

#[tokio::test]
async fn live_resolver_rejects_a_different_credential_reference_before_env_lookup() {
    let resolver = EnvironmentResolver {
        expected_reference: DropboxSecretReference::new("env://dropbox-live").expect("reference"),
    };

    assert!(matches!(
        resolver
            .resolve(&DropboxSecretReference::new("env://other").expect("reference"))
            .await,
        Err(DropboxConnectorError::InvalidSecret)
    ));
}
