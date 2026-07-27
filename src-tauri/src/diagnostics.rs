//! Small, local-only diagnostic logging.
//!
//! The event vocabulary is intentionally closed: callers can report lifecycle
//! phases, numeric counts, opaque source digests, and typed error codes, but
//! there is no field for document text, query text, credentials, tokens, or a
//! filesystem path. Export applies a second defensive redaction pass so even a
//! legacy/corrupt line cannot leak those values into a support bundle.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

const DIAGNOSTIC_SETTINGS_VERSION: u32 = 1;
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
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
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
                message: "diagnostic source identity must be a lowercase BLAKE3 digest".to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum DiagnosticError {
    ConfirmationRequired,
    CorruptSettings { message: String },
    InvalidEvent { message: String },
    WriteFailed { message: String },
    ExportFailed { message: String },
    ClearFailed { message: String },
    RevealFailed { message: String },
}

impl std::fmt::Display for DiagnosticError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConfirmationRequired => {
                formatter.write_str("clearing diagnostics requires explicit confirmation")
            }
            Self::CorruptSettings { message }
            | Self::InvalidEvent { message }
            | Self::WriteFailed { message }
            | Self::ExportFailed { message }
            | Self::ClearFailed { message }
            | Self::RevealFailed { message } => formatter.write_str(message),
        }
    }
}

impl std::error::Error for DiagnosticError {}

#[derive(Debug, Serialize)]
struct DiagnosticRecord<'a> {
    timestamp_ms: u64,
    app: &'static str,
    version: &'static str,
    platform: &'static str,
    event: &'a DiagnosticEvent,
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
    pub directory: String,
    pub max_file_bytes: u32,
    pub retained_files: u32,
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
    pub path: Option<String>,
    pub bytes_written: u32,
}

#[derive(Default)]
pub(crate) struct DiagnosticsState(Mutex<()>);

#[derive(Debug)]
pub(crate) struct DiagnosticLog {
    directory: PathBuf,
    max_log_bytes: usize,
    retention: usize,
    max_export_bytes: usize,
    lock: Mutex<()>,
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
        Self {
            directory: directory.as_ref().to_path_buf(),
            max_log_bytes: max_log_bytes.max(1),
            retention: retention.max(1),
            max_export_bytes: max_export_bytes.max(1),
            lock: Mutex::new(()),
        }
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    fn settings_path(&self) -> PathBuf {
        self.directory.join("settings.json")
    }

    fn log_path(&self, slot: usize) -> PathBuf {
        if slot == 0 {
            self.directory.join("mdreader.log")
        } else {
            self.directory.join(format!("mdreader.log.{slot}"))
        }
    }

    #[cfg(test)]
    pub(crate) fn log_files(&self) -> Result<Vec<PathBuf>, DiagnosticError> {
        let _guard = self.lock.lock().map_err(|_| DiagnosticError::WriteFailed {
            message: "diagnostic log lock is poisoned".to_string(),
        })?;
        Ok((0..self.retention)
            .rev()
            .map(|slot| self.log_path(slot))
            .filter(|path| path.is_file())
            .collect())
    }

