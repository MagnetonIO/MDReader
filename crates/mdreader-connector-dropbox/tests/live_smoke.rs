//! Opt-in Dropbox provider acceptance.
//!
//! The ignored tests require a pinned, non-secret source contract plus
//! runtime-only credentials. The read-only gate never mutates provider data;
//! the guarded-write gate addresses only an explicitly opted-in dedicated
//! fixture and attempts hash-verified restoration before asserting outcomes.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use mdreader_connector_dropbox::{
    DropboxAccessToken, DropboxChangeCursor, DropboxChangeOutcome, DropboxConnectorError,
    DropboxCursorKey, DropboxDocumentError, DropboxHttpRequest, DropboxHttpResponse,
    DropboxRuntimeSecrets, DropboxSecretReference, DropboxSecretResolver, DropboxSource,
    DropboxTransport, DropboxTransportError, PinnedDropboxSourceSpec, DROPBOX_CONTENT_READ_SCOPE,
    DROPBOX_CONTENT_WRITE_SCOPE, DROPBOX_METADATA_READ_SCOPE,
};
use catdb_sdk::connectors::{read, CancellationToken, ReadContext, SourceError};
use catdb_sdk::query::{EntityQuery, EntityTarget};
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const CONFIG_ENV: &str = "CATDB_DROPBOX_LIVE_CONFIG_JSON";
const TOKEN_ENV: &str = "CATDB_DROPBOX_ACCESS_TOKEN";
const CURSOR_KEY_ENV: &str = "CATDB_DROPBOX_CURSOR_KEY_HEX";
const SCOPE_ATTESTATION_ENV: &str = "CATDB_DROPBOX_SCOPE_ATTESTATION_JSON";
const MAX_FIELDS: usize = 32;
const MAX_WORK_BUDGET: usize = 512;
const MAX_BYTE_BUDGET: usize = 16 * 1024 * 1024;
const MAX_DEADLINE_SECONDS: u64 = 120;
const MAX_ATTESTATION_LIFETIME_SECONDS: u64 = 15 * 60;
const ATTESTATION_CLOCK_SKEW_SECONDS: u64 = 60;
const GUARDED_WRITE_BODY: &[u8] =
    b"# CatDB guarded-write acceptance\n\nTemporary Dropbox fixture content.\n";
