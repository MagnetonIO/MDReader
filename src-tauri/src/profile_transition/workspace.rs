//! Actual native workspace/Library admission and retained grant observation.
use super::{EffectLease, FreezeScope, FrozenParticipant, LiveParticipant};
use mdreader_profile::{Error, Participant, Result, RootBinding};
use std::{path::Path, sync::Arc, time::Instant};
use tauri::Manager;

pub(crate) struct AdmittedWorkspace<'a> {
    workspace: &'a crate::commands::WorkspaceState,
    _lease: Option<EffectLease>,
}
impl std::ops::Deref for AdmittedWorkspace<'_> {
    type Target = crate::commands::WorkspaceState;
    fn deref(&self) -> &Self::Target {
        self.workspace
    }
}
pub(crate) fn admit<'a>(
    app: &tauri::AppHandle,
    workspace: &'a crate::commands::WorkspaceState,
) -> std::result::Result<AdmittedWorkspace<'a>, String> {
    Ok(AdmittedWorkspace {
        workspace,
        _lease: effect(app)?,
    })
}

pub(crate) fn effect(app: &tauri::AppHandle) -> std::result::Result<Option<EffectLease>, String> {
    if !cfg!(all(unix, not(mobile))) {
        return Ok(None);
    }
    let state = app
        .try_state::<super::persistence::commands::PersistenceState>()
        .ok_or("Native profile admission is unavailable")?;
    state
        .transition
        .as_ref()
        .ok_or("Native profile admission is unavailable")?
        .effect(Participant::WorkspaceAndReviews)
        .map(Some)
        .map_err(|_| "Profile preservation is in progress".into())
}
pub(super) struct ParticipantOwner {
    root: RootBinding,
    app: tauri::AppHandle,
}
impl ParticipantOwner {
    pub(super) fn new(root: RootBinding, app: tauri::AppHandle) -> Self {
        Self { root, app }
    }
}
impl LiveParticipant for ParticipantOwner {
    fn participant(&self) -> Participant {
        Participant::WorkspaceAndReviews
    }
    fn root(&self) -> &RootBinding {
        &self.root
    }
    fn freeze(&self, scope: &FreezeScope<'_>) -> Result<Box<dyn FrozenParticipant>> {
        let library = self
            .app
            .try_state::<crate::library::LibraryState>()
            .ok_or(Error::NotQuiescent)?;
        let _owner = library.0.try_lock().map_err(|_| Error::NotQuiescent)?;
        let workspace = self
            .app
            .try_state::<crate::commands::WorkspaceState>()
            .ok_or(Error::NotQuiescent)?;
        let frozen = FrozenWorkspace {
            root: self.root.clone(),
            app: self.app.clone(),
            epoch: workspace.current(),
            operation: scope.operation(),
            owner_epoch: scope.owner_epoch(),
        };
        frozen.verify(scope)?;
        Ok(Box::new(frozen))
    }
}
struct FrozenWorkspace {
    root: RootBinding,
    app: tauri::AppHandle,
    epoch: Option<Arc<crate::engine::Epoch>>,
    operation: uuid::Uuid,
    owner_epoch: uuid::Uuid,
}
impl FrozenWorkspace {
    fn verify(&self, scope: &FreezeScope<'_>) -> Result<()> {
        if self.root != *scope.root()
            || self.operation != scope.operation()
            || self.owner_epoch != scope.owner_epoch()
            || Instant::now() >= scope.deadline()
            || crate::reader_profile::directory(&self.app).map_err(|_| Error::Conflict)?
                != self.root.locator.restore()?
        {
            return Err(Error::Conflict);
        }
        let workspace = self
            .app
            .try_state::<crate::commands::WorkspaceState>()
            .ok_or(Error::NotQuiescent)?;
        match (&self.epoch, workspace.current()) {
            (None, None) => Ok(()),
            (Some(expected), Some(current)) if Arc::ptr_eq(expected, &current) => {
                expected.validate_root().map_err(|_| Error::Conflict)
            }
            _ => Err(Error::Conflict),
        }
    }
}
impl FrozenParticipant for FrozenWorkspace {
    fn revalidate(&mut self, scope: &FreezeScope<'_>) -> Result<()> {
        self.verify(scope)
    }
    fn with_database(
        &mut self,
        _: &Path,
        _: &mut dyn FnMut(&rusqlite::Connection) -> Result<()>,
    ) -> Result<()> {
        Err(Error::Invalid)
    }
}