    pub(crate) fn level(&self) -> Result<DiagnosticLevel, DiagnosticError> {
        let path = self.settings_path();
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(DiagnosticLevel::Standard)
            }
            Err(error) => {
                return Err(DiagnosticError::CorruptSettings {
                    message: format!("cannot read diagnostic settings: {error}"),
                })
            }
        };
        let stored: StoredDiagnosticSettings =
            serde_json::from_slice(&bytes).map_err(|error| DiagnosticError::CorruptSettings {
                message: format!("cannot parse diagnostic settings: {error}"),
            })?;
        if stored.version != DIAGNOSTIC_SETTINGS_VERSION {
            return Err(DiagnosticError::CorruptSettings {
                message: format!(
                    "unsupported diagnostic settings version {}; expected {}",
                    stored.version, DIAGNOSTIC_SETTINGS_VERSION
                ),
            });
        }
        Ok(stored.level)
    }

    pub(crate) fn set_level(&self, level: DiagnosticLevel) -> Result<(), DiagnosticError> {
        let _guard = self.lock.lock().map_err(|_| DiagnosticError::WriteFailed {
            message: "diagnostic log lock is poisoned".to_string(),
        })?;
        fs::create_dir_all(&self.directory).map_err(|error| DiagnosticError::WriteFailed {
            message: format!("cannot create diagnostic directory: {error}"),
        })?;
        let body = serde_json::to_vec_pretty(&StoredDiagnosticSettings {
            version: DIAGNOSTIC_SETTINGS_VERSION,
            level,
        })
        .map_err(|error| DiagnosticError::WriteFailed {
            message: format!("cannot encode diagnostic settings: {error}"),
        })?;
        atomic_write(&self.settings_path(), &body).map_err(|error| DiagnosticError::WriteFailed {
            message: format!("cannot persist diagnostic settings: {error}"),
        })
    }

    pub(crate) fn append(&self, event: DiagnosticEvent) -> Result<(), DiagnosticError> {
        event.validate()?;
        if !self.level()?.permits(event.required_level()) {
            return Ok(());
        }
        let _guard = self.lock.lock().map_err(|_| DiagnosticError::WriteFailed {
            message: "diagnostic log lock is poisoned".to_string(),
        })?;
        fs::create_dir_all(&self.directory).map_err(|error| DiagnosticError::WriteFailed {
            message: format!("cannot create diagnostic directory: {error}"),
        })?;
        let record = DiagnosticRecord {
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            app: "MDReader",
            version: env!("CARGO_PKG_VERSION"),
            platform: std::env::consts::OS,
            event: &event,
        };
        let mut line =
            serde_json::to_string(&record).map_err(|error| DiagnosticError::WriteFailed {
                message: format!("cannot encode diagnostic event: {error}"),
            })?;
        line.push('\n');
        if line.len() > self.max_log_bytes {
            return Err(DiagnosticError::InvalidEvent {
                message: "diagnostic event exceeds the local log bound".to_string(),
            });
        }

        let active = self.log_path(0);
        let current_bytes = fs::metadata(&active).map_or(0, |meta| meta.len() as usize);
        if current_bytes.saturating_add(line.len()) > self.max_log_bytes {
            self.rotate()?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(active)
            .map_err(|error| DiagnosticError::WriteFailed {
                message: format!("cannot open diagnostic log: {error}"),
            })?;
        file.write_all(line.as_bytes())
            .and_then(|()| file.flush())
            .map_err(|error| DiagnosticError::WriteFailed {
                message: format!("cannot append diagnostic log: {error}"),
            })
    }

    fn rotate(&self) -> Result<(), DiagnosticError> {
        let oldest = self.log_path(self.retention - 1);
        if oldest.exists() {
            fs::remove_file(&oldest).map_err(|error| DiagnosticError::WriteFailed {
                message: format!("cannot retire oldest diagnostic log: {error}"),
            })?;
        }
        for slot in (1..self.retention).rev() {
            let previous = self.log_path(slot - 1);
            let metadata = match fs::metadata(&previous) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(DiagnosticError::WriteFailed {
                        message: format!("cannot inspect diagnostic log during rotation: {error}"),
                    })
                }
            };
            if metadata.len() > self.max_log_bytes as u64 {
                fs::remove_file(previous).map_err(|error| DiagnosticError::WriteFailed {
                    message: format!("cannot discard oversized diagnostic log: {error}"),
                })?;
            } else {
                fs::rename(previous, self.log_path(slot)).map_err(|error| {
                    DiagnosticError::WriteFailed {
                        message: format!("cannot rotate diagnostic log: {error}"),
                    }
                })?;
            }
        }
        Ok(())
    }

    pub(crate) fn export_to(&self, destination: &Path) -> Result<(), DiagnosticError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| DiagnosticError::ExportFailed {
                message: "diagnostic log lock is poisoned".to_string(),
            })?;
        let mut bundle =
            b"MDReader local diagnostics bundle v1\nNo document or query contents are collected.\n\n"
                .to_vec();
        for slot in (0..self.retention).rev() {
            let path = self.log_path(slot);
            let mut file = match fs::File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(DiagnosticError::ExportFailed {
                        message: format!("cannot read diagnostic log for export: {error}"),
                    })
                }
            };
            let remaining = self.max_export_bytes.saturating_sub(bundle.len());
            if remaining == 0 {
                break;
            }
            let mut chunk = Vec::new();
            Read::by_ref(&mut file)
                .take(remaining as u64)
                .read_to_end(&mut chunk)
                .map_err(|error| DiagnosticError::ExportFailed {
                    message: format!("cannot read diagnostic export: {error}"),
                })?;
            let redacted = redact_text(&String::from_utf8_lossy(&chunk));
            let remaining = self.max_export_bytes.saturating_sub(bundle.len());
            bundle.extend_from_slice(
                redacted
                    .as_bytes()
                    .get(..remaining.min(redacted.len()))
                    .unwrap_or_default(),
            );
        }
        atomic_write(destination, &bundle).map_err(|error| DiagnosticError::ExportFailed {
            message: format!("cannot write diagnostic export: {error}"),
        })
    }

    pub(crate) fn clear(&self, confirmed: bool) -> Result<(), DiagnosticError> {
        if !confirmed {
            return Err(DiagnosticError::ConfirmationRequired);
        }
        let _guard = self.lock.lock().map_err(|_| DiagnosticError::ClearFailed {
            message: "diagnostic log lock is poisoned".to_string(),
        })?;
        for slot in 0..self.retention {
            let path = self.log_path(slot);
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(DiagnosticError::ClearFailed {
                        message: format!("cannot clear {}: {error}", path.display()),
                    })
                }
            }
        }
        Ok(())
    }
}