const STALE_WRITE_BODY: &[u8] =
    b"# CatDB stale-write probe\n\nThis body must never survive the gate.\n";

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ScopeProfile {
    ReadOnly,
    MarkdownWrite,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLiveConfig {
    pinned_spec: PinnedDropboxSourceSpec,
    credential_reference: String,
    oauth_audience: String,
    scope_attestation_public_key_hex: String,
    entity: String,
    fields: Vec<String>,
    expected: RawExpectedRecord,
    scope_profile: ScopeProfile,
    #[serde(default)]
    guarded_write_fixture_id: Option<String>,
    #[serde(default)]
    allow_guarded_write_test: bool,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScopeAttestation {
    version: String,
    provider: String,
    issuer: String,
    audience: String,
    scopes: Vec<String>,
    token_sha256: String,
    issued_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
    signature_ed25519: String,
}

struct LiveConfig {
    pinned_spec: PinnedDropboxSourceSpec,
    credential_reference: DropboxSecretReference,
    oauth_audience: String,
    scope_attestation_public_key: [u8; 32],
    entity: String,
    fields: Vec<String>,
    expected: ExpectedRecord,
    scope_profile: ScopeProfile,
    guarded_write_fixture_id: Option<String>,
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
    expected_audience: String,
    scope_attestation_public_key: [u8; 32],
    expected_scope_profile: ScopeProfile,
}

struct BlockingDropboxTransport {
    started: Arc<tokio::sync::Barrier>,
}

#[async_trait]
impl DropboxTransport for BlockingDropboxTransport {
    async fn send(
        &self,
        _request: DropboxHttpRequest,
    ) -> Result<DropboxHttpResponse, DropboxTransportError> {
        self.started.wait().await;
        std::future::pending().await
    }
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
        let token = Zeroizing::new(
            std::env::var(TOKEN_ENV).map_err(|_| DropboxConnectorError::InvalidSecret)?,
        );
        let cursor_key = Zeroizing::new(
            std::env::var(CURSOR_KEY_ENV).map_err(|_| DropboxConnectorError::InvalidSecret)?,
        );
        let attestation = Zeroizing::new(
            std::env::var(SCOPE_ATTESTATION_ENV)
                .map_err(|_| DropboxConnectorError::InvalidSecret)?,
        );
        verify_scope_attestation(
            &attestation,
            &token,
            &self.expected_audience,
            &self.scope_attestation_public_key,
            self.expected_scope_profile,
            unix_seconds()?,
        )?;
        Ok(DropboxRuntimeSecrets::new(
            DropboxAccessToken::new(token.as_str().to_owned())?,
            DropboxCursorKey::new(decode_hex::<32>(&cursor_key)?),
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
            expected_audience: config.oauth_audience,
            scope_attestation_public_key: config.scope_attestation_public_key,
            expected_scope_profile: config.scope_profile,
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
                .read_changes(&restored, &change_context)
                .await
                .expect("read one bounded Dropbox change page"),
            DropboxChangeOutcome::Page { .. }
        ),
        "a freshly issued Dropbox cursor must produce a typed change page"
    );
}

#[tokio::test]
#[ignore = "destructive-but-restoring; Task 9 must explicitly run this against a dedicated Dropbox fixture"]
async fn guarded_markdown_write_rejects_a_stale_revision_and_restores_fixture() {
    let config_json = std::env::var(CONFIG_ENV)
        .unwrap_or_else(|_| panic!("set non-secret {CONFIG_ENV} for the live provider gate"));
    let config = parse_live_config_json(&config_json)
        .unwrap_or_else(|error| panic!("{CONFIG_ENV} is invalid: {error}"));
    let fixture_id = config
        .guarded_write_fixture_id
        .clone()
        .expect("guarded-write parsing requires one dedicated fixture ID");
    let deadline = config.deadline;
    let work_budget = config.work_budget;
    let byte_budget = config.byte_budget;
    let page_size = config.page_size;
    let source = DropboxSource::open_pinned(
        config.pinned_spec,
        Arc::new(EnvironmentResolver {
            expected_reference: config.credential_reference,
            expected_audience: config.oauth_audience,
            scope_attestation_public_key: config.scope_attestation_public_key,
            expected_scope_profile: config.scope_profile,
        }),
    )
    .await
    .unwrap_or_else(|error| {
        panic!("open the configured Dropbox source with runtime-only credentials: {error}")
    });
    let operation_context = || read_context(deadline, work_budget, byte_budget, page_size);

    let original = source
        .download_markdown(&fixture_id, &operation_context())
        .await
        .expect("download the dedicated Dropbox fixture before mutation");
    assert_ne!(
        original.body(),
        GUARDED_WRITE_BODY,
        "the dedicated fixture must not start with the controlled test body"
    );
    assert_ne!(
        original.body(),
        STALE_WRITE_BODY,
        "the dedicated fixture must not start with the stale-write probe body"
    );
    let original_body = original.body().to_vec();
    let original_hash = blake3::hash(&original_body);
    let original_revision = original.revision().clone();

    // After the first mutation is dispatched, do not panic until restoration
    // has been attempted and independently verified.
    let first_update = source
        .update_markdown(
            &fixture_id,
            &original_revision,
            GUARDED_WRITE_BODY,
            &operation_context(),
        )
        .await;
    let after_first = source
        .download_markdown(&fixture_id, &operation_context())
        .await;
    let first_bytes_accepted = after_first
        .as_ref()
        .is_ok_and(|document| document.body() == GUARDED_WRITE_BODY);

    let stale_update = if first_bytes_accepted {
        Some(
            source
                .update_markdown(
                    &fixture_id,
                    &original_revision,
                    STALE_WRITE_BODY,
                    &operation_context(),
                )
                .await,
        )
    } else {
        None
    };
    let after_stale = if stale_update.is_some() {
        Some(
            source
                .download_markdown(&fixture_id, &operation_context())
                .await,
        )
    } else {
        None
    };
    let accepted_bytes_remain = after_stale
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .is_some_and(|document| document.body() == GUARDED_WRITE_BODY);

    let restore_revision = stale_update
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|receipt| receipt.revision().clone())
        .or_else(|| {
            after_stale
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .map(|document| document.revision().clone())
        })
        .or_else(|| {
            after_first
                .as_ref()
                .ok()
                .map(|document| document.revision().clone())
        })
        .or_else(|| {
            first_update
                .as_ref()
                .ok()
                .map(|receipt| receipt.revision().clone())
        });
    let provider_body_may_need_restore = stale_update.as_ref().is_some_and(|result| result.is_ok())
        || after_stale
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .is_some_and(|document| {
                document.body() == GUARDED_WRITE_BODY || document.body() == STALE_WRITE_BODY
            })
        || first_bytes_accepted;
    let restore_update = if provider_body_may_need_restore {
        match restore_revision.as_ref() {
            Some(revision) => Some(
                source
                    .update_markdown(&fixture_id, revision, &original_body, &operation_context())
                    .await,
            ),
            None => None,
        }
    } else {
        None
    };
    let restored = source
        .download_markdown(&fixture_id, &operation_context())
        .await;
    let original_restored = restored.as_ref().is_ok_and(|document| {
        document.body() == original_body && blake3::hash(document.body()) == original_hash
    });

    assert!(
        original_restored,
        "the dedicated Dropbox fixture must be hash-verified as restored before the gate exits"
    );
    assert!(
        restore_update.as_ref().is_some_and(|result| result.is_ok()),
        "restoration must return an accepted Dropbox receipt"
    );
    assert!(
        first_update.is_ok(),
        "the first guarded Dropbox update must return an accepted receipt"
    );
    assert!(
        matches!(
            stale_update,
            Some(Err(DropboxDocumentError::Conflict { .. }))
        ),
        "Dropbox must atomically reject the stale original revision"
    );
    assert!(
        accepted_bytes_remain,
        "the accepted controlled Dropbox bytes must remain after the stale write"
    );
}

fn parse_live_config_json(input: &str) -> Result<LiveConfig, String> {
    let raw: RawLiveConfig =
        serde_json::from_str(input).map_err(|error| format!("invalid JSON contract: {error}"))?;
    validate_atom("oauth_audience", &raw.oauth_audience, 512)?;
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
    if !matches!(raw.expected.field.as_str(), "body" | "contentHash") {
        return Err(
            "expected.field must be a content-bearing Dropbox field: body or contentHash"
                .to_string(),
        );
    }
    let value_blake3 = validate_blake3(&raw.expected.value_blake3)?;
    let guarded_write_fixture_id = match raw.scope_profile {
        ScopeProfile::ReadOnly => {
            if raw.allow_guarded_write_test || raw.guarded_write_fixture_id.is_some() {
                return Err(
                    "read-only scope_profile must not opt in to the guarded-write test".to_string(),
                );
            }
            None
        }
        ScopeProfile::MarkdownWrite => {
            if !raw.allow_guarded_write_test {
                return Err(
                    "markdown-write scope_profile requires allow_guarded_write_test true"
                        .to_string(),
                );
            }
            let fixture_id = raw.guarded_write_fixture_id.ok_or_else(|| {
                "markdown-write scope_profile requires guarded_write_fixture_id".to_string()
            })?;
            validate_dropbox_fixture_id(&fixture_id)?;
            Some(fixture_id)
        }
    };
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
        || raw.page_size > catdb_sdk::connectors::MAX_PAGE_SIZE
        || raw.page_size > raw.work_budget
    {
        return Err(
            "page_size must be positive and no greater than the work budget or connector maximum"
                .to_string(),
        );
    }
    let credential_reference =
        DropboxSecretReference::new(raw.credential_reference).map_err(|error| error.to_string())?;
    let scope_attestation_public_key = decode_hex::<32>(&raw.scope_attestation_public_key_hex)
        .map_err(|error| error.to_string())?;
    DropboxSource::declared_model(&raw.pinned_spec).map_err(|error| error.to_string())?;

    Ok(LiveConfig {
        pinned_spec: raw.pinned_spec,
        credential_reference,
        oauth_audience: raw.oauth_audience,
        scope_attestation_public_key,
        entity: raw.entity,
        fields: raw.fields,
        expected: ExpectedRecord {
            key: raw.expected.key,
            field: raw.expected.field,
            value_blake3,
        },
        scope_profile: raw.scope_profile,
        guarded_write_fixture_id,
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

fn validate_dropbox_fixture_id(value: &str) -> Result<(), String> {
    let Some(provider_id) = value.strip_prefix("id:") else {
        return Err("guarded_write_fixture_id must be one immutable Dropbox file ID".to_string());
    };
    if provider_id.is_empty()
        || value.len() > 1_024
        || !provider_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("guarded_write_fixture_id must be one immutable Dropbox file ID".to_string());
    }
    Ok(())
}

fn verify_scope_attestation(
    input: &str,
    token: &str,
    expected_audience: &str,
    public_key: &[u8; 32],
    scope_profile: ScopeProfile,
    now: u64,
) -> Result<(), DropboxConnectorError> {
    let attestation: RawScopeAttestation =
        serde_json::from_str(input).map_err(|_| DropboxConnectorError::InvalidSecret)?;
    let expected_scopes: &[&str] = match scope_profile {
        ScopeProfile::ReadOnly => &[DROPBOX_METADATA_READ_SCOPE, DROPBOX_CONTENT_READ_SCOPE],
        ScopeProfile::MarkdownWrite => &[
            DROPBOX_METADATA_READ_SCOPE,
            DROPBOX_CONTENT_READ_SCOPE,
            DROPBOX_CONTENT_WRITE_SCOPE,
        ],
    };
    if attestation.version != "catdb-live-scope-attestation/v1"
        || attestation.provider != "dropbox"
        || attestation.issuer != "https://www.dropbox.com"
        || attestation.audience != expected_audience
        || attestation.scopes.len() != expected_scopes.len()
        || !attestation
            .scopes
            .iter()
            .zip(expected_scopes.iter().copied())
            .all(|(actual, expected)| actual == expected)
        || attestation.token_sha256 != format!("{:x}", Sha256::digest(token.as_bytes()))
        || attestation.issued_at_unix_seconds > now.saturating_add(ATTESTATION_CLOCK_SKEW_SECONDS)
        || attestation.expires_at_unix_seconds <= now
        || attestation.expires_at_unix_seconds <= attestation.issued_at_unix_seconds
        || attestation
            .expires_at_unix_seconds
            .saturating_sub(attestation.issued_at_unix_seconds)
            > MAX_ATTESTATION_LIFETIME_SECONDS
    {
        return Err(DropboxConnectorError::InvalidSecret);
    }
    let canonical = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        attestation.version,
        attestation.provider,
        attestation.issuer,
        attestation.audience,
        attestation.scopes.join(" "),
        attestation.token_sha256,
        attestation.issued_at_unix_seconds,
        attestation.expires_at_unix_seconds
    );
    let signature = decode_hex::<64>(&attestation.signature_ed25519)?;
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(canonical.as_bytes(), &signature)
        .map_err(|_| DropboxConnectorError::InvalidSecret)
}

fn unix_seconds() -> Result<u64, DropboxConnectorError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| DropboxConnectorError::InvalidSecret)
}

