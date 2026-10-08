use super::{
    reservation::{load, origin_key},
    schema,
    types::*,
};
use crate::{device, publish_local_source, ReaderStore};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::marker::PhantomData;

/// Cannot escape the scoped live-owner connection borrow or survive owner reopen.
pub struct PublicationGuard<'owner> {
    connection: usize,
    origin: String,
    owner: std::sync::Arc<()>,
    _borrow: PhantomData<&'owner mut Connection>,
}
/// Bind publication capability to this exact live owner immediately before the
/// caller attaches sync to its borrowed connection. No lock/borrow across decoding.
/// The action must run revalidate_publication in its final transaction check,
/// after receipt/cursor writes and immediately before commit; never commit on error.
pub fn with_publication<T, E>(
    store: &mut ReaderStore,
    ready: &ReadyIncoming,
    current: &IncomingOrigin,
    action: impl for<'owner> FnOnce(
        &'owner mut Connection,
        PublicationGuard<'owner>,
    ) -> std::result::Result<T, E>,
) -> PublicationResult<std::result::Result<T, E>> {
    if !std::sync::Arc::ptr_eq(&ready.plan.owner, &store.device_owner_session())
        || ready.plan.intent.origin != origin_key(current)?
    {
        return Err(PublicationError::Conflict);
    }
    ready.plan.assets.incoming_owner(store)?;
    ready.plan.parent.verify()?;
    ready.plan.root.verify()?;
    for file in &ready.evidence {
        file.verify()?;
    }
    let origin = origin_key(current)?;
    Ok(store.with_library_connection(|db| {
        let guard = PublicationGuard {
            connection: db as *const Connection as usize,
            origin,
            owner: ready.plan.owner.clone(),
            _borrow: PhantomData,
        };
        action(db, guard)
    }))
}
/// Borrowed-transaction publication only. Caller MUST drop/rollback the entire
/// transaction on error; success/receipt is provisional until its durable commit.
/// No source grant from orphan files; no commit, cursor update or indexing here.
pub fn publish_new(
    tx: &Transaction<'_>,
    guard: &PublicationGuard<'_>,
    ready: &ReadyIncoming,
) -> PublicationResult<PublicationReceipt> {
    if &**tx as *const Connection as usize != guard.connection
        || !std::sync::Arc::ptr_eq(&guard.owner, &ready.plan.owner)
    {
        return Err(PublicationError::Conflict);
    }
    device::schema::initialize_in(tx)?;
    schema::initialize(tx)?;
    let intent = load(tx, &ready.plan.intent.id)?;
    if intent.state != "reserved" || intent.origin != guard.origin || intent != ready.plan.intent {
        return Err(PublicationError::Conflict);
    }
    let occupied:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM device_documents_v1 WHERE id=?1 UNION ALL SELECT 1 FROM device_import_intents_v1 WHERE id=?1)",[&intent.id],|r|r.get(0))?;
    if occupied {
        return Err(PublicationError::Conflict);
    }
    ready.plan.parent.verify()?;
    ready.plan.root.verify()?;
    ready.plan.assets.incoming_verify_tx(tx)?;
    for file in &ready.evidence {
        file.verify()?;
    }
    let (source, ()) =
        publish_local_source(tx, &intent.name, ready.plan.root.clone(), |_, source| {
            if !source.enabled {
                return Err(crate::ReaderError::Unavailable);
            }
            Ok(())
        })
        .map_err(device::Error::from)?;
    tx.execute(
        "INSERT INTO device_documents_v1(id,name,source_id,revision,bytes) VALUES(?1,?2,?3,?4,?5)",
        params![
            intent.id,
            intent.name,
            source.id,
            intent.revision,
            intent.bytes
        ],
    )?;
    let assets = ready.plan.assets.incoming_publish(
        tx,
        &intent.id,
        &intent.revision,
        &intent.digest,
        &intent.assets,
        &ready.rasters,
    )?;
    tx.execute("INSERT INTO device_import_intents_v1(id,name,revision,bytes,parent_identity,root_identity,state) VALUES(?1,?2,?3,?4,?5,?6,'published')",params![intent.id,intent.name,intent.revision,intent.bytes,intent.parent_identity,intent.root_identity])?;
    if tx.execute("UPDATE device_incoming_intents_v1 SET state='published',source_id=?2 WHERE id=?1 AND state='reserved' AND origin=?3 AND input_digest=?4",params![intent.id,source.id,guard.origin,intent.digest])?!=1 {return Err(PublicationError::Conflict);}
    // Final checks occur after all authority writes, before returning to short SQL
    // association/receipt/cursor writes in the same caller-owned transaction.
    ready.plan.parent.verify()?;
    ready.plan.root.verify()?;
    ready.plan.assets.incoming_verify_tx(tx)?;
    for file in &ready.evidence {
        file.verify()?;
    }
    let enabled: Option<bool> = tx
        .query_row(
            "SELECT enabled FROM sources WHERE id=?1",
            [&source.id],
            |r| r.get(0),
        )
        .optional()?;
    if enabled != Some(true) {
        return Err(PublicationError::Conflict);
    }
    Ok(PublicationReceipt {
        document: device::DeviceDocument {
            id: intent.id,
            name: intent.name,
            source_id: source.id,
            revision: intent.revision,
            bytes: intent.bytes,
            pinned: false,
            line: 1,
            indexed: false,
        },
        assets,
    })
}

