//! Small, local-only diagnostic logging.
//!
//! The event vocabulary is intentionally closed: callers can report lifecycle
//! phases, numeric counts, opaque source digests, and typed error codes, but
//! there is no field for document text, query text, credentials, tokens, or a
//! filesystem path. Export reparses bounded records against the exact schema
//! and reconstructs canonical output, so legacy/corrupt bytes are never copied
//! into a support bundle.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::file_lock::AdvisoryFileLock;
use mdreader_core::SourceRoot;

mod preservation;

const DIAGNOSTIC_SETTINGS_VERSION: u32 = 1;
const DIAGNOSTIC_RECORD_SCHEMA: &str = "mdreader.diagnostics";
const DIAGNOSTIC_RECORD_VERSION: u32 = 1;
const MAX_DIAGNOSTIC_RECORD_BYTES: usize = 4 * 1_024;
pub(crate) const MAX_LOG_BYTES: usize = 256 * 1_024;
pub(crate) const LOG_RETENTION: usize = 3;
pub(crate) const MAX_EXPORT_BYTES: usize = 512 * 1_024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DiagnosticLevel {
    Errors,
    Standard,
    Verbose,
}

impl DiagnosticLevel {
    fn permits(self, required: Self) -> bool {
        let rank = |level| match level {
            Self::Errors => 0,
            Self::Standard => 1,
            Self::Verbose => 2,
        };
        rank(self) >= rank(required)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DiagnosticIndexPhase {
    Loading,
    Importing,
    Reembedding,
    Done,
    ModelUnavailable,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DiagnosticWatcherState {
    Idle,
    Watching,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DiagnosticErrorCode {
    InvalidSource,
    SourceMissing,
    SourceUnreadable,
    WatcherUnavailable,
    IndexUnavailable,
    PersistenceUnavailable,
    StartupFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub(crate) enum DiagnosticEvent {
    ApplicationStarted,
    Workspace {
        source_id: String,
        active: bool,
    },
    Watcher {
        source_id: String,
        state: DiagnosticWatcherState,
        error: Option<DiagnosticErrorCode>,
    },
    Index {
        source_id: String,
        phase: DiagnosticIndexPhase,
        files_done: u32,
        files_total: u32,
        chunks: u32,
        error: Option<DiagnosticErrorCode>,
    },
    Error {
        code: DiagnosticErrorCode,
    },
}

impl DiagnosticEvent {
    fn required_level(&self) -> DiagnosticLevel {
        match self {
            Self::Error { .. }
            | Self::Index {
                phase: DiagnosticIndexPhase::Error | DiagnosticIndexPhase::ModelUnavailable,
                ..
            }
            | Self::Watcher {
                state: DiagnosticWatcherState::Error,
                ..
            } => DiagnosticLevel::Errors,
            Self::Index {
                phase:
                    DiagnosticIndexPhase::Loading
                    | DiagnosticIndexPhase::Importing
                    | DiagnosticIndexPhase::Reembedding,
                ..
            } => DiagnosticLevel::Verbose,
            Self::ApplicationStarted
            | Self::Workspace { .. }
            | Self::Watcher { .. }
            | Self::Index { .. } => DiagnosticLevel::Standard,
        }
    }

    fn validate(&self) -> Result<(), DiagnosticError> {
        let source_id = match self {
            Self::Workspace { source_id, .. }
            | Self::Watcher { source_id, .. }
            | Self::Index { source_id, .. } => Some(source_id),
            Self::ApplicationStarted | Self::Error { .. } => None,
        };
        if source_id.is_some_and(|id| {
            id.len() != 64
                || !id
                    .as_bytes()
                    .iter()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        }) {
            return Err(DiagnosticError::InvalidEvent {
                reason: DiagnosticFailureReason::EventInvalid,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum DiagnosticError {
    ConfirmationRequired,
    CorruptSettings { reason: DiagnosticFailureReason },
    InvalidEvent { reason: DiagnosticFailureReason },
    WriteFailed { reason: DiagnosticFailureReason },
    ExportFailed { reason: DiagnosticFailureReason },
    ClearFailed { reason: DiagnosticFailureReason },
    RevealFailed { reason: DiagnosticFailureReason },
}

#[derive(Debug, Clone, Copy, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DiagnosticFailureReason {
    AuthorityUnavailable,
    AuthorityChanged,
    LockUnavailable,
    SettingsUnavailable,
    SettingsInvalid,
    EventInvalid,
    StorageUnavailable,
    DestinationUnavailable,
}

impl std::fmt::Display for DiagnosticError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConfirmationRequired => {
                formatter.write_str("clearing diagnostics requires explicit confirmation")
            }
            Self::CorruptSettings { reason }
            | Self::InvalidEvent { reason }
            | Self::WriteFailed { reason }
            | Self::ExportFailed { reason }
            | Self::ClearFailed { reason }
            | Self::RevealFailed { reason } => formatter.write_str(reason.as_str()),
        }
    }
}

impl std::error::Error for DiagnosticError {}

impl DiagnosticFailureReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::AuthorityUnavailable => "authority-unavailable",
            Self::AuthorityChanged => "authority-changed",
            Self::LockUnavailable => "lock-unavailable",
            Self::SettingsUnavailable => "settings-unavailable",
            Self::SettingsInvalid => "settings-invalid",
            Self::EventInvalid => "event-invalid",
            Self::StorageUnavailable => "storage-unavailable",
            Self::DestinationUnavailable => "destination-unavailable",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiagnosticRecord {
    schema: String,
    schema_version: u32,
    timestamp_ms: u64,
    app: String,
    app_version: String,
    platform: String,
    event: DiagnosticEvent,
}

impl DiagnosticRecord {
    fn validate(&self) -> Result<(), DiagnosticError> {
        if self.schema != DIAGNOSTIC_RECORD_SCHEMA {
            return Err(DiagnosticError::InvalidEvent {
                reason: DiagnosticFailureReason::EventInvalid,
            });
        }
        if self.schema_version != DIAGNOSTIC_RECORD_VERSION {
            return Err(DiagnosticError::InvalidEvent {
                reason: DiagnosticFailureReason::EventInvalid,
            });
        }
        if self.app != "MDReader" || self.app_version != env!("CARGO_PKG_VERSION") {
            return Err(DiagnosticError::InvalidEvent {
                reason: DiagnosticFailureReason::EventInvalid,
            });
        }
        if self.platform != std::env::consts::OS {
            return Err(DiagnosticError::InvalidEvent {
                reason: DiagnosticFailureReason::EventInvalid,
            });
        }
        self.event.validate()
    }
}

fn parse_diagnostic_record(line: &[u8]) -> Result<DiagnosticRecord, ()> {
    let raw: serde_json::Value = serde_json::from_slice(line).map_err(|_| ())?;
    let object = raw.as_object().ok_or(())?;
    if !has_exact_keys(
        object,
        &[
            "schema",
            "schema_version",
            "timestamp_ms",
            "app",
            "app_version",
            "platform",
            "event",
        ],
    ) {
        return Err(());
    }
    let event = object
        .get("event")
        .and_then(serde_json::Value::as_object)
        .ok_or(())?;
    let expected_event_keys: &[&str] = match event
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or(())?
    {
        "application-started" => &["kind"],
        "workspace" => &["kind", "source_id", "active"],
        "watcher" => &["kind", "source_id", "state", "error"],
        "index" => &[
            "kind",
            "source_id",
            "phase",
            "files_done",
            "files_total",
            "chunks",
            "error",
        ],
        "error" => &["kind", "code"],
        _ => return Err(()),
    };
    if !has_exact_keys(event, expected_event_keys) {
        return Err(());
    }
    let record: DiagnosticRecord = serde_json::from_value(raw).map_err(|_| ())?;
    record.validate().map_err(|_| ())?;
    Ok(record)
}

fn has_exact_keys(object: &serde_json::Map<String, serde_json::Value>, expected: &[&str]) -> bool {
    object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDiagnosticSettings {
    version: u32,
    level: DiagnosticLevel,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct DiagnosticInfo {
    pub level: DiagnosticLevel,
    pub max_file_bytes: u32,
    pub max_retained_files: u32,
    pub max_export_bytes: u32,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DiagnosticExportStatus {
    Cancelled,
    Exported,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub(crate) struct DiagnosticExport {
    pub status: DiagnosticExportStatus,
    pub bytes_written: u32,
}

pub(crate) struct DiagnosticsState {
    log: DiagnosticLog,
}

impl DiagnosticsState {
    pub(crate) fn new(directory: PathBuf) -> Self {
        Self {
            log: DiagnosticLog::new(directory),
        }
    }

    fn info(&self) -> Result<DiagnosticInfo, DiagnosticError> {
        diagnostic_info(&self.log)
    }

    fn set_level(&self, level: DiagnosticLevel) -> Result<DiagnosticInfo, DiagnosticError> {
        self.log.set_level(level)?;
        diagnostic_info(&self.log)
    }

    fn append(&self, event: DiagnosticEvent) -> Result<(), DiagnosticError> {
        self.log.append(event)
    }

    fn export_to(&self, destination: &Path) -> Result<(), DiagnosticError> {
        self.log.export_to(destination)
    }

    fn clear(&self, confirmed: bool) -> Result<DiagnosticInfo, DiagnosticError> {
        let level = self.log.clear(confirmed)?;
        Ok(diagnostic_info_with_level(&self.log, level))
    }

    fn reveal_with(
        &self,
        reveal: impl FnOnce(&Path) -> Result<(), DiagnosticError>,
    ) -> Result<(), DiagnosticError> {
        let (_guard, _lease, authority) = self
            .log
            .acquire_transaction(DiagnosticTransaction::Reveal)?;
        let reveal_result = reveal(self.log.directory());
        authority
            .verify()
            .map_err(|_| DiagnosticError::RevealFailed {
                reason: DiagnosticFailureReason::AuthorityChanged,
            })?;
        reveal_result
    }
}

pub(crate) struct DiagnosticLog {
    directory: PathBuf,
    authority: Result<SourceRoot, DiagnosticFailureReason>,
    max_log_bytes: usize,
    retention: usize,
    max_export_bytes: usize,
    lock: Mutex<()>,
}

impl std::fmt::Debug for DiagnosticLog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DiagnosticLog")
            .field("authority", &"<redacted>")
            .field("max_log_bytes", &self.max_log_bytes)
            .field("retention", &self.retention)
            .field("max_export_bytes", &self.max_export_bytes)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy)]
enum DiagnosticTransaction {
    Write,
    Export,
    Clear,
    Reveal,
}

impl DiagnosticTransaction {
    fn error(self, reason: DiagnosticFailureReason) -> DiagnosticError {
        match self {
            Self::Write => DiagnosticError::WriteFailed { reason },
            Self::Export => DiagnosticError::ExportFailed { reason },
            Self::Clear => DiagnosticError::ClearFailed { reason },
            Self::Reveal => DiagnosticError::RevealFailed { reason },
        }
    }
}

impl DiagnosticLog {
    pub(crate) fn new(directory: impl AsRef<Path>) -> Self {
        Self::with_limits(directory, MAX_LOG_BYTES, LOG_RETENTION, MAX_EXPORT_BYTES)
    }

    pub(crate) fn with_limits(
        directory: impl AsRef<Path>,
        max_log_bytes: usize,
        retention: usize,
        max_export_bytes: usize,
    ) -> Self {
        let directory = directory.as_ref().to_path_buf();
        let authority = initialize_directory_authority(&directory);
        Self {
            directory,
            authority,
            max_log_bytes: max_log_bytes.max(1),
            retention: retention.max(1),
            max_export_bytes: max_export_bytes.max(1),
            lock: Mutex::new(()),
        }
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    fn acquire_transaction(
        &self,
        transaction: DiagnosticTransaction,
    ) -> Result<(std::sync::MutexGuard<'_, ()>, AdvisoryFileLock, SourceRoot), DiagnosticError>
    {
        let guard = self
            .lock
            .lock()
            .map_err(|_| transaction.error(DiagnosticFailureReason::LockUnavailable))?;
        let authority = self.authority.as_ref().map_err(|error| {
            let _ = error;
            transaction.error(DiagnosticFailureReason::AuthorityUnavailable)
        })?;
        authority
            .verify()
            .map_err(|_| transaction.error(DiagnosticFailureReason::AuthorityChanged))?;
        let lock_file = authority
            .open_lock_relative(Path::new("diagnostics.lock"))
            .map_err(|_| transaction.error(DiagnosticFailureReason::LockUnavailable))?;
        let lease = AdvisoryFileLock::acquire_file(lock_file)
            .map_err(|_| transaction.error(DiagnosticFailureReason::LockUnavailable))?;
        authority
            .verify()
            .map_err(|_| transaction.error(DiagnosticFailureReason::AuthorityChanged))?;
        Ok((guard, lease, authority.clone()))
    }

    fn log_relative(&self, slot: usize) -> PathBuf {
        if slot == 0 {
            PathBuf::from("mdreader.log")
        } else {
            PathBuf::from(format!("mdreader.log.{slot}"))
        }
    }

    #[cfg(test)]
    fn log_path(&self, slot: usize) -> PathBuf {
        self.directory.join(self.log_relative(slot))
    }

    #[cfg(test)]
    pub(crate) fn log_files(&self) -> Result<Vec<PathBuf>, DiagnosticError> {
        let (_guard, _lease, authority) = self.acquire_transaction(DiagnosticTransaction::Write)?;
        let mut files = Vec::new();
        for slot in (0..self.retention).rev() {
            if authority
                .metadata_optional_relative(&self.log_relative(slot))
                .map_err(|_| DiagnosticError::WriteFailed {
                    reason: DiagnosticFailureReason::StorageUnavailable,
                })?
                .is_some()
            {
                files.push(self.log_path(slot));
            }
        }
        Ok(files)
    }

    pub(crate) fn level(&self) -> Result<DiagnosticLevel, DiagnosticError> {
        let (_guard, _lease, authority) = self.acquire_transaction(DiagnosticTransaction::Write)?;
        self.level_unlocked(&authority)
    }

    fn level_unlocked(&self, authority: &SourceRoot) -> Result<DiagnosticLevel, DiagnosticError> {
        const MAX_SETTINGS_BYTES: u64 = 64 * 1_024;
        let Some(mut file) = authority
            .open_optional_file_relative(Path::new("settings.json"))
            .map_err(|_| DiagnosticError::CorruptSettings {
                reason: DiagnosticFailureReason::SettingsUnavailable,
            })?
        else {
            return Ok(DiagnosticLevel::Standard);
        };
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_SETTINGS_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| DiagnosticError::CorruptSettings {
                reason: DiagnosticFailureReason::SettingsUnavailable,
            })?;
        if bytes.len() as u64 > MAX_SETTINGS_BYTES {
            return Err(DiagnosticError::CorruptSettings {
                reason: DiagnosticFailureReason::SettingsInvalid,
            });
        }
        let stored: StoredDiagnosticSettings =
            serde_json::from_slice(&bytes).map_err(|_| DiagnosticError::CorruptSettings {
                reason: DiagnosticFailureReason::SettingsInvalid,
            })?;
        if stored.version != DIAGNOSTIC_SETTINGS_VERSION {
            return Err(DiagnosticError::CorruptSettings {
                reason: DiagnosticFailureReason::SettingsInvalid,
            });
        }
        Ok(stored.level)
    }

    pub(crate) fn set_level(&self, level: DiagnosticLevel) -> Result<(), DiagnosticError> {
        let (_guard, _lease, authority) = self.acquire_transaction(DiagnosticTransaction::Write)?;
        let body = serde_json::to_vec_pretty(&StoredDiagnosticSettings {
            version: DIAGNOSTIC_SETTINGS_VERSION,
            level,
        })
        .map_err(|_| DiagnosticError::WriteFailed {
            reason: DiagnosticFailureReason::SettingsUnavailable,
        })?;
        atomic_write_relative(&authority, Path::new("settings.json"), &body).map_err(|_| {
            DiagnosticError::WriteFailed {
                reason: DiagnosticFailureReason::SettingsUnavailable,
            }
        })
    }

    pub(crate) fn append(&self, event: DiagnosticEvent) -> Result<(), DiagnosticError> {
        event.validate()?;
        let (_guard, _lease, authority) = self.acquire_transaction(DiagnosticTransaction::Write)?;
        if !self
            .level_unlocked(&authority)?
            .permits(event.required_level())
        {
            return Ok(());
        }
        let record = DiagnosticRecord {
            schema: DIAGNOSTIC_RECORD_SCHEMA.to_string(),
            schema_version: DIAGNOSTIC_RECORD_VERSION,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            app: "MDReader".to_string(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            platform: std::env::consts::OS.to_string(),
            event,
        };
        let mut line =
            serde_json::to_string(&record).map_err(|_| DiagnosticError::WriteFailed {
                reason: DiagnosticFailureReason::EventInvalid,
            })?;
        line.push('\n');
        if line.len() > MAX_DIAGNOSTIC_RECORD_BYTES || line.len() > self.max_log_bytes {
            return Err(DiagnosticError::InvalidEvent {
                reason: DiagnosticFailureReason::EventInvalid,
            });
        }

        let active = self.log_relative(0);
        let current_bytes = authority
            .metadata_optional_relative(&active)
            .map_err(|_| DiagnosticError::WriteFailed {
                reason: DiagnosticFailureReason::StorageUnavailable,
            })?
            .map_or(0, |metadata| metadata.len() as usize);
        if current_bytes.saturating_add(line.len()) > self.max_log_bytes {
            self.rotate(&authority)?;
        }
        let mut file =
            authority
                .open_append_relative(&active)
                .map_err(|_| DiagnosticError::WriteFailed {
                    reason: DiagnosticFailureReason::StorageUnavailable,
                })?;
        file.write_all(line.as_bytes())
            .and_then(|()| file.flush())
            .map_err(|_| DiagnosticError::WriteFailed {
                reason: DiagnosticFailureReason::StorageUnavailable,
            })?;
        authority
            .verify()
            .map_err(|_| DiagnosticError::WriteFailed {
                reason: DiagnosticFailureReason::AuthorityChanged,
            })
    }

    fn rotate(&self, authority: &SourceRoot) -> Result<(), DiagnosticError> {
        let mut present = Vec::with_capacity(self.retention);
        for slot in 0..self.retention {
            present.push(
                authority
                    .metadata_optional_relative(&self.log_relative(slot))
                    .map_err(|_| DiagnosticError::WriteFailed {
                        reason: DiagnosticFailureReason::StorageUnavailable,
                    })?,
            );
        }

        let oldest = self.log_relative(self.retention - 1);
        if present[self.retention - 1].is_some() {
            authority
                .remove_file_relative(&oldest)
                .map_err(|_| DiagnosticError::WriteFailed {
                    reason: DiagnosticFailureReason::StorageUnavailable,
                })?;
        }
        for slot in (1..self.retention).rev() {
            let previous = self.log_relative(slot - 1);
            let Some(metadata) = present[slot - 1].as_ref() else {
                continue;
            };
            if metadata.len() > self.max_log_bytes as u64 {
                authority.remove_file_relative(&previous).map_err(|_| {
                    DiagnosticError::WriteFailed {
                        reason: DiagnosticFailureReason::StorageUnavailable,
                    }
                })?;
            } else {
                authority
                    .rename_relative(&previous, &self.log_relative(slot))
                    .map_err(|_| DiagnosticError::WriteFailed {
                        reason: DiagnosticFailureReason::StorageUnavailable,
                    })?;
            }
        }
        authority
            .verify()
            .map_err(|_| DiagnosticError::WriteFailed {
                reason: DiagnosticFailureReason::AuthorityChanged,
            })
    }

    pub(crate) fn export_to(&self, destination: &Path) -> Result<(), DiagnosticError> {
        let (_guard, _lease, authority) =
            self.acquire_transaction(DiagnosticTransaction::Export)?;
        const HEADER: &[u8] = b"MDReader local diagnostics bundle v1\n\
            Records are reconstructed from the closed diagnostic schema; invalid input is omitted.\n\n";
        const SUMMARY_RESERVE: usize = 96;
        if HEADER.len().saturating_add(SUMMARY_RESERVE) > self.max_export_bytes {
            return Err(DiagnosticError::ExportFailed {
                reason: DiagnosticFailureReason::DestinationUnavailable,
            });
        }
        let mut canonical_records = Vec::new();
        let mut omitted_records = 0_u64;
        let mut omitted_files = 0_u64;
        for slot in (0..self.retention).rev() {
            let relative = self.log_relative(slot);
            let Some(metadata) = authority
                .metadata_optional_relative(&relative)
                .map_err(|_| DiagnosticError::ExportFailed {
                    reason: DiagnosticFailureReason::StorageUnavailable,
                })?
            else {
                continue;
            };
            if metadata.len() > self.max_log_bytes as u64 {
                omitted_files = omitted_files.saturating_add(1);
                continue;
            }
            let Some(mut file) =
                authority
                    .open_optional_file_relative(&relative)
                    .map_err(|_| DiagnosticError::ExportFailed {
                        reason: DiagnosticFailureReason::StorageUnavailable,
                    })?
            else {
                continue;
            };
            let mut bytes = Vec::with_capacity(metadata.len() as usize);
            Read::by_ref(&mut file)
                .take(self.max_log_bytes as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| DiagnosticError::ExportFailed {
                    reason: DiagnosticFailureReason::StorageUnavailable,
                })?;
            if bytes.len() > self.max_log_bytes {
                omitted_files = omitted_files.saturating_add(1);
                continue;
            }
            let mut line_start = 0;
            for line_end in bytes
                .iter()
                .enumerate()
                .filter_map(|(index, byte)| (*byte == b'\n').then_some(index))
            {
                let mut line = &bytes[line_start..line_end];
                line_start = line_end + 1;
                if line.last() == Some(&b'\r') {
                    line = &line[..line.len() - 1];
                }
                let record = (line.len() <= MAX_DIAGNOSTIC_RECORD_BYTES)
                    .then(|| parse_diagnostic_record(line))
                    .transpose();
                let Ok(Some(record)) = record else {
                    omitted_records = omitted_records.saturating_add(1);
                    continue;
                };
                let mut canonical =
                    serde_json::to_vec(&record).map_err(|_| DiagnosticError::ExportFailed {
                        reason: DiagnosticFailureReason::EventInvalid,
                    })?;
                canonical.push(b'\n');
                if canonical.len() > MAX_DIAGNOSTIC_RECORD_BYTES {
                    omitted_records = omitted_records.saturating_add(1);
                    continue;
                }
                canonical_records.push(canonical);
            }
            if line_start != bytes.len() {
                omitted_records = omitted_records.saturating_add(1);
            }
        }
        let mut bundle = HEADER.to_vec();
        for canonical in canonical_records {
            if bundle
                .len()
                .saturating_add(canonical.len())
                .saturating_add(SUMMARY_RESERVE)
                > self.max_export_bytes
            {
                omitted_records = omitted_records.saturating_add(1);
                continue;
            }
            bundle.extend_from_slice(&canonical);
        }
        let summary = format!(
            "\nOmitted records: {omitted_records}\n\
             Omitted oversized files: {omitted_files}\n"
        );
        bundle.extend_from_slice(summary.as_bytes());
        authority
            .verify()
            .map_err(|_| DiagnosticError::ExportFailed {
                reason: DiagnosticFailureReason::AuthorityChanged,
            })?;
        atomic_write(destination, &bundle).map_err(|_| DiagnosticError::ExportFailed {
            reason: DiagnosticFailureReason::DestinationUnavailable,
        })
    }

    pub(crate) fn clear(&self, confirmed: bool) -> Result<DiagnosticLevel, DiagnosticError> {
        if !confirmed {
            return Err(DiagnosticError::ConfirmationRequired);
        }
        let (_guard, _lease, authority) = self.acquire_transaction(DiagnosticTransaction::Clear)?;
        let level = self.level_unlocked(&authority)?;
        let mut present = Vec::with_capacity(self.retention);
        for slot in 0..self.retention {
            present.push(
                authority
                    .metadata_optional_relative(&self.log_relative(slot))
                    .map_err(|_| DiagnosticError::ClearFailed {
                        reason: DiagnosticFailureReason::StorageUnavailable,
                    })?
                    .is_some(),
            );
        }
        for (slot, exists) in present.into_iter().enumerate() {
            if exists {
                authority
                    .remove_file_relative(&self.log_relative(slot))
                    .map_err(|_| DiagnosticError::ClearFailed {
                        reason: DiagnosticFailureReason::StorageUnavailable,
                    })?;
            }
        }
        authority
            .verify()
            .map_err(|_| DiagnosticError::ClearFailed {
                reason: DiagnosticFailureReason::AuthorityChanged,
            })?;
        Ok(level)
    }
}

fn initialize_directory_authority(directory: &Path) -> Result<SourceRoot, DiagnosticFailureReason> {
    let mut cursor = directory;
    let mut missing = Vec::<OsString>::new();
    let mut authority = loop {
        match fs::symlink_metadata(cursor) {
            Ok(_) => {
                break SourceRoot::capture(cursor.to_path_buf())
                    .map_err(|_| DiagnosticFailureReason::AuthorityUnavailable)?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let name = cursor
                    .file_name()
                    .ok_or(DiagnosticFailureReason::AuthorityUnavailable)?;
                missing.push(name.to_os_string());
                cursor = cursor
                    .parent()
                    .ok_or(DiagnosticFailureReason::AuthorityUnavailable)?;
            }
            Err(_) => return Err(DiagnosticFailureReason::AuthorityUnavailable),
        }
    };
    for component in missing.iter().rev() {
        authority = authority
            .create_or_capture_directory_relative(Path::new(component))
            .map_err(|_| DiagnosticFailureReason::AuthorityUnavailable)?;
    }
    Ok(authority)
}

/// Best effort: startup may fail before the diagnostics owner exists. The log
/// schema carries only a code; the message goes to stderr and the dialog.
pub(crate) fn record_startup_failure(app: &AppHandle) {
    if let Ok(directory) = diagnostics_directory(app) {
        let _ = DiagnosticLog::new(directory).append(DiagnosticEvent::Error {
            code: DiagnosticErrorCode::StartupFailed,
        });
    }
}

pub(crate) fn diagnostics_directory(app: &AppHandle) -> Result<PathBuf, DiagnosticError> {
    crate::reader_profile::directory(app)
        .map(|directory| directory.join("diagnostics"))
        .map_err(|_| DiagnosticError::WriteFailed {
            reason: DiagnosticFailureReason::AuthorityUnavailable,
        })
}

fn diagnostic_info(log: &DiagnosticLog) -> Result<DiagnosticInfo, DiagnosticError> {
    Ok(diagnostic_info_with_level(log, log.level()?))
}

fn diagnostic_info_with_level(_log: &DiagnosticLog, level: DiagnosticLevel) -> DiagnosticInfo {
    DiagnosticInfo {
        level,
        max_file_bytes: MAX_LOG_BYTES as u32,
        max_retained_files: LOG_RETENTION as u32,
        max_export_bytes: MAX_EXPORT_BYTES as u32,
    }
}

/// Return the local diagnostic policy and bounds without exposing filesystem
/// authority.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn diagnostics_info(
    state: tauri::State<'_, DiagnosticsState>,
) -> Result<DiagnosticInfo, DiagnosticError> {
    state.info()
}

/// Persist the local verbosity level; no telemetry or upload side effect exists.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn set_diagnostics_level(
    level: DiagnosticLevel,
    state: tauri::State<'_, DiagnosticsState>,
) -> Result<DiagnosticInfo, DiagnosticError> {
    state.set_level(level)
}

/// Append one closed-vocabulary lifecycle event. Product call sites issue this
/// fire-and-forget and deliberately ignore a failure, so logging cannot gate a
/// workspace switch, read, import, save, or close.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn record_diagnostic_event(
    event: DiagnosticEvent,
    state: tauri::State<'_, DiagnosticsState>,
) -> Result<(), DiagnosticError> {
    state.append(event)
}

/// Open the diagnostics directory using the platform's supported opener API.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn reveal_diagnostics(
    state: tauri::State<'_, DiagnosticsState>,
) -> Result<(), DiagnosticError> {
    state.reveal_with(|directory| {
        tauri_plugin_opener::open_path(directory, None::<&str>).map_err(|_| {
            DiagnosticError::RevealFailed {
                reason: DiagnosticFailureReason::DestinationUnavailable,
            }
        })
    })
}

/// Ask for a local destination and write a schema-validated, bounded bundle.
#[tauri::command]
#[specta::specta]
pub(crate) async fn export_diagnostics(
    app: AppHandle,
    state: tauri::State<'_, DiagnosticsState>,
) -> Result<DiagnosticExport, DiagnosticError> {
    let Some(destination) = app
        .dialog()
        .file()
        .add_filter("Text", &["txt"])
        .set_file_name("mdreader-diagnostics.txt")
        .blocking_save_file()
        .and_then(|path| path.into_path().ok())
    else {
        return Ok(DiagnosticExport {
            status: DiagnosticExportStatus::Cancelled,
            bytes_written: 0,
        });
    };

    state.export_to(&destination)?;
    let bytes_written = fs::metadata(&destination)
        .map_err(|_| DiagnosticError::ExportFailed {
            reason: DiagnosticFailureReason::DestinationUnavailable,
        })?
        .len()
        .min(u64::from(u32::MAX)) as u32;
    Ok(DiagnosticExport {
        status: DiagnosticExportStatus::Exported,
        bytes_written,
    })
}

/// Remove only the three named diagnostic log files. The backend repeats the
/// confirmation check so a UI regression cannot clear them silently.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn clear_diagnostics(
    confirmed: bool,
    state: tauri::State<'_, DiagnosticsState>,
) -> Result<DiagnosticInfo, DiagnosticError> {
    if !confirmed {
        return Err(DiagnosticError::ConfirmationRequired);
    }
    state.clear(confirmed)
}

fn atomic_write_relative(authority: &SourceRoot, path: &Path, body: &[u8]) -> Result<(), String> {
    let name = path
        .file_name()
        .ok_or_else(|| "diagnostic settings path has no file name".to_string())?;
    let temporary = PathBuf::from(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut file = authority
            .create_new_relative(&temporary)
            .map_err(|error| error.to_string())?;
        file.write_all(body)
            .and_then(|()| file.sync_all())
            .map_err(|error| error.to_string())?;
        authority
            .rename_relative(&temporary, path)
            .map_err(|error| error.to_string())?;
        authority.verify().map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = authority.remove_file_relative(&temporary);
    }
    result
}

fn atomic_write(path: &Path, body: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(body)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map(|_| ())
        .map_err(|error| error.error)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::io::Write;
    use std::path::Path;
    use std::sync::{mpsc, Arc};
    use std::thread;
    use std::time::Duration;

    use tempfile::tempdir;

    use crate::file_lock::AdvisoryFileLock;

    use super::{
        diagnostic_info, DiagnosticError, DiagnosticEvent, DiagnosticExport,
        DiagnosticExportStatus, DiagnosticFailureReason, DiagnosticIndexPhase, DiagnosticLevel,
        DiagnosticLog, DiagnosticsState, MAX_EXPORT_BYTES,
    };

    fn assert_sidecar_blocks(
        directory: &Path,
        operation: impl FnOnce() -> Result<(), DiagnosticError> + Send + 'static,
    ) {
        let lease = AdvisoryFileLock::acquire(&directory.join("diagnostics.lock"))
            .expect("hold diagnostics sidecar");
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            started_tx.send(()).expect("signal operation start");
            let result = operation();
            done_tx.send(()).expect("signal operation completion");
            result
        });
        started_rx.recv().expect("operation started");
        assert!(
            done_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "the operation must wait for the process-external sidecar lease"
        );
        drop(lease);
        worker
            .join()
            .expect("join contending operation")
            .expect("operation succeeds after release");
    }

    #[test]
    fn rotation_and_retention_keep_only_the_documented_bounded_files() {
        let temp = tempdir().expect("tempdir");
        let log = DiagnosticLog::with_limits(temp.path(), 512, 3, 1_024);
        for index in 0..40 {
            log.append(DiagnosticEvent::Index {
                source_id: format!("{index:064x}"),
                phase: DiagnosticIndexPhase::Done,
                files_done: index,
                files_total: 40,
                chunks: index * 2,
                error: None,
            })
            .expect("append diagnostic");
        }

        let files = log.log_files().expect("list bounded log files");
        assert_eq!(files.len(), 3);
        assert!(
            files
                .iter()
                .all(|path| fs::metadata(path).expect("log metadata").len() <= 512),
            "every retained file respects the configured byte bound"
        );
        assert!(!temp.path().join("mdreader.log.3").exists());
    }

    #[test]
    fn generic_diagnostic_contracts_and_debug_never_expose_native_or_secret_context() {
        let temp = tempdir().expect("tempdir");
        let sentinel_directory = temp.path().join("token-document-dsn-bound-value");
        fs::create_dir(&sentinel_directory).expect("create diagnostics directory");
        let log = DiagnosticLog::new(&sentinel_directory);
        let log_debug = format!("{log:?}");
        assert!(!log_debug.contains("token-document-dsn-bound-value"));
        assert!(!log_debug.contains(&temp.path().to_string_lossy().into_owned()));

        let info = serde_json::to_value(diagnostic_info(&log).expect("diagnostic info"))
            .expect("serialize info");
        assert!(info.get("directory").is_none());
        assert!(info.get("max_retained_files").is_some());
        assert!(info.get("retained_files").is_none());

        let export = serde_json::to_value(DiagnosticExport {
            status: DiagnosticExportStatus::Exported,
            bytes_written: 12,
        })
        .expect("serialize export");
        assert!(export.get("path").is_none());

        let error = DiagnosticError::WriteFailed {
            reason: DiagnosticFailureReason::StorageUnavailable,
        };
        for surface in [
            format!("{error:?}"),
            error.to_string(),
            serde_json::to_string(&error).expect("serialize error"),
        ] {
            assert!(!surface.contains("postgres://"));
            assert!(!surface.contains("token"));
            assert!(!surface.contains("document"));
            assert!(!surface.contains("bound-value"));
            assert!(!surface.contains("/Users/private"));
        }
    }

    #[test]
    fn rotation_drops_a_hostile_oversized_file_instead_of_retaining_it() {
        let temp = tempdir().expect("tempdir");
        let log = DiagnosticLog::with_limits(temp.path(), 256, 3, 1_024);
        fs::write(temp.path().join("mdreader.log"), vec![b'x'; 1_024])
            .expect("write hostile oversized log");

        log.append(DiagnosticEvent::ApplicationStarted)
            .expect("a bounded fresh record replaces the hostile file");

        let files = log.log_files().expect("list retained logs");
        assert!(!files.is_empty());
        assert!(
            files
                .iter()
                .all(|path| fs::metadata(path).expect("metadata").len() <= 256),
            "even externally modified retained files respect the hard bound"
        );
    }

    #[test]
    fn export_omits_hostile_legacy_bytes_and_remains_hard_bounded() {
        let temp = tempdir().expect("tempdir");
        let logs = temp.path().join("logs");
        fs::create_dir(&logs).expect("create logs");
        let hostile = format!(
            "{{\"event\":\"legacy\",\"token\":\"{}\",\"path\":\"/Users/alice/private/note.md\"}}\n",
            "x".repeat(MAX_EXPORT_BYTES * 2)
        );
        fs::write(logs.join("mdreader.log"), hostile).expect("write hostile log");
        let export = temp.path().join("bundle.txt");
        let log = DiagnosticLog::new(&logs);

        log.export_to(&export).expect("export diagnostics");
        let bytes = fs::read(&export).expect("read export");
        let body = String::from_utf8(bytes.clone()).expect("utf8 bundle");
        assert!(bytes.len() <= MAX_EXPORT_BYTES);
        assert!(!body.contains("token"));
        assert!(!body.contains("/Users/alice"));
        assert!(!body.contains(&"x".repeat(128)));
    }

    #[test]
    fn clear_requires_confirmation_preserves_unrelated_files_and_surfaces_io_failure() {
        let temp = tempdir().expect("tempdir");
        let log = DiagnosticLog::new(temp.path());
        fs::write(temp.path().join("mdreader.log"), "one\n").expect("write log");
        fs::write(temp.path().join("keep.txt"), "keep\n").expect("write unrelated");

        assert_eq!(
            log.clear(false).expect_err("confirmation is mandatory"),
            DiagnosticError::ConfirmationRequired
        );
        assert!(temp.path().join("mdreader.log").exists());
        assert_eq!(
            log.clear(true).expect("confirmed clear"),
            DiagnosticLevel::Standard
        );
        assert!(!temp.path().join("mdreader.log").exists());
        assert!(temp.path().join("keep.txt").exists());

        fs::create_dir(temp.path().join("mdreader.log")).expect("make removal fail");
        assert!(matches!(
            log.clear(true),
            Err(DiagnosticError::ClearFailed { .. })
        ));
        assert!(temp.path().join("mdreader.log").is_dir());
    }

    #[test]
    fn level_persistence_is_versioned_and_corruption_is_not_defaulted() {
        let temp = tempdir().expect("tempdir");
        let log = DiagnosticLog::new(temp.path());
        assert_eq!(
            log.level().expect("default level"),
            DiagnosticLevel::Standard
        );
        log.set_level(DiagnosticLevel::Verbose)
            .expect("persist verbose");
        assert_eq!(
            DiagnosticLog::new(temp.path())
                .level()
                .expect("restore level"),
            DiagnosticLevel::Verbose
        );

        fs::write(temp.path().join("settings.json"), b"{not-json")
            .expect("corrupt diagnostic settings");
        assert!(matches!(
            log.level(),
            Err(DiagnosticError::CorruptSettings { .. })
        ));
    }

    #[test]
    fn independent_diagnostic_handles_contend_for_every_file_transaction_and_keep_bounds() {
        let temp = tempdir().expect("tempdir");
        let directory = temp.path().join("logs");
        fs::create_dir_all(&directory).expect("create diagnostics directory");

        let for_level = directory.clone();
        assert_sidecar_blocks(&directory, move || {
            DiagnosticLog::new(for_level).level().map(|_| ())
        });
        let for_set = directory.clone();
        assert_sidecar_blocks(&directory, move || {
            DiagnosticLog::new(for_set).set_level(DiagnosticLevel::Verbose)
        });
        let for_append = directory.clone();
        assert_sidecar_blocks(&directory, move || {
            DiagnosticLog::new(for_append).append(DiagnosticEvent::ApplicationStarted)
        });
        let for_export = directory.clone();
        let export = temp.path().join("bundle.txt");
        assert_sidecar_blocks(&directory, move || {
            DiagnosticLog::new(for_export).export_to(&export)
        });
        let for_clear = directory.clone();
        assert_sidecar_blocks(&directory, move || {
            DiagnosticLog::new(for_clear).clear(true).map(|_| ())
        });

        let first = Arc::new(DiagnosticLog::with_limits(&directory, 512, 3, 1_024));
        let second = Arc::new(DiagnosticLog::with_limits(&directory, 512, 3, 1_024));
        let mut workers = Vec::new();
        for worker_id in 0..8_u32 {
            let log = if worker_id % 2 == 0 {
                Arc::clone(&first)
            } else {
                Arc::clone(&second)
            };
            workers.push(thread::spawn(move || {
                for index in 0..25_u32 {
                    log.append(DiagnosticEvent::Index {
                        source_id: format!("{worker_id:064x}"),
                        phase: DiagnosticIndexPhase::Done,
                        files_done: index,
                        files_total: 25,
                        chunks: index,
                        error: None,
                    })?;
                }
                Ok::<(), DiagnosticError>(())
            }));
        }
        for worker in workers {
            worker
                .join()
                .expect("join diagnostic writer")
                .expect("diagnostic writer succeeds");
        }
        let files = first.log_files().expect("list bounded files");
        assert!(files.len() <= 3);
        assert!(files
            .iter()
            .all(|path| { fs::metadata(path).expect("diagnostic metadata").len() <= 512 }));
    }

    #[test]
    fn diagnostic_lock_failure_is_closed_before_any_file_mutation() {
        let temp = tempdir().expect("tempdir");
        fs::create_dir(temp.path().join("diagnostics.lock"))
            .expect("occupy lock sidecar with directory");
        let log = DiagnosticLog::new(temp.path());

        assert!(matches!(
            log.append(DiagnosticEvent::ApplicationStarted),
            Err(DiagnosticError::WriteFailed { .. })
        ));
        assert!(!temp.path().join("mdreader.log").exists());
    }

    #[cfg(unix)]
    #[test]
    fn substituted_named_logs_are_refused_without_following_or_removing_the_link() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let logs = temp.path().join("logs");
        fs::create_dir(&logs).expect("create logs");
        let outside = temp.path().join("outside.txt");
        fs::write(&outside, b"outside stays").expect("write outside marker");
        let active = logs.join("mdreader.log");
        symlink(&outside, &active).expect("substitute active log");
        let log = DiagnosticLog::with_limits(&logs, 256, 3, 1_024);

        assert!(matches!(
            log.append(DiagnosticEvent::ApplicationStarted),
            Err(DiagnosticError::WriteFailed { .. })
        ));
        assert!(active
            .symlink_metadata()
            .expect("link remains")
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read(&outside).expect("read outside after append"),
            b"outside stays"
        );

        let export = temp.path().join("bundle.txt");
        assert!(matches!(
            log.export_to(&export),
            Err(DiagnosticError::ExportFailed { .. })
        ));
        assert!(!export.exists(), "a refused export publishes no bundle");
        assert_eq!(
            fs::read(&outside).expect("read outside after export"),
            b"outside stays"
        );

        assert!(matches!(
            log.clear(true),
            Err(DiagnosticError::ClearFailed { .. })
        ));
        assert!(active
            .symlink_metadata()
            .expect("link remains")
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read(&outside).expect("read outside after clear"),
            b"outside stays"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rotation_refuses_a_substituted_retained_slot_before_mutating_any_log() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let logs = temp.path().join("logs");
        fs::create_dir(&logs).expect("create logs");
        let active = logs.join("mdreader.log");
        fs::write(&active, vec![b'a'; 250]).expect("write nearly-full active log");
        let outside = temp.path().join("outside.txt");
        fs::write(&outside, b"outside stays").expect("write outside marker");
        let retained = logs.join("mdreader.log.1");
        symlink(&outside, &retained).expect("substitute retained log");
        let before = fs::read(&active).expect("read active before rotation");
        let log = DiagnosticLog::with_limits(&logs, 256, 3, 1_024);

        assert!(matches!(
            log.append(DiagnosticEvent::ApplicationStarted),
            Err(DiagnosticError::WriteFailed { .. })
        ));
        assert_eq!(fs::read(&active).expect("active remains"), before);
        assert!(retained
            .symlink_metadata()
            .expect("retained link remains")
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read(&outside).expect("outside remains"),
            b"outside stays"
        );
    }

    #[test]
    fn corrupt_settings_reject_clear_before_any_log_bytes_change() {
        let temp = tempdir().expect("tempdir");
        let log = DiagnosticLog::new(temp.path());
        let active = temp.path().join("mdreader.log");
        let retained = temp.path().join("mdreader.log.1");
        fs::write(&active, b"active bytes").expect("write active");
        fs::write(&retained, b"retained bytes").expect("write retained");
        fs::write(temp.path().join("settings.json"), b"{not-json").expect("write corrupt settings");

        assert!(matches!(
            log.clear(true),
            Err(DiagnosticError::CorruptSettings { .. })
        ));
        assert_eq!(fs::read(active).expect("active remains"), b"active bytes");
        assert_eq!(
            fs::read(retained).expect("retained remains"),
            b"retained bytes"
        );
    }

    #[test]
    fn successful_clear_returns_the_validated_level_without_post_clear_io() {
        let temp = tempdir().expect("tempdir");
        let log = DiagnosticLog::new(temp.path());
        log.set_level(DiagnosticLevel::Verbose)
            .expect("persist verbose");
        log.append(DiagnosticEvent::ApplicationStarted)
            .expect("append active log");

        let level = log.clear(true).expect("clear succeeds");

        assert_eq!(level, DiagnosticLevel::Verbose);
        assert!(!temp.path().join("mdreader.log").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_substituted_diagnostics_directory_is_not_treated_as_local_authority() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).expect("create outside directory");
        let logs = temp.path().join("logs");
        symlink(&outside, &logs).expect("substitute diagnostics directory");

        assert!(matches!(
            DiagnosticLog::new(&logs).append(DiagnosticEvent::ApplicationStarted),
            Err(DiagnosticError::WriteFailed { .. })
        ));
        assert!(
            !outside.join("mdreader.log").exists(),
            "a substituted directory receives no diagnostic bytes"
        );
    }

    #[test]
    fn a_real_directory_replacement_after_capture_is_refused_for_every_operation() {
        let temp = tempdir().expect("tempdir");
        let logs = temp.path().join("logs");
        fs::create_dir(&logs).expect("create admitted diagnostics directory");
        let log = DiagnosticLog::new(&logs);
        log.append(DiagnosticEvent::ApplicationStarted)
            .expect("capture diagnostics authority");
        let admitted = temp.path().join("admitted");
        fs::rename(&logs, &admitted).expect("retire admitted directory");
        fs::create_dir(&logs).expect("install replacement directory");
        let export = temp.path().join("bundle.txt");

        assert!(matches!(
            log.append(DiagnosticEvent::ApplicationStarted),
            Err(DiagnosticError::WriteFailed { .. })
        ));
        assert!(matches!(
            log.level(),
            Err(DiagnosticError::WriteFailed { .. })
        ));
        assert!(matches!(
            log.export_to(&export),
            Err(DiagnosticError::ExportFailed { .. })
        ));
        assert!(matches!(
            log.clear(true),
            Err(DiagnosticError::ClearFailed { .. })
        ));
        assert!(
            !logs.join("mdreader.log").exists(),
            "the replacement directory receives no diagnostic bytes"
        );
        assert!(
            !export.exists(),
            "a refused export publishes no support bundle"
        );
    }

    #[test]
    fn a_real_directory_replacement_before_first_operation_is_refused() {
        let temp = tempdir().expect("tempdir");
        let logs = temp.path().join("logs");
        fs::create_dir(&logs).expect("create admitted diagnostics directory");
        let log = DiagnosticLog::new(&logs);
        fs::rename(&logs, temp.path().join("admitted"))
            .expect("retire admitted directory before first operation");
        fs::create_dir(&logs).expect("install replacement directory");

        assert!(matches!(
            log.append(DiagnosticEvent::ApplicationStarted),
            Err(DiagnosticError::WriteFailed { .. })
        ));
        assert!(
            !logs.join("mdreader.log").exists(),
            "the replacement directory receives no diagnostic bytes"
        );
    }

    #[test]
    fn an_absent_nested_diagnostics_directory_is_initialized_once() {
        let temp = tempdir().expect("tempdir");
        let logs = temp.path().join("app-data").join("diagnostics");
        let state = DiagnosticsState::new(logs.clone());

        state
            .append(DiagnosticEvent::ApplicationStarted)
            .expect("initialize and append diagnostics");

        assert!(logs.is_dir());
        assert!(logs.join("mdreader.log").is_file());
    }

    #[test]
    fn process_state_shares_one_directory_authority_across_command_operations() {
        let temp = tempdir().expect("tempdir");
        let logs = temp.path().join("logs");
        fs::create_dir(&logs).expect("create admitted diagnostics directory");
        let state = DiagnosticsState::new(logs.clone());
        state
            .append(DiagnosticEvent::ApplicationStarted)
            .expect("first command captures process authority");
        fs::rename(&logs, temp.path().join("admitted")).expect("retire admitted directory");
        fs::create_dir(&logs).expect("install replacement directory");
        let export = temp.path().join("bundle.txt");

        assert!(matches!(
            state.info(),
            Err(DiagnosticError::WriteFailed { .. })
        ));
        assert!(matches!(
            state.set_level(DiagnosticLevel::Verbose),
            Err(DiagnosticError::WriteFailed { .. })
        ));
        assert!(matches!(
            state.append(DiagnosticEvent::ApplicationStarted),
            Err(DiagnosticError::WriteFailed { .. })
        ));
        assert!(matches!(
            state.export_to(&export),
            Err(DiagnosticError::ExportFailed { .. })
        ));
        assert!(matches!(
            state.clear(true),
            Err(DiagnosticError::ClearFailed { .. })
        ));
        let reveal_called = Cell::new(false);
        assert!(matches!(
            state.reveal_with(|_| {
                reveal_called.set(true);
                Ok(())
            }),
            Err(DiagnosticError::RevealFailed { .. })
        ));
        assert!(
            !reveal_called.get(),
            "the platform opener is not called for a replaced directory"
        );
        assert!(
            !logs.join("mdreader.log").exists(),
            "no command recaptures the replacement directory"
        );
    }

    #[test]
    fn export_rebuilds_only_valid_structured_records_and_counts_omissions() {
        let temp = tempdir().expect("tempdir");
        let logs = temp.path().join("logs");
        fs::create_dir(&logs).expect("create logs");
        let log = DiagnosticLog::with_limits(&logs, 32 * 1_024, 3, 64 * 1_024);
        log.append(DiagnosticEvent::ApplicationStarted)
            .expect("append one valid structured record");
        let mut active = fs::OpenOptions::new()
            .append(true)
            .open(logs.join("mdreader.log"))
            .expect("open active log for hostile legacy input");
        writeln!(
            active,
            "The blue lantern waits beneath the cedar after the private meeting."
        )
        .expect("append arbitrary private sentence");
        writeln!(
            active,
            r#"{{"schema":"mdreader.diagnostics","schema_version":1,"timestamp_ms":7,"app":"MDReader","app_version":"{}","platform":"{}","event":{{"kind":"application-started","document":"Acquisition terms for Northstar"}}}}"#,
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS
        )
        .expect("append valid-looking record with arbitrary content");
        writeln!(
            active,
            r#"{{"schema":"other.diagnostics","schema_version":1,"timestamp_ms":8,"app":"MDReader","app_version":"{}","platform":"{}","event":{{"kind":"application-started"}}}}"#,
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS
        )
        .expect("append unsupported schema");
        writeln!(
            active,
            r#"{{"schema":"mdreader.diagnostics","schema_version":2,"timestamp_ms":9,"app":"MDReader","app_version":"{}","platform":"{}","event":{{"kind":"application-started"}}}}"#,
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS
        )
        .expect("append unsupported schema version");
        writeln!(
            active,
            r#"{{"schema":"mdreader.diagnostics","schema_version":1,"timestamp_ms":10,"app":"MDReader","app_version":"blue-lantern-private-build","platform":"{}","event":{{"kind":"application-started"}}}}"#,
            std::env::consts::OS
        )
        .expect("append unsupported application version");
        writeln!(active, "{{\"timestamp_ms\":").expect("append malformed json");
        writeln!(active, "{}", "z".repeat(5_000)).expect("append oversized line");
        write!(active, "{{\"timestamp_ms\":42").expect("append truncated line");
        active.flush().expect("flush hostile input");
        let export = temp.path().join("bundle.txt");

        log.export_to(&export).expect("export diagnostics");

        let body = fs::read_to_string(export).expect("read structured export");
        for forbidden in [
            "blue lantern",
            "private meeting",
            "blue-lantern-private-build",
            "Acquisition terms",
            "Northstar",
            &"z".repeat(128),
            "{\"timestamp_ms\":42",
        ] {
            assert!(
                !body.contains(forbidden),
                "export emitted non-allowlisted input: {forbidden:?}"
            );
        }
        let records: Vec<_> = body.lines().filter(|line| line.starts_with('{')).collect();
        assert_eq!(
            records.len(),
            1,
            "only the valid application-started record is exported"
        );
        let record: serde_json::Value =
            serde_json::from_str(records[0]).expect("canonical record is valid json");
        assert_eq!(record["event"]["kind"], "application-started");
        assert!(
            body.contains("Omitted records: 8"),
            "every invalid, oversized, or truncated input is accounted for"
        );
    }
}