fn diagnostics_directory(app: &AppHandle) -> Result<PathBuf, DiagnosticError> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join("diagnostics"))
        .map_err(|error| DiagnosticError::WriteFailed {
            message: format!("cannot resolve diagnostic directory: {error}"),
        })
}

fn lock_diagnostics(
    state: &DiagnosticsState,
) -> Result<std::sync::MutexGuard<'_, ()>, DiagnosticError> {
    state.0.lock().map_err(|_| DiagnosticError::WriteFailed {
        message: "diagnostic state lock is poisoned".to_string(),
    })
}

fn diagnostic_info(log: &DiagnosticLog) -> Result<DiagnosticInfo, DiagnosticError> {
    Ok(DiagnosticInfo {
        level: log.level()?,
        directory: log.directory().to_string_lossy().into_owned(),
        max_file_bytes: MAX_LOG_BYTES as u32,
        retained_files: LOG_RETENTION as u32,
        max_export_bytes: MAX_EXPORT_BYTES as u32,
    })
}

/// Return the exact local log location and documented bounds.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn diagnostics_info(
    app: AppHandle,
    state: tauri::State<'_, DiagnosticsState>,
) -> Result<DiagnosticInfo, DiagnosticError> {
    let _guard = lock_diagnostics(&state)?;
    diagnostic_info(&DiagnosticLog::new(diagnostics_directory(&app)?))
}

/// Persist the local verbosity level; no telemetry or upload side effect exists.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn set_diagnostics_level(
    app: AppHandle,
    level: DiagnosticLevel,
    state: tauri::State<'_, DiagnosticsState>,
) -> Result<DiagnosticInfo, DiagnosticError> {
    let _guard = lock_diagnostics(&state)?;
    let log = DiagnosticLog::new(diagnostics_directory(&app)?);
    log.set_level(level)?;
    diagnostic_info(&log)
}