fn decode_hex<const N: usize>(value: &str) -> Result<[u8; N], DropboxConnectorError> {
    if value.len() != N.saturating_mul(2) {
        return Err(DropboxConnectorError::InvalidSecret);
    }
    let mut key = [0u8; N];
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
        "oauth_audience": "dropbox-live-client",
        "scope_attestation_public_key_hex": "2222222222222222222222222222222222222222222222222222222222222222",
        "scope_profile": "read-only",
        "entity": "dropbox.Document",
        "fields": ["id", "body"],
        "expected": {
            "key": "/catdb-live/fixture.md",
            "field": "body",
            "value_blake3": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        }
    })
}

#[cfg(test)]
fn guarded_write_config_value() -> serde_json::Value {
    let mut value = valid_config_value();
    let object = value.as_object_mut().expect("object fixture");
    object.insert(
        "scope_profile".to_string(),
        serde_json::Value::String("markdown-write".to_string()),
    );
    object.insert(
        "guarded_write_fixture_id".to_string(),
        serde_json::Value::String("id:dedicated-guarded-write-fixture".to_string()),
    );
    object.insert(
        "allow_guarded_write_test".to_string(),
        serde_json::Value::Bool(true),
    );
    value
}

#[test]
fn guarded_write_config_accepts_an_explicit_markdown_write_fixture_opt_in() {
    parse_live_config_json(&guarded_write_config_value().to_string())
        .expect("the explicit guarded-write fixture contract must parse");
}

