//! Sole strict owner for desktop session/settings JSON wrappers. Startup
//! omits the plugin writer and registers this owner before exposing commands.
//! Preservation flush/event wiring is a separate composition responsibility.
pub(crate) mod bridge;
#[cfg(all(test, unix))]
mod bridge_tests;
pub(crate) mod commands;
pub(crate) mod editor;
#[cfg(not(windows))]
mod files;
#[cfg(windows)]
#[path = "windows_files.rs"]
mod files;
mod json;
#[cfg(all(test, unix))]
mod tests;
mod types;
#[cfg(windows)]
pub(crate) mod windows;
use mdreader_core::SourceRoot;
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Mutex};
use types::Result;
pub(crate) use types::{
    JsonKind, PersistedJson, PersistenceError, PreflushJson, RendererIdentity, WriteReceipt,
};
use uuid::Uuid;

pub(crate) struct JsonOwner {
    root: SourceRoot,
    state: Mutex<State>,
    lifecycle: files::JsonLifecycle,
}
struct State {
    identity: RendererIdentity,
    window: Option<String>,
    generations: [u32; 2],
    receipts: [Option<(String, WriteReceipt)>; 2],
    frozen: Option<FlushState>,
    uncertain: bool,
}
struct FlushState {
    operation: Uuid,
    originals: [Option<files::Original>; 2],
    published: [bool; 2],
}
impl JsonOwner {
    /// The root is supplied by the native composition root, never renderer data.
    pub(crate) fn new(root: SourceRoot) -> Result<Self> {
        if !cfg!(any(unix, windows)) {
            return Err(PersistenceError::Unavailable);
        }
        let lifecycle = files::JsonLifecycle::capture(&root)?;
        Ok(Self {
            root,
            lifecycle,
            state: Mutex::new(State {
                identity: RendererIdentity {
                    owner_epoch: Uuid::new_v4().to_string(),
                    renderer_epoch: Uuid::new_v4().to_string(),
                },
                window: None,
                generations: [0, 0],
                receipts: [None, None],
                frozen: None,
                uncertain: false,
            }),
        })
    }
    /// `window` comes from the invoking native Window label. Only one renderer
    /// may register; a replacement requires a new explicit owner lifecycle.
    pub(crate) fn load(&self, window: &str) -> Result<PersistedJson> {
        if window.is_empty() || window.len() > 256 {
            return Err(PersistenceError::Invalid);
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistenceError::Unavailable)?;
        self.verify_owner()?;
        check(&state)?;
        if state.frozen.is_some() {
            return Err(PersistenceError::Frozen);
        }
        if state.window.as_deref().is_some_and(|old| old != window) {
            return Err(PersistenceError::Conflict);
        }
        let session = self.read(JsonKind::Session)?;
        let settings = self.read(JsonKind::Settings)?;
        // Reload invalidates packets from the preceding renderer runtime,
        // including when its native window label is unchanged.
        state.identity.renderer_epoch = Uuid::new_v4().to_string();
        state.generations = [0, 0];
        state.receipts = [None, None];
        state.window = Some(window.to_owned());
        Ok(PersistedJson {
            identity: state.identity.clone(),
            session_json: session,
            settings_json: settings,
        })
    }
    fn read(&self, kind: JsonKind) -> Result<Option<String>> {
        let Some(original) = files::read(&self.root, &self.lifecycle, Path::new(kind.file()))?
        else {
            return Ok(None);
        };
        json::validate(&original.bytes)?;
        String::from_utf8(original.bytes)
            .map(Some)
            .map_err(|_| PersistenceError::Invalid)
    }
    pub(crate) fn read_current(
        &self,
        window: &str,
        identity: &RendererIdentity,
    ) -> Result<PersistedJson> {
        let state = self
            .state
            .lock()
            .map_err(|_| PersistenceError::Unavailable)?;
        self.verify_owner()?;
        authenticate(&state, window, identity)?;
        if state.frozen.is_some() {
            return Err(PersistenceError::Frozen);
        }
        Ok(PersistedJson {
            identity: state.identity.clone(),
            session_json: self.read(JsonKind::Session)?,
            settings_json: self.read(JsonKind::Settings)?,
        })
    }
    /// A caller must retain its global EffectLease through this actual worker.
    /// Success means durable native publication, not merely enqueued: Unix
    /// syncs the file/directory; Windows commits the canonical private journal.
    pub(crate) fn write(
        &self,
        window: &str,
        identity: &RendererIdentity,
        kind: JsonKind,
        generation: u32,
        patch: &str,
    ) -> Result<WriteReceipt> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistenceError::Unavailable)?;
        self.verify_owner()?;
        authenticate(&state, window, identity)?;
        if state.frozen.is_some() {
            return Err(PersistenceError::Frozen);
        }
        self.write_locked(&mut state, kind, generation, patch)
    }
    fn write_locked(
        &self,
        state: &mut State,
        kind: JsonKind,
        generation: u32,
        patch: &str,
    ) -> Result<WriteReceipt> {
        check(state)?;
        if patch.len() > types::MAX_JSON_BYTES {
            return Err(PersistenceError::TooLarge);
        }
        let index = kind.index();
        let digest = format!("{:x}", Sha256::digest(patch.as_bytes()));
        if generation == state.generations[index] {
            if let Some((prior, receipt)) = &state.receipts[index] {
                if prior == &digest {
                    return Ok(receipt.clone());
                }
            }
            return Err(PersistenceError::Conflict);
        }
        if generation == 0 || generation < state.generations[index] || generation > i32::MAX as u32
        {
            return Err(PersistenceError::Conflict);
        }
        let original = files::read(&self.root, &self.lifecycle, Path::new(kind.file()))?;
        let bytes = json::merge(kind, original.as_ref().map(|v| v.bytes.as_slice()), patch)?;
        let publication = files::replace(
            &self.root,
            &self.lifecycle,
            Path::new(kind.file()),
            original.as_ref(),
            &bytes,
        );
        if publication == Err(PersistenceError::PublicationUncertain) {
            state.uncertain = true;
        }
        publication?;
        let receipt = WriteReceipt {
            kind,
            generation,
            owner_epoch: state.identity.owner_epoch.clone(),
            renderer_epoch: state.identity.renderer_epoch.clone(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        state.generations[index] = generation;
        state.receipts[index] = Some((digest, receipt.clone()));
        Ok(receipt)
    }
    /// Called by the native freeze adapter after global admission is denied and
    /// drained. The opaque operation is native-issued, not a readiness boolean.
    /// Exact originals remain owned bytes until explicitly archived. No files
    /// are added inside the installed profile to hold these preimages.
    pub(crate) fn begin_flush(&self, operation: Uuid) -> Result<Vec<PreflushJson>> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistenceError::Unavailable)?;
        self.verify_owner()?;
        check(&state)?;
        if state.window.is_none() || state.frozen.is_some() {
            return Err(PersistenceError::Frozen);
        }
        let mut originals = Vec::new();
        let mut retained = [None, None];
        for kind in [JsonKind::Session, JsonKind::Settings] {
            let original = files::read(&self.root, &self.lifecycle, Path::new(kind.file()))?;
            if let Some(original) = &original {
                json::validate(&original.bytes)?;
            }
            originals.push(PreflushJson {
                kind,
                bytes: original.as_ref().map(|v| v.bytes.clone()),
            });
            retained[kind.index()] = original;
        }
        state.frozen = Some(FlushState {
            operation,
            originals: retained,
            published: [false; 2],
        });
        Ok(originals)
    }
    /// Only the trusted freeze adapter may route operation-bound latest-state
    /// flushes here. The renderer command must not expose arbitrary operation IDs.
    pub(crate) fn flush(
        &self,
        operation: Uuid,
        window: &str,
        identity: &RendererIdentity,
        kind: JsonKind,
        generation: u32,
        patch: &str,
    ) -> Result<WriteReceipt> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistenceError::Unavailable)?;
        self.verify_owner()?;
        authenticate(&state, window, identity)?;
        let frozen = state.frozen.as_ref().ok_or(PersistenceError::Conflict)?;
        if frozen.operation != operation {
            return Err(PersistenceError::Conflict);
        }
        if !frozen.published[kind.index()] {
            files::verify(
                &self.root,
                &self.lifecycle,
                Path::new(kind.file()),
                frozen.originals[kind.index()].as_ref(),
            )?;
        }
        let receipt = self.write_locked(&mut state, kind, generation, patch)?;
        state
            .frozen
            .as_mut()
            .ok_or(PersistenceError::Conflict)?
            .published[kind.index()] = true;
        Ok(receipt)
    }
    pub(crate) fn revalidate_flush(
        &self,
        operation: Uuid,
        receipts: &[WriteReceipt; 2],
    ) -> Result<()> {
        let state = self
            .state
            .lock()
            .map_err(|_| PersistenceError::Unavailable)?;
        self.verify_owner()?;
        check(&state)?;
        self.root.verify().map_err(|_| PersistenceError::Conflict)?;
        if state.frozen.as_ref().map(|v| v.operation) != Some(operation) {
            return Err(PersistenceError::Conflict);
        }
        for (kind, receipt) in [JsonKind::Session, JsonKind::Settings]
            .into_iter()
            .zip(receipts)
        {
            let Some((_, current)) = &state.receipts[kind.index()] else {
                return Err(PersistenceError::Conflict);
            };
            if receipt.kind != kind
                || current.generation != receipt.generation
                || current.sha256 != receipt.sha256
                || current.owner_epoch != receipt.owner_epoch
                || current.renderer_epoch != receipt.renderer_epoch
            {
                return Err(PersistenceError::Conflict);
            }
            let bytes = files::read(&self.root, &self.lifecycle, Path::new(kind.file()))?
                .ok_or(PersistenceError::Conflict)?
                .bytes;
            if format!("{:x}", Sha256::digest(&bytes)) != receipt.sha256 {
                return Err(PersistenceError::Conflict);
            }
        }
        Ok(())
    }
    /// Preservation-only release after all final proof checks. Activation must
    /// keep old writers denied and use a separate durable handoff instead.
    pub(crate) fn release_flush(&self, operation: Uuid) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PersistenceError::Unavailable)?;
        self.verify_owner()?;
        check(&state)?;
        self.root.verify().map_err(|_| PersistenceError::Conflict)?;
        if state.frozen.as_ref().map(|v| v.operation) != Some(operation) {
            return Err(PersistenceError::Conflict);
        }
        state.frozen = None;
        Ok(())
    }
    pub(super) fn renderer(&self) -> Result<(String, RendererIdentity)> {
        let state = self
            .state
            .lock()
            .map_err(|_| PersistenceError::Unavailable)?;
        check(&state)?;
        Ok((
            state.window.clone().ok_or(PersistenceError::Unavailable)?,
            state.identity.clone(),
        ))
    }
    fn verify_owner(&self) -> Result<()> {
        self.lifecycle.verify(&self.root)
    }
    pub(super) fn verify_binding(&self, binding: &mdreader_profile::RootBinding) -> Result<()> {
        self.verify_owner()?;
        if binding
            .locator
            .restore()
            .map_err(|_| PersistenceError::Conflict)?
            != self.root.path
        {
            return Err(PersistenceError::Conflict);
        }
        match self.root.identity {
            mdreader_core::SourceRootIdentity::Unix { device, inode }
                if binding.device == device
                    && binding.inode == inode
                    && binding.windows_file_id.is_none() =>
            {
                Ok(())
            }
            mdreader_core::SourceRootIdentity::Windows {
                volume_serial_number,
                file_id,
            } if binding.device == volume_serial_number
                && binding.windows_file_id == Some(file_id) =>
            {
                binding.validate().map_err(|_| PersistenceError::Conflict)
            }
            _ => Err(PersistenceError::Unavailable),
        }
    }
}
fn check(state: &State) -> Result<()> {
    if state.uncertain {
        Err(PersistenceError::PublicationUncertain)
    } else {
        Ok(())
    }
}
fn authenticate(state: &State, window: &str, identity: &RendererIdentity) -> Result<()> {
    check(state)?;
    if state.window.as_deref() != Some(window)
        || state.identity.owner_epoch != identity.owner_epoch
        || state.identity.renderer_epoch != identity.renderer_epoch
    {
        return Err(PersistenceError::Conflict);
    }
    Ok(())
}