/// Call after ALL association/receipt/cursor writes, immediately before COMMIT.
/// Checks final authority without decoding or hashing original bodies under lock.
pub fn revalidate_publication(
    tx: &Transaction<'_>,
    guard: &PublicationGuard<'_>,
    ready: &ReadyIncoming,
) -> PublicationResult<()> {
    revalidate_with_state(tx, guard, ready, false, 1, 0)
}
pub(super) fn revalidate_with_state(
    tx: &Transaction<'_>,
    guard: &PublicationGuard<'_>,
    ready: &ReadyIncoming,
    pinned: bool,
    line: u32,
    generation: u64,
) -> PublicationResult<()> {
    if &**tx as *const Connection as usize != guard.connection
        || !std::sync::Arc::ptr_eq(&guard.owner, &ready.plan.owner)
    {
        return Err(PublicationError::Conflict);
    }
    device::schema::initialize_in(tx)?;
    schema::initialize(tx)?;
    let current = load(tx, &ready.plan.intent.id)?;
    let mut expected = ready.plan.intent.clone();
    expected.state = "published".into();
    if current != expected || current.origin != guard.origin {
        return Err(PublicationError::Conflict);
    }
    ready.plan.parent.verify()?;
    crate::store::validate_root(tx, &ready.plan.root).map_err(device::Error::from)?;
    for file in &ready.evidence {
        file.verify()?;
    }
    ready.plan.assets.incoming_verify_tx(tx)?;
    let source_id: String = tx.query_row(
        "SELECT source_id FROM device_incoming_intents_v1 WHERE id=?1",
        [&current.id],
        |r| r.get(0),
    )?;
    let (name, path, identity, enabled): (String, Vec<u8>, String, bool) = tx.query_row(
        "SELECT name,path,identity,enabled FROM sources WHERE id=?1",
        [&source_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    if !source_path_matches(&path, &ready.plan.root.path)? {
        return Err(PublicationError::Conflict);
    }
    let expected_source =
        blake3::hash(serde_json::to_string(&ready.plan.root.identity)?.as_bytes())
            .to_hex()
            .to_string();
    if source_id != expected_source {
        return Err(PublicationError::Conflict);
    }
    if name != current.name
        || !enabled
        || identity != serde_json::to_string(&ready.plan.root.identity)?
    {
        return Err(PublicationError::Conflict);
    }
    let exact:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM device_documents_v1 WHERE id=?1 AND source_id=?2 AND name=?3 AND revision=?4 AND bytes=?5 AND pinned=?6 AND line=?7 AND indexed=0 AND state_generation=?8 AND retired=0 AND retirement IS NULL)",params![current.id,source_id,current.name,current.revision,current.bytes,pinned,line,generation],|r|r.get(0))?;
    let import:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM device_import_intents_v1 WHERE id=?1 AND name=?2 AND revision=?3 AND bytes=?4 AND parent_identity=?5 AND root_identity=?6 AND state='published')",params![current.id,current.name,current.revision,current.bytes,current.parent_identity,current.root_identity],|r|r.get(0))?;
    if !exact || !import {
        return Err(PublicationError::Conflict);
    }
    ready.plan.assets.incoming_revalidate(
        tx,
        &current.id,
        &current.revision,
        &current.digest,
        &current.assets,
        &ready.rasters,
    )?;
    Ok(())
}

fn source_path_matches(encoded: &[u8], path: &std::path::Path) -> PublicationResult<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(encoded == path.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        let _ = (encoded, path);
        Err(device::Error::AuthorityChanged.into())
    }
}