#[test]
fn guarded_write_config_fails_closed_without_every_explicit_safety_gate() {
    let mut missing_profile = guarded_write_config_value();
    missing_profile
        .as_object_mut()
        .expect("object fixture")
        .remove("scope_profile");

    let mut missing_fixture = guarded_write_config_value();
    missing_fixture
        .as_object_mut()
        .expect("object fixture")
        .remove("guarded_write_fixture_id");

    let mut disabled = guarded_write_config_value();
    disabled["allow_guarded_write_test"] = serde_json::Value::Bool(false);

    let mut path_instead_of_id = guarded_write_config_value();
    path_instead_of_id["guarded_write_fixture_id"] =
        serde_json::Value::String("/catdb-live/fixture.md".to_string());

    let mut read_only_opt_in = guarded_write_config_value();
    read_only_opt_in["scope_profile"] = serde_json::Value::String("read-only".to_string());

    for rejected in [
        missing_profile,
        missing_fixture,
        disabled,
        path_instead_of_id,
        read_only_opt_in,
    ] {
        assert!(
            parse_live_config_json(&rejected.to_string()).is_err(),
            "every guarded-write safety gate must fail closed"
        );
    }
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
    value["expected"]["field"] = serde_json::Value::String("contentHash".to_string());

    assert_eq!(
        parse_live_config_json(&value.to_string())
            .err()
            .expect("unprojected evidence must fail"),
        "expected.field must be included in fields"
    );
}

