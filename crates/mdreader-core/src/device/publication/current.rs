//! Verified current bytes/state for domain transitions; never a generic writer.
use super::{files::FileEvidence, reservation::origin_key, types::*};
use crate::{
    device::{
        self,
        assets::{AssetStore, CurrentAssets},
        state::DocumentState,
    },
    ReaderStore, SourceRoot,
};
use rusqlite::{Connection, OptionalExtension, Transaction};
use std::{marker::PhantomData, path::Path, sync::Arc};

pub struct CurrentPlan {
    pub(super) state: DocumentState,
    pub(super) root: SourceRoot,
    pub(super) assets: AssetStore,
    pub(super) asset_state: CurrentAssets,
    pub(super) owner: Arc<()>,
    pub(super) origin: String,
}
pub struct VerifiedCurrent {
    pub(super) plan: CurrentPlan,
    pub(super) file: FileEvidence,
    pub(super) max_line: u32,
}
pub struct CurrentGuard<'owner> {
    pub(super) connection: usize,
    pub(super) owner: Arc<()>,
    pub(super) origin: String,
    pub(super) _borrow: PhantomData<&'owner mut Connection>,
}
pub fn prepare_current(
    store: &mut ReaderStore,
    id: &str,
    assets: &AssetStore,
    origin: &IncomingOrigin,
) -> PublicationResult<CurrentPlan> {
    super::reservation::require_platform()?;
    device::schema(store)?;
    assets.incoming_owner(store)?;
    let (state, root, asset_state) =
        store.with_library_connection(|db| -> PublicationResult<_> {
            let tx = db.transaction()?;
            let state = device::state::load(&tx, id, true)?;
            let root = source_root(&tx, &state.document.source_id)?;
            crate::store::validate_root(&tx, &root).map_err(device::Error::from)?;
            let asset_state = assets.current_assets_in(&tx, id, &state.document.revision)?;
            tx.commit()?;
            Ok((state, root, asset_state))
        })?;
    Ok(CurrentPlan {
        state,
        root,
        assets: assets.clone(),
        asset_state,
        owner: store.device_owner_session(),
        origin: origin_key(origin)?,
    })
}
pub(super) fn source_root(db: &Connection, source: &str) -> PublicationResult<SourceRoot> {
    let (path, identity): (Vec<u8>, String) = db.query_row(
        "SELECT path,identity FROM sources WHERE id=?1",
        [source],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(path));
        Ok(SourceRoot::restore(path, serde_json::from_str(&identity)?)?)
    }
    #[cfg(not(unix))]
    {
        let _ = (path, identity);
        Err(PublicationError::UnsupportedPlatform)
    }
}
impl CurrentPlan {
    /// Read/hash exact old bytes outside owner/authority locks. No position conversion.
    pub fn verify(self) -> PublicationResult<VerifiedCurrent> {
        let (file, bytes) = FileEvidence::read(
            &self.root,
            Path::new("document.md").into(),
            self.state.document.bytes,
        )?;
        if blake3::hash(&bytes).to_hex().as_str() != self.state.document.revision {
            return Err(PublicationError::Conflict);
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| PublicationError::Conflict)?;
        let max_line = text.lines().count().max(1) as u32;
        Ok(VerifiedCurrent {
            plan: self,
            file,
            max_line,
        })
    }
}
impl VerifiedCurrent {
    pub fn state(&self) -> &DocumentState {
        &self.plan.state
    }
    pub fn max_line(&self) -> u32 {
        self.max_line
    }
}
pub(super) fn bind_current(
    store: &ReaderStore,
    current: &VerifiedCurrent,
    origin: &IncomingOrigin,
) -> PublicationResult<()> {
    if !Arc::ptr_eq(&current.plan.owner, &store.device_owner_session())
        || current.plan.origin != origin_key(origin)?
    {
        return Err(PublicationError::Conflict);
    }
    current.plan.root.verify()?;
    current.file.verify()?;
    Ok(())
}
/// Guard cannot escape this exact borrowed live-owner connection. Caller rolls
/// back on error and invokes a domain final revalidation immediately precommit.
pub fn with_transition<T, E>(
    store: &mut ReaderStore,
    current: &VerifiedCurrent,
    origin: &IncomingOrigin,
    action: impl for<'owner> FnOnce(
        &'owner mut Connection,
        CurrentGuard<'owner>,
    ) -> std::result::Result<T, E>,
) -> PublicationResult<std::result::Result<T, E>> {
    bind_current(store, current, origin)?;
    current.plan.assets.incoming_owner(store)?;
    Ok(store.with_library_connection(|db| {
        let guard = CurrentGuard {
            connection: db as *const Connection as usize,
            owner: current.plan.owner.clone(),
            origin: current.plan.origin.clone(),
            _borrow: PhantomData,
        };
        action(db, guard)
    }))
}
pub(super) fn guard_current(
    tx: &Transaction<'_>,
    guard: &CurrentGuard<'_>,
    current: &VerifiedCurrent,
) -> PublicationResult<()> {
    if &**tx as *const Connection as usize != guard.connection
        || !Arc::ptr_eq(&guard.owner, &current.plan.owner)
        || guard.origin != current.plan.origin
    {
        return Err(PublicationError::Conflict);
    }
    device::schema::initialize_in(tx)?;
    current.plan.root.verify()?;
    current.file.verify()?;
    let stored = source_root(tx, &current.plan.state.document.source_id)?;
    let name: String = tx.query_row(
        "SELECT name FROM sources WHERE id=?1",
        [&current.plan.state.document.source_id],
        |r| r.get(0),
    )?;
    if stored.path != current.plan.root.path
        || stored.identity != current.plan.root.identity
        || name != current.plan.state.document.name
    {
        return Err(PublicationError::Conflict);
    }
    crate::store::validate_root(tx, &current.plan.root).map_err(device::Error::from)?;
    current.plan.assets.incoming_verify_tx(tx)?;
    Ok(())
}
pub(super) fn verify_live(
    tx: &Transaction<'_>,
    guard: &CurrentGuard<'_>,
    current: &VerifiedCurrent,
) -> PublicationResult<()> {
    guard_current(tx, guard, current)?;
    if device::state::load(tx, &current.plan.state.document.id, true)? != current.plan.state {
        return Err(PublicationError::Conflict);
    }
    current
        .plan
        .assets
        .verify_current_assets_in(tx, &current.plan.asset_state)?;
    Ok(())
}

/// Read-only fence for publishing a selected snapshot's external association in
/// the same borrowed owner transaction. Does not change pin/position/generation,
/// regrant a source, or commit. The caller rolls back the whole transaction on
/// error and invokes this again immediately before committing its own records.
pub fn revalidate_current(
    tx: &Transaction<'_>,
    guard: &CurrentGuard<'_>,
    current: &VerifiedCurrent,
) -> PublicationResult<()> {
    // Preparation already adopted the current namespace. Refuse callback changes
    // to old/missing markers before guard_current's compatibility initializer can
    // run. With v2 pinned, that validator checks the complete exact table layouts,
    // singleton, rows and state, and has no schema-write path.
    let version: Option<i64> = tx
        .query_row(
            "SELECT version FROM device_document_schema_v1 WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| device::Error::InvalidSchema)?;
    match version {
        Some(2) => verify_live(tx, guard, current),
        Some(_) => Err(device::Error::UnsupportedSchema.into()),
        None => Err(device::Error::InvalidSchema.into()),
    }
}
