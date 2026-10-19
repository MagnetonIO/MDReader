//! Native capability retained until whole-registry final validation/release.
use super::*;
use crate::profile_transition::FrozenParticipant;
use mdreader_profile::{
    bundle::{PreflushArtifact, PreflushKind},
    Participant,
};
use std::path::Path;

pub(super) struct ParticipantOwner(pub Arc<EditorOwner>);
impl LiveParticipant for ParticipantOwner {
    fn participant(&self) -> Participant {
        Participant::UnsavedEditor
    }
    fn root(&self) -> &RootBinding {
        &self.0.root
    }
    fn freeze(&self, scope: &FreezeScope<'_>) -> Result<Box<dyn FrozenParticipant>> {
        let bytes = self.0.freeze(scope)?;
        Ok(Box::new(FrozenEditor {
            owner: Arc::clone(&self.0),
            bytes,
            released: false,
        }))
    }
    fn post_resume(&self, scope: &FreezeScope<'_>) -> Result<()> {
        let notice = {
            let mut phase = self.0.phase.lock().map_err(|_| Error::NotQuiescent)?;
            match &*phase {
                Phase::Idle => None, // No editor request was issued.
                Phase::Released(pending) => {
                    self.0.verify(pending, scope)?;
                    let notice = (pending.window.clone(), pending.request.clone());
                    *phase = Phase::Idle;
                    Some(notice)
                }
                _ => return Err(Error::NotQuiescent),
            }
        };
        if let Some((window, request)) = notice {
            self.0
                .emit(&window, "mdreader://profile-editor-released", &request)?;
        }
        Ok(())
    }
}
struct FrozenEditor {
    owner: Arc<EditorOwner>,
    bytes: Arc<Vec<u8>>,
    released: bool,
}
impl FrozenParticipant for FrozenEditor {
    fn revalidate(&mut self, scope: &FreezeScope<'_>) -> Result<()> {
        let phase = self.owner.phase.lock().map_err(|_| Error::NotQuiescent)?;
        let Phase::Held(pending) = &*phase else {
            return Err(Error::NotQuiescent);
        };
        self.owner.verify(pending, scope)?;
        if pending
            .archive
            .as_ref()
            .is_none_or(|bytes| !Arc::ptr_eq(bytes, &self.bytes))
        {
            return Err(Error::NotQuiescent);
        }
        Ok(())
    }
    fn release(&mut self, scope: &FreezeScope<'_>) -> Result<()> {
        self.revalidate(scope)?;
        self.released = true;
        Ok(())
    }
    fn preflush_json(&mut self) -> Result<Vec<PreflushArtifact>> {
        Ok(vec![PreflushArtifact {
            kind: PreflushKind::EditorBuffers,
            bytes: Some((*self.bytes).clone()),
        }])
    }
    fn with_database(
        &mut self,
        _: &Path,
        _: &mut dyn FnMut(&rusqlite::Connection) -> Result<()>,
    ) -> Result<()> {
        Err(Error::Invalid)
    }
}
impl Drop for FrozenEditor {
    fn drop(&mut self) {
        if let Ok(mut phase) = self.owner.phase.lock() {
            let previous = std::mem::replace(&mut *phase, Phase::Quarantined);
            if let Phase::Held(pending) = previous {
                if self.released
                    && pending
                        .archive
                        .as_ref()
                        .is_some_and(|bytes| Arc::ptr_eq(bytes, &self.bytes))
                {
                    *phase = Phase::Released(pending);
                }
            }
        }
    }
}