#[test]
fn live_config_rejects_identity_or_display_only_evidence() {
    for field in ["id", "path", "name"] {
        let mut value = valid_config_value();
        value["fields"] = serde_json::json!([field]);
        value["expected"]["field"] = serde_json::Value::String(field.to_string());

        let error = parse_live_config_json(&value.to_string())
            .err()
            .expect("identity and display fields must not stand in for content evidence");
        assert!(
            error.contains("content-bearing"),
            "unexpected rejection for {field}: {error}"
        );
    }
}

#[test]
fn live_config_rejects_an_invalid_schema_revision_before_secret_resolution() {
    for schema_version in [
        String::new(),
        "x".repeat(513),
        "dropbox-live-\u{0000}-v1".to_string(),
    ] {
        let mut value = valid_config_value();
        value["pinned_spec"]["schema_version"] = serde_json::Value::String(schema_version);

        let error = parse_live_config_json(&value.to_string())
            .err()
            .expect("invalid schema revisions must fail before opening the source");
        assert!(
            error.contains("schema_version"),
            "unexpected schema revision rejection: {error}"
        );
    }
}

#[test]
fn scope_attestation_is_bound_to_the_exact_read_only_grant() {
    let token = "dropbox-live-token-for-attestation-test";
    let audience = "dropbox-live-client";
    let signing_seed = [0x44; 32];
    let now = 1_700_000_000;
    let (valid, public_key) = sign_scope_attestation_for_test(
        token,
        audience,
        &[DROPBOX_METADATA_READ_SCOPE, DROPBOX_CONTENT_READ_SCOPE],
        now - 10,
        now + 300,
        &signing_seed,
    );

    assert!(verify_scope_attestation(
        &valid,
        token,
        audience,
        &public_key,
        ScopeProfile::ReadOnly,
        now
    )
    .is_ok());

    let (overprivileged, _) = sign_scope_attestation_for_test(
        token,
        audience,
        &[
            DROPBOX_METADATA_READ_SCOPE,
            DROPBOX_CONTENT_READ_SCOPE,
            "files.content.write",
        ],
        now - 10,
        now + 300,
        &signing_seed,
    );
    assert!(verify_scope_attestation(
        &overprivileged,
        token,
        audience,
        &public_key,
        ScopeProfile::ReadOnly,
        now
    )
    .is_err());
    assert!(verify_scope_attestation(
        &valid,
        "different-token",
        audience,
        &public_key,
        ScopeProfile::ReadOnly,
        now
    )
    .is_err());
    assert!(verify_scope_attestation(
        &valid,
        token,
        audience,
        &public_key,
        ScopeProfile::ReadOnly,
        now + 301
    )
    .is_err());
}

