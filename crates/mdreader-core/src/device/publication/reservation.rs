use super::{schema, types::*};
use crate::{
    device::{self, assets::AssetStore},
    ReaderStore, SourceRoot,
};
use rusqlite::{params, OptionalExtension, Transaction};
use std::path::Path;

pub(super) fn require_platform() -> PublicationResult<()> {
    if !cfg!(unix) {
        return Err(PublicationError::UnsupportedPlatform);
    }
    Ok(())
}
pub(super) fn origin_key(origin: &IncomingOrigin) -> PublicationResult<String> {
    if [
        origin.item_id,
        origin.wire_revision,
        origin.session_epoch,
        origin.selection_epoch,
    ]
    .iter()
    .any(|id| id.get_version_num() != 4)
    {
        return Err(PublicationError::InvalidInput);
    }
    Ok(serde_json::to_string(origin)?)
}
pub(super) fn load(tx: &Transaction<'_>, id: &str) -> PublicationResult<Intent> {
    let (mut intent, assets): (Intent,String) = tx.query_row(
        "SELECT id,origin,name,revision,bytes,input_digest,parent_identity,root_identity,assets_identity,assets,state FROM device_incoming_intents_v1 WHERE id=?1", [id],
        |r| Ok((Intent {id:r.get(0)?,origin:r.get(1)?,name:r.get(2)?,revision:r.get(3)?,bytes:r.get(4)?,digest:r.get(5)?,parent_identity:r.get(6)?,root_identity:r.get(7)?,assets_identity:r.get(8)?,assets:Vec::new(),state:r.get(10)?},r.get(9)?)),
    )?;
    if assets.len() > 16384
        || intent.bytes > 1048576
        || intent.origin.len() > 1024
        || intent.parent_identity.len() > 1024
        || intent.assets_identity.len() > 1024
        || intent
            .root_identity
            .as_ref()
            .is_some_and(|s| s.len() > 1024)
    {
        return Err(PublicationError::InvalidInput);
    }
    intent.assets = serde_json::from_str(&assets)?;
    let is_hash = |s: &str| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    };
    if uuid::Uuid::parse_str(&intent.id).is_err()
        || !is_hash(&intent.revision)
        || !is_hash(&intent.digest)
        || intent.assets.len() > 8
        || intent
            .assets
            .iter()
            .map(|a| u64::from(a.bytes))
            .sum::<u64>()
            > 16 * 1024 * 1024
        || intent.assets.iter().any(|a| {
            uuid::Uuid::parse_str(&a.id).is_err()
                || a.bytes == 0
                || a.bytes > 4194304
                || !is_hash(&a.hash)
        })
    {
        return Err(PublicationError::InvalidInput);
    }
    Ok(intent)
}
fn plan(
    store: &mut ReaderStore,
    parent: SourceRoot,
    assets: &AssetStore,
    mut intent: Intent,
) -> PublicationResult<WritePlan> {
    assets.incoming_owner(store)?;
    if intent.state != "reserved"
        || serde_json::to_string(&parent.identity)? != intent.parent_identity
        || serde_json::to_string(&assets.incoming_root().identity)? != intent.assets_identity
    {
        return Err(PublicationError::Conflict);
    }
    let root = if let Some(identity) = &intent.root_identity {
        SourceRoot::restore(
            parent.path.join(&intent.id),
            serde_json::from_str(identity)?,
        )?
    } else {
        let root = parent.create_or_capture_directory_relative(Path::new(&intent.id))?;
        parent.try_clone_directory_handle()?.sync_all()?;
        let identity = serde_json::to_string(&root.identity)?;
        store.with_library_connection(|db|->PublicationResult<()> {
            let tx=db.transaction()?;
            device::schema::initialize_in(&tx)?;
            schema::initialize(&tx)?;
            if tx.execute("UPDATE device_incoming_intents_v1 SET root_identity=?2 WHERE id=?1 AND state='reserved' AND root_identity IS NULL",params![intent.id,identity])?!=1 {return Err(PublicationError::Conflict);}
            tx.commit()?; Ok(())
        })?;
        intent.root_identity = Some(identity);
        root
    };
    parent.verify()?;
    root.verify()?;
    Ok(WritePlan {
        intent,
        parent,
        root,
        assets: assets.clone(),
        owner: store.device_owner_session(),
    })
}
/// Durable reservation precedes generated leaf/files; exact retry preserves its local UUID.
/// Expensive copies and decoder work happen through the returned owned plan.
pub fn reserve_new(
    store: &mut ReaderStore,
    directory: &Path,
    assets: &AssetStore,
    origin: &IncomingOrigin,
    input: &IncomingDocument,
) -> PublicationResult<WritePlan> {
    require_platform()?;
    device::schema(store)?;
    assets.incoming_owner(store)?;
    device::imports::private_directory(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(directory)?.permissions().mode() & 0o077 != 0 {
            return Err(device::Error::AuthorityChanged.into());
        }
    }
    let parent = SourceRoot::capture(directory.canonicalize()?)?;
    let key = origin_key(origin)?;
    let assets_identity = serde_json::to_string(&assets.incoming_root().identity)?;
    let parent_identity = serde_json::to_string(&parent.identity)?;
    let intent=store.with_library_connection(|db|->PublicationResult<Intent> {
        let tx=db.transaction()?;
        schema::initialize(&tx)?;
        let previous:Option<String>=tx.query_row("SELECT id FROM device_incoming_intents_v1 WHERE origin=?1",[&key],|r|r.get(0)).optional()?;
        if let Some(id)=previous {
            let previous=load(&tx,&id)?;
            if previous.digest!=input.digest || previous.state!="reserved" {return Err(PublicationError::Conflict);}
            tx.commit()?; return Ok(previous);
        }
        let (count,bytes):(u64,u64)=tx.query_row("SELECT count(*),coalesce(sum(bytes),0) FROM (SELECT bytes FROM device_documents_v1 UNION ALL SELECT bytes FROM device_import_intents_v1 WHERE state='reserved')",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
        let (incoming_count,incoming_bytes)=schema::reserved_usage(&tx)?;
        let records:u64=tx.query_row("SELECT count(*) FROM device_incoming_intents_v1",[],|r|r.get(0))?;
        if count+incoming_count>=1000 || bytes+incoming_bytes+input.markdown.len() as u64>64*1024*1024 {return Err(device::Error::Quota.into());}
        if records>=4096 {return Err(PublicationError::IntentLimit);}
        let id=uuid::Uuid::new_v4().to_string();
        let records=input.records();
        let value=serde_json::to_string(&records)?;
        assets.incoming_reserve(&tx,&id,&input.revision,&input.digest,&records)?;
        tx.execute("INSERT INTO device_incoming_intents_v1(id,origin,name,revision,bytes,input_digest,parent_identity,assets_identity,assets,state) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'reserved')",params![id,key,input.name,input.revision,input.markdown.len() as u32,input.digest,parent_identity,assets_identity,value])?;
        let intent=load(&tx,&id)?;
        tx.commit()?; Ok(intent)
    })?;
    plan(store, parent, assets, intent)
}
/// Select one persisted incoming intent explicitly; no directory enumeration or grant.
pub fn resume(
    store: &mut ReaderStore,
    directory: &Path,
    assets: &AssetStore,
    id: &str,
    origin: &IncomingOrigin,
) -> PublicationResult<WritePlan> {
    require_platform()?;
    uuid::Uuid::parse_str(id).map_err(|_| PublicationError::InvalidInput)?;
    device::schema(store)?;
    let intent = store.with_library_connection(|db| -> PublicationResult<Intent> {
        let tx = db.transaction()?;
        schema::initialize(&tx)?;
        let intent = load(&tx, id)?;
        if intent.origin != origin_key(origin)? {
            return Err(PublicationError::Conflict);
        }
        tx.commit()?;
        Ok(intent)
    })?;
    let parent = SourceRoot::restore(
        directory.canonicalize()?,
        serde_json::from_str(&intent.parent_identity)?,
    )?;
    plan(store, parent, assets, intent)
}