/// Append one closed-vocabulary lifecycle event. Product call sites issue this
/// fire-and-forget and deliberately ignore a failure, so logging cannot gate a
/// workspace switch, read, import, save, or close.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn record_diagnostic_event(
    app: AppHandle,
    event: DiagnosticEvent,
    state: tauri::State<'_, DiagnosticsState>,
) -> Result<(), DiagnosticError> {
    let _guard = lock_diagnostics(&state)?;
    DiagnosticLog::new(diagnostics_directory(&app)?).append(event)
}

/// Open the diagnostics directory using the platform's supported opener API.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn reveal_diagnostics(
    app: AppHandle,
    state: tauri::State<'_, DiagnosticsState>,
) -> Result<(), DiagnosticError> {
    let _guard = lock_diagnostics(&state)?;
    let directory = diagnostics_directory(&app)?;
    fs::create_dir_all(&directory).map_err(|error| DiagnosticError::RevealFailed {
        message: format!("cannot create diagnostic directory: {error}"),
    })?;
    tauri_plugin_opener::open_path(&directory, None::<&str>).map_err(|error| {
        DiagnosticError::RevealFailed {
            message: format!("cannot reveal diagnostic directory: {error}"),
        }
    })
}

/// Ask for a local destination and write a redacted, hard-bounded text bundle.
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
            path: None,
            bytes_written: 0,
        });
    };

    let _guard = lock_diagnostics(&state)?;
    DiagnosticLog::new(diagnostics_directory(&app)?).export_to(&destination)?;
    let bytes_written = fs::metadata(&destination)
        .map_err(|error| DiagnosticError::ExportFailed {
            message: format!("cannot inspect diagnostic export: {error}"),
        })?
        .len()
        .min(u64::from(u32::MAX)) as u32;
    Ok(DiagnosticExport {
        status: DiagnosticExportStatus::Exported,
        path: Some(destination.to_string_lossy().into_owned()),
        bytes_written,
    })
}

/// Remove only the three named diagnostic log files. The backend repeats the
/// confirmation check so a UI regression cannot clear them silently.
#[tauri::command(async)]
#[specta::specta]
pub(crate) fn clear_diagnostics(
    app: AppHandle,
    confirmed: bool,
    state: tauri::State<'_, DiagnosticsState>,
) -> Result<DiagnosticInfo, DiagnosticError> {
    let _guard = lock_diagnostics(&state)?;
    let log = DiagnosticLog::new(diagnostics_directory(&app)?);
    log.clear(confirmed)?;
    diagnostic_info(&log)
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

/// Defense in depth for exporting pre-Task-9A or hand-edited log files.
pub(crate) fn redact_text(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    if [
        "token",
        "password",
        "credential",
        "authorization",
        "api_key",
        "api-key",
        "query",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return "<redacted-sensitive-line>\n".to_string();
    }

    static PATH_PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern = PATH_PATTERN.get_or_init(|| {
        Regex::new(
            r#"(?x)
            (?:/[^\s"'<>/]+){2,}
            |
            [A-Za-z]:\\(?:[^\s"'<>\\]+\\)+[^\s"'<>\\]*
            "#,
        )
        .expect("constant diagnostic path redaction regex")
    });
    pattern.replace_all(input, "<redacted-path>").into_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        redact_text, DiagnosticError, DiagnosticEvent, DiagnosticIndexPhase, DiagnosticLevel,
        DiagnosticLog, MAX_EXPORT_BYTES,
    };

    #[test]
    fn redaction_removes_credentials_queries_and_raw_full_paths() {
        let input = concat!(
            "token=sk-secret query=select * from private ",
            "password=hunter2 /Users/alice/Secret/notes.md ",
            r#"C:\Users\alice\Secret\notes.md"#
        );
        let redacted = redact_text(input);
        for forbidden in [
            "sk-secret",
            "select",
            "hunter2",
            "/Users/alice",
            r#"C:\Users\alice"#,
        ] {
            assert!(
                !redacted.contains(forbidden),
                "redaction leaked {forbidden:?}: {redacted}"
            );
        }
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
    fn export_is_redacted_and_hard_bounded_even_with_a_hostile_legacy_log() {
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
        log.clear(true).expect("confirmed clear");
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
}
