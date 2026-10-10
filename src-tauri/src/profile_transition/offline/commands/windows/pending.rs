//! Reconcile retained capabilities before exposing a migration preview or state.
use super::*;
impl Pending {
    pub(in crate::profile_transition::offline::commands) fn handoff_issued(&self) -> bool {
        self.windows.handoff.issued()
            || self
                .windows
                .prepared
                .as_ref()
                .is_some_and(|prepared| prepared.handoff_issued())
    }
    pub(in crate::profile_transition::offline::commands) fn handoff_failure(
        &self,
    ) -> Option<&'static str> {
        self.windows.handoff.failure_kind().or_else(|| {
            self.windows
                .prepared
                .as_ref()
                .and_then(|prepared| prepared.handoff_failure())
        })
    }
    pub(super) fn waiting(&self) -> MigrationReply {
        let mut result = reply(
            match self.handoff_failure() {
                _ if self.windows.controller.is_some() => "return_controller_requested",
                _ if self.windows.rollback_pending => "rollback_handoff_pending",
                Some("background_service_unsupported") => "background_service_unsupported",
                _ if self.handoff_issued() => "service_handoff_pending",
                _ => "close_original_required",
            },
            Some(self.id),
        );
        result.origin = Some(
            if self.windows.observer.is_some()
                || self.windows.legacy.is_some()
                || self.windows.reobserve.is_some()
            {
                "legacy_catdb"
            } else {
                "reader"
            },
        );
        result
    }
    pub(super) fn finish(&mut self, app: &AppHandle) -> Result<MigrationReply> {
        if let Some(root) = self.windows.controller_root.as_ref() {
            if let Some(controller) = self.windows.live_controller.as_ref() {
                controller.verify_live()?;
            }
            let state = workspace::controller::request(&root.path, false)?;
            let mut result = reply(state, Some(self.id));
            result.origin = Some("legacy_catdb");
            result.imported = self.windows.legacy_imported;
            return Ok(result);
        }
        if let Some((root, launch)) = self.windows.reobserve.as_ref() {
            if self.windows.observer.is_none() && self.windows.legacy.is_none() {
                match launch.observe_application() {
                    Ok(observer) => self.windows.observer = Some(observer),
                    Err(Error::NotQuiescent) => {
                        let mut result = reply("reobserve_original_required", Some(self.id));
                        result.origin = Some("legacy_catdb");
                        return Ok(result);
                    }
                    Err(error) => return Err(error),
                }
            }
            if self.windows.legacy.is_none() {
                match ClosedOriginal::acquire_retaining(
                    &mut self.windows.observer,
                    Default::default(),
                ) {
                    Ok(owner) => self.windows.legacy = Some(owner),
                    Err(Error::NotQuiescent) => return Ok(self.waiting()),
                    Err(error) => return Err(error),
                }
            }
            if self.windows.preserved_draft {
                match workspace::legacy::Draft::resume(
                    &root.path,
                    self.windows.legacy.as_mut().ok_or(Error::Conflict)?,
                )? {
                    workspace::legacy::Recovery::Draft(draft) => self.windows.draft = Some(*draft),
                    workspace::legacy::Recovery::Prepared(prepared) => {
                        self.windows.prepared = Some(*prepared)
                    }
                }
                self.windows.reobserve = None;
            } else {
                // Actual lock acquisition blocks a concurrent retained controller;
                // a failed network response alone never declares it absent.
                let private = mdreader_capture::PrivateDirectory::open(&root.path)
                    .map_err(|_| Error::Conflict)?;
                let lock = private
                    .open_lock(Path::new("controller.lock"))
                    .map_err(|_| Error::Storage)?;
                fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| Error::NotQuiescent)?;
                self.windows.prepared = Some(workspace::Prepared::resume(&root.path)?);
                private.verify().map_err(|_| Error::Conflict)?;
                self.windows.reobserve = None;
            }
        }
        if self.windows.observer.is_some() || self.windows.legacy.is_some() {
            if self.windows.legacy.is_none() {
                match ClosedOriginal::acquire_retaining(
                    &mut self.windows.observer,
                    Default::default(),
                ) {
                    Ok(owner) => self.windows.legacy = Some(owner),
                    Err(Error::NotQuiescent) => return Ok(self.waiting()),
                    Err(error) => return Err(error),
                }
            }
            if self.windows.prepared.is_none() {
                let owner = self.windows.legacy.as_mut().ok_or(Error::Conflict)?;
                if self.windows.draft.is_none() {
                    let Some(parent) = selected_parent(app, owner.path())? else {
                        return Ok(reply("cancelled", Some(self.id)));
                    };
                    self.windows.draft = Some(workspace::legacy::Draft::begin(
                        owner,
                        &parent.path,
                        self.id,
                    )?);
                    parent.verify().map_err(|_| Error::Conflict)?;
                }
                let mut result = reply("preview", Some(self.id));
                result.origin = Some("legacy_catdb");
                result.documents = self
                    .windows
                    .draft
                    .as_ref()
                    .ok_or(Error::Conflict)?
                    .documents()?;
                return Ok(result);
            }
            if self
                .windows
                .prepared
                .as_mut()
                .ok_or(Error::Conflict)?
                .state()?
                == "launch_requested"
            {
                let prepared = self.windows.prepared.take().ok_or(Error::Conflict)?;
                let imported = self.windows.legacy_imported;
                let root = SourceRoot::capture(prepared.launch_directory())
                    .map_err(|_| Error::Conflict)?;
                let original = self.windows.legacy.take().ok_or(Error::Conflict)?;
                self.windows.live_controller = Some(workspace::controller::start(
                    app.clone(),
                    original,
                    prepared,
                )?);
                self.windows.controller_root = Some(root);
                self.windows.legacy_imported = imported;
                return self.finish(app);
            }
        }
        if self.windows.rollback_pending
            || self.windows.controller.is_some()
            || (self.windows.prepared.is_some() && self.handoff_issued())
        {
            return Ok(self.waiting());
        }
        if let Some(prepared) = self.windows.prepared.as_mut() {
            let mut state = prepared.state()?;
            if state == "launch_requested" {
                let selected = super::super::windows_workspace::selected_for_launch(
                    &prepared.launch_directory(),
                )?;
                if crate::reader_profile::is_imported_selected(app, &selected.path)? {
                    state = "activated";
                }
            }
            let mut result = reply(state, Some(self.id));
            result.origin = Some("reader");
            result.imported = prepared.imported()?;
            return Ok(result);
        }
        self.windows.root.verify().map_err(|_| Error::Conflict)?;
        if self.windows.original.is_none() {
            let acquired =
                ClosedOwner::acquire_retaining(&self.windows.root.path, &mut self.windows.handoff);
            match acquired {
                Ok(owner) => self.windows.original = Some(owner),
                Err(Error::NotQuiescent | Error::ReconciliationRequired) => {
                    return Ok(self.waiting())
                }
                Err(error) => return Err(error),
            }
        }
        let owner = self.windows.original.as_mut().ok_or(Error::Conflict)?;
        crate::profile_transition::backup_inventory::selections(owner.path(), &Default::default())?;
        let store = owner.store()?;
        let documents = super::super::conversion::documents(&store)?
            .into_iter()
            .map(|document| Document {
                document_id: document.id,
                name: document.name,
                revision: document.revision,
                bytes: document.bytes,
                pinned: document.pinned,
                line: document.line,
            })
            .collect();
        owner.revalidate()?;
        let mut result = reply("preview", Some(self.id));
        result.origin = Some("reader");
        result.documents = documents;
        Ok(result)
    }
}
