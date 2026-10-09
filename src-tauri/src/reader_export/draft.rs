//! Preserve one explicit UTF-8 editor snapshot without source authorization.
//! Filename is display-only; output authority comes solely from the OS dialog.
use super::{copy, copy_filename, failure, CopyRuntime, ExportError, ExportReply};
use serde::Deserialize;
use specta::Type;
use tauri::{AppHandle, State};

#[derive(Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftSnapshot {
    pub markdown: String,
    pub filename: String,
}
impl DraftSnapshot {
    fn validate(&self) -> Result<(), ExportError> {
        if self.markdown.len() > 2 * 1024 * 1024 {
            return Err(failure("too_large"));
        }
        if self.filename.len() > 1024 {
            return Err(failure("invalid"));
        }
        Ok(())
    }
}
#[tauri::command]
#[specta::specta]
pub async fn reader_export_draft(
    app: AppHandle,
    snapshot: DraftSnapshot,
    copies: State<'_, CopyRuntime>,
) -> Result<ExportReply, ExportError> {
    snapshot.validate()?;
    let filename = copy_filename(&snapshot.filename);
    let lease = copies.claim()?;
    tauri::async_runtime::spawn_blocking(move || {
        // Hold through actual OS completion even if the renderer goes away.
        let _lease = lease;
        copy::choose_and_write(&app, &filename, || Ok(snapshot.markdown))
    })
    .await
    .map_err(|_| failure("write_unconfirmed"))?
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn draft_budget_counts_exact_utf8_bytes_and_accepts_empty_recovery_copies() {
        assert!(DraftSnapshot {
            markdown: String::new(),
            filename: "draft.md".into()
        }
        .validate()
        .is_ok());
        assert!(DraftSnapshot {
            markdown: "λ".repeat(1024 * 1024),
            filename: "draft.md".into()
        }
        .validate()
        .is_ok());
        assert_eq!(
            DraftSnapshot {
                markdown: "λ".repeat(1024 * 1024 + 1),
                filename: "draft.md".into()
            }
            .validate()
            .unwrap_err()
            .kind,
            "too_large"
        );
    }
}
