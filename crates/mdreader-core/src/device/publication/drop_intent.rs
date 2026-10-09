use super::{
    reservation::{load, origin_key, require_platform},
    schema,
    types::*,
};
use crate::{device, device::assets::AssetStore, ReaderStore, SourceRoot};
use rusqlite::{params, Transaction};
use std::path::Path;

/// Caller must hold the selected owner's single operation lease and quiesce all
/// previously issued WritePlan file I/O before cleanup/finish. Durable `dropping`
/// fences publication, but cannot cancel an already running filesystem writer.
/// The core owner adds no global runtime or independent lock authority.
pub struct DropPlan {
    intent: Intent,
    parent: SourceRoot,
    root: SourceRoot,
    assets: AssetStore,
    owner: std::sync::Arc<()>,
}
pub struct RemovedIncoming {
    plan: DropPlan,
}
fn no_document(tx: &Transaction<'_>, id: &str) -> PublicationResult<()> {
    let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM device_documents_v1 WHERE id=?1 UNION ALL SELECT 1 FROM device_import_intents_v1 WHERE id=?1)",[id],|r|r.get(0))?;
    if exists {
        return Err(PublicationError::Conflict);
    }
    Ok(())
}
/// Fence publication durably before explicit filesystem cleanup. This does not
/// clear a published document, reactivate a source or discover untracked files.
pub fn prepare_drop(
    store: &mut ReaderStore,
    directory: &Path,
    assets: &AssetStore,
    id: &str,
    origin: &IncomingOrigin,
) -> PublicationResult<DropPlan> {
    require_platform()?;
    assets.incoming_owner(store)?;
    device::schema(store)?;
    let intent = store.with_library_connection(|db| -> PublicationResult<Intent> {
        let tx = db.transaction()?;
        schema::initialize(&tx)?;
        let mut intent = load(&tx, id)?;
        if intent.origin != origin_key(origin)?
            || !matches!(intent.state.as_str(), "reserved" | "dropping")
        {
            return Err(PublicationError::Conflict);
        }
        no_document(&tx, id)?;
        assets.incoming_retire(&tx, id, &intent.digest, &intent.assets)?;
        tx.execute(
            "UPDATE device_incoming_intents_v1 SET state='dropping' WHERE id=?1",
            [id],
        )?;
        intent.state = "dropping".into();
        tx.commit()?;
        Ok(intent)
    })?;
    let parent = SourceRoot::restore(
        directory.canonicalize()?,
        serde_json::from_str(&intent.parent_identity)?,
    )?;
    let root = SourceRoot::restore(
        parent.path.join(id),
        serde_json::from_str(
            intent
                .root_identity
                .as_deref()
                .ok_or(PublicationError::Incomplete)?,
        )?,
    )?;
    if serde_json::to_string(&assets.incoming_root().identity)? != intent.assets_identity {
        return Err(PublicationError::Conflict);
    }
    Ok(DropPlan {
        intent,
        parent,
        root,
        assets: assets.clone(),
        owner: store.device_owner_session(),
    })
}
impl DropPlan {
    /// Only persisted generated filenames are removed, never recursive cleanup.
    /// Unknown files/empty leaves are not grants and are left intact.
    pub fn remove_files(self) -> PublicationResult<RemovedIncoming> {
        self.parent.verify()?;
        self.root.verify()?;
        #[cfg(windows)]
        mdreader_capture::PrivateDirectory::open(&self.root.path)
            .map_err(|_| device::Error::AuthorityChanged)?
            .remove_projection_file(Path::new("document.md"), u64::from(self.intent.bytes))
            .map_err(|_| device::Error::AuthorityChanged)?;
        #[cfg(not(windows))]
        if self
            .root
            .open_optional_file_relative(Path::new("document.md"))?
            .is_some()
        {
            self.root.remove_file_relative(Path::new("document.md"))?;
        }
        let root = self.assets.incoming_root();
        root.verify()?;
        for asset in &self.intent.assets {
            let filename = format!("{}.bin", asset.id);
            #[cfg(windows)]
            mdreader_capture::PrivateDirectory::open(&root.path)
                .map_err(|_| device::Error::AuthorityChanged)?
                .remove_projection_file(Path::new(&filename), u64::from(asset.bytes))
                .map_err(|_| device::Error::AuthorityChanged)?;
            #[cfg(not(windows))]
            if root
                .open_optional_file_relative(Path::new(&filename))?
                .is_some()
            {
                root.remove_file_relative(Path::new(&filename))?;
            }
        }
        #[cfg(unix)]
        {
            self.root.try_clone_directory_handle()?.sync_all()?;
            root.try_clone_directory_handle()?.sync_all()?;
        }
        self.parent.verify()?;
        self.root.verify()?;
        root.verify()?;
        Ok(RemovedIncoming { plan: self })
    }
}
/// Release quotas only after generated copies were removed and fsynced, with all
/// issued materialization writers quiescent under the caller's operation lease.
/// A lost reply is reconciled by origin/state inspection, never re-publication.
pub fn finish_drop(store: &mut ReaderStore, removed: RemovedIncoming) -> PublicationResult<()> {
    let plan = removed.plan;
    if !std::sync::Arc::ptr_eq(&plan.owner, &store.device_owner_session()) {
        return Err(PublicationError::Conflict);
    }
    plan.assets.incoming_owner(store)?;
    plan.parent.verify()?;
    plan.root.verify()?;
    if plan
        .root
        .open_optional_file_relative(Path::new("document.md"))?
        .is_some()
    {
        return Err(PublicationError::Conflict);
    }
    for asset in &plan.intent.assets {
        if plan
            .assets
            .incoming_root()
            .open_optional_file_relative(Path::new(&format!("{}.bin", asset.id)))?
            .is_some()
        {
            return Err(PublicationError::Conflict);
        }
    }
    store.with_library_connection(|db|->PublicationResult<()> {
        let tx=db.transaction()?;device::schema::initialize_in(&tx)?;schema::initialize(&tx)?;
        let intent=load(&tx,&plan.intent.id)?;
        if intent!=plan.intent || intent.state!="dropping" {return Err(PublicationError::Conflict);}
        no_document(&tx,&intent.id)?;
        #[cfg(windows)]
        super::windows_input::remove(&tx,&intent)?;
        plan.assets.incoming_delete(&tx,&intent.id,&intent.assets)?;
        if tx.execute("UPDATE device_incoming_intents_v1 SET state='dropped' WHERE id=?1 AND state='dropping' AND origin=?2",params![intent.id,intent.origin])?!=1 {return Err(PublicationError::Conflict);}
        tx.commit()?;Ok(())
    })
}
