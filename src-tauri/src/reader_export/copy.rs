//! Destination authorization and exclusive copy writing shared by exact
//! document export and source-independent draft preservation.
use super::{failure, ExportError, ExportReply, ExportState};
use mdreader_core::SourceRoot;
use std::{
    io::Write,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

#[derive(Default)]
pub(crate) struct CopyRuntime {
    busy: Arc<AtomicBool>,
}
pub(super) struct CopyLease(Arc<AtomicBool>);
impl Drop for CopyLease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl CopyRuntime {
    pub(super) fn claim(&self) -> Result<CopyLease, ExportError> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| failure("busy"))?;
        Ok(CopyLease(self.busy.clone()))
    }
}
pub(super) fn choose_and_write(
    app: &AppHandle,
    filename: &str,
    body: impl FnOnce() -> Result<String, ExportError>,
) -> Result<ExportReply, ExportError> {
    let Some(destination) = app
        .dialog()
        .file()
        .set_title("Save Markdown copy — choose a new file")
        .add_filter("Markdown", &["md", "markdown"])
        .set_file_name(filename)
        .blocking_save_file()
    else {
        return Ok(ExportReply {
            state: ExportState::Cancelled,
        });
    };
    let destination = destination
        .into_path()
        .map_err(|_| failure("unsupported_destination"))?;
    let markdown = body()?;
    if markdown.len() > 2 * 1024 * 1024 {
        return Err(failure("too_large"));
    }
    write_new(&destination, markdown.as_bytes())?;
    Ok(ExportReply {
        state: ExportState::Exported,
    })
}
pub(super) fn write_new(destination: &Path, markdown: &[u8]) -> Result<(), ExportError> {
    if !destination.is_absolute() {
        return Err(failure("destination_unavailable"));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| failure("destination_unavailable"))?;
    let name = destination
        .file_name()
        .ok_or_else(|| failure("destination_unavailable"))?;
    let root = SourceRoot::capture(parent.to_path_buf())
        .map_err(|_| failure("destination_unavailable"))?;
    // Descriptor-relative exclusive creation refuses existing destinations and
    // raced-in objects; the selected original can never be truncated/replaced.
    let mut file = root
        .create_new_relative(Path::new(name))
        .map_err(|_| failure("destination_exists_or_unavailable"))?;
    file.write_all(markdown)
        .and_then(|()| file.sync_all())
        .map_err(|_| failure("write_unconfirmed"))?;
    let directory = root
        .try_clone_directory_handle()
        .map_err(|_| failure("write_unconfirmed"))?;
    directory
        .sync_all()
        .map_err(|_| failure("write_unconfirmed"))?;
    root.verify().map_err(|_| failure("write_unconfirmed"))?;
    // An uncertain partial copy remains. Deleting a name could target another
    // actor's replacement; callers report uncertainty and retain their draft.
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_copy_dialog_claim_survives_until_the_worker_releases_it() {
        let runtime = CopyRuntime::default();
        let lease = runtime.claim().unwrap();
        assert_eq!(runtime.claim().err().unwrap().kind, "busy");
        drop(lease);
        assert!(runtime.claim().is_ok());
    }
}
