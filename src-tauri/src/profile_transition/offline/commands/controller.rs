//! Explicit Return transfers work to a separate native GUI, never unlocks self.
use super::*;

pub(super) fn resume(app: &AppHandle, pending: &mut Option<Pending>) -> Result<MigrationReply> {
    #[cfg(not(mobile))]
    {
        let target = crate::reader_profile::return_target(app)?;
        let archive = super::super::archive_import::ArchiveImport::resume(&target.path)?;
        let legacy =
            super::super::legacy_return::resume(&target.path)?.ok_or(Error::Unsupported)?;
        let active = Pending {
            id: Uuid::new_v4(),
            rollback_pending: true,
            controller_issued: false,
            original: None,
            opening: None,
            legacy: Some(legacy),
            workspace: None,
            archive: Some(archive),
        };
        let result = active.waiting();
        *pending = Some(active);
        Ok(result)
    }
    #[cfg(mobile)]
    {
        let _ = (app, pending);
        Err(Error::Unsupported)
    }
}

pub(super) fn launch_if_current(app: &AppHandle, active: &mut Pending) -> Result<bool> {
    #[cfg(not(mobile))]
    {
        let candidate = active
            .archive
            .as_ref()
            .ok_or(Error::Conflict)?
            .selected_profile()?;
        let current = crate::reader_profile::directory(app).map_err(|_| Error::Conflict)?;
        if current != candidate {
            return Ok(false);
        }
        if !crate::reader_profile::is_imported_selected(app, candidate)? {
            return Err(Error::Conflict);
        }
        super::super::legacy_return::resume(candidate)?.ok_or(Error::Unsupported)?;
        let target = SourceRoot::capture(candidate.to_owned()).map_err(|_| Error::Conflict)?;
        let executable = std::env::current_exe().map_err(|_| Error::Storage)?;
        target.verify().map_err(|_| Error::Conflict)?;
        // Retain before OS dispatch. An uncertain spawn cannot dispatch a second
        // child or fall through to the current GUI's own writer locks.
        active.controller_issued = true;
        std::process::Command::new(executable)
            .arg("--reader-return-controller")
            .arg(&target.path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|_| Error::ReconciliationRequired)?;
        Ok(true)
    }
    #[cfg(mobile)]
    {
        let _ = (app, active);
        Err(Error::Unsupported)
    }
}