#[test]
fn markdown_write_scope_attestation_is_bound_to_the_exact_dropbox_grant() {
    let token = "dropbox-write-token-for-attestation-test";
    let audience = "dropbox-live-client";
    let signing_seed = [0x58; 32];
    let now = 1_700_000_000;
    let (write, public_key) = sign_scope_attestation_for_test(
        token,
        audience,
        &[
            DROPBOX_METADATA_READ_SCOPE,
            DROPBOX_CONTENT_READ_SCOPE,
            DROPBOX_CONTENT_WRITE_SCOPE,
        ],
        now - 10,
        now + 300,
        &signing_seed,
    );
    let (read_only, _) = sign_scope_attestation_for_test(
        token,
        audience,
        &[DROPBOX_METADATA_READ_SCOPE, DROPBOX_CONTENT_READ_SCOPE],
        now - 10,
        now + 300,
        &signing_seed,
    );

    assert!(verify_scope_attestation(
        &write,
        token,
        audience,
        &public_key,
        ScopeProfile::MarkdownWrite,
        now
    )
    .is_ok());
    assert!(verify_scope_attestation(
        &read_only,
        token,
        audience,
        &public_key,
        ScopeProfile::MarkdownWrite,
        now
    )
    .is_err());
    assert!(verify_scope_attestation(
        &write,
        token,
        audience,
        &public_key,
        ScopeProfile::ReadOnly,
        now
    )
    .is_err());
}

#[tokio::test]
async fn cancellation_interrupts_an_in_flight_production_connector_request() {
    let config =
        parse_live_config_json(&valid_config_value().to_string()).expect("valid source contract");
    let started = Arc::new(tokio::sync::Barrier::new(2));
    let source = DropboxSource::open_with_transport(
        config.pinned_spec,
        DropboxRuntimeSecrets::new(
            DropboxAccessToken::new("cancellation-test-token").expect("token"),
            DropboxCursorKey::new([0x32; 32]),
        ),
        Arc::new(BlockingDropboxTransport {
            started: Arc::clone(&started),
        }),
    )
    .expect("shipping Dropbox source");
    let cancellation = CancellationToken::new();
    let context = ReadContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_secs(2),
        4,
        4096,
        1,
        cancellation.clone(),
    )
    .expect("bounded cancellation context");
    let query = EntityQuery {
        target: EntityTarget::Entity(config.entity),
        filters: Vec::new(),
    };
    let operation = read(&source, &query, &config.fields, &context);
    tokio::pin!(operation);

    tokio::select! {
        result = &mut operation => panic!("request finished before cancellation: {result:?}"),
        _ = started.wait() => {}
    }
    cancellation.cancel();
    assert!(matches!(operation.await, Err(SourceError::Cancelled)));
}

#[cfg(test)]
fn sign_scope_attestation_for_test(
    token: &str,
    audience: &str,
    scopes: &[&str],
    issued_at: u64,
    expires_at: u64,
    key: &[u8; 32],
) -> (String, [u8; 32]) {
    let token_sha256 = format!("{:x}", Sha256::digest(token.as_bytes()));
    let canonical = format!(
        "catdb-live-scope-attestation/v1\ndropbox\nhttps://www.dropbox.com\n{audience}\n{}\n{token_sha256}\n{issued_at}\n{expires_at}",
        scopes.join(" ")
    );
    let signer = Ed25519KeyPair::from_seed_unchecked(key).expect("fixed Ed25519 test seed");
    let public_key = signer
        .public_key()
        .as_ref()
        .try_into()
        .expect("Ed25519 public key is 32 bytes");
    let signature = signer.sign(canonical.as_bytes());
    let json = serde_json::json!({
        "version": "catdb-live-scope-attestation/v1",
        "provider": "dropbox",
        "issuer": "https://www.dropbox.com",
        "audience": audience,
        "scopes": scopes,
        "token_sha256": token_sha256,
        "issued_at_unix_seconds": issued_at,
        "expires_at_unix_seconds": expires_at,
        "signature_ed25519": signature.as_ref().iter().map(|byte| format!("{byte:02x}")).collect::<String>()
    })
    .to_string();
    (json, public_key)
}

#[test]
fn live_config_and_resolver_debug_never_contain_secret_material() {
    let config =
        parse_live_config_json(&valid_config_value().to_string()).expect("valid non-secret config");
    let resolver = EnvironmentResolver {
        expected_reference: config.credential_reference,
        expected_audience: config.oauth_audience,
        scope_attestation_public_key: config.scope_attestation_public_key,
        expected_scope_profile: config.scope_profile,
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
        expected_audience: "dropbox-live-client".to_string(),
        scope_attestation_public_key: [0x22; 32],
        expected_scope_profile: ScopeProfile::ReadOnly,
    };

    assert!(matches!(
        resolver
            .resolve(&DropboxSecretReference::new("env://other").expect("reference"))
            .await,
        Err(DropboxConnectorError::InvalidSecret)
    ));
}
