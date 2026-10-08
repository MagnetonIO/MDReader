//! Scoped wire associations only; core remains the sole native document authority.
use super::{identity::Identity, types::*};
use rusqlite::{params, OptionalExtension, Transaction};
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::sync) struct Head {
    pub item: String,
    pub kind: String,
    pub revision: String,
    pub document: String,
    pub target: Option<String>,
    pub generation: Option<u64>,
    pub tombstone: Option<String>,
}
pub(super) fn get(tx: &Transaction<'_>, scope: &str, item: &str) -> Result<Option<Head>> {
    Ok(tx.query_row("SELECT wire_item,kind,head_revision,document_id,target_item,baseline_generation,tombstone_revision FROM native_incoming_heads_v2 WHERE scope=?1 AND wire_item=?2",params![scope,item],|r|Ok(Head {item:r.get(0)?,kind:r.get(1)?,revision:r.get(2)?,document:r.get(3)?,target:r.get(4)?,generation:r.get(5)?,tombstone:r.get(6)?})).optional()?)
}
pub(super) fn verify(tx: &Transaction<'_>, scope: &str, head: &Head) -> Result<()> {
    if get(tx, scope, &head.item)?.as_ref() != Some(head) {
        return Err(Error::Conflict);
    }
    Ok(())
}
pub(super) fn save(
    tx: &Transaction<'_>,
    scope: &str,
    old: Option<&Head>,
    new: &Head,
) -> Result<()> {
    if let Some(old) = old {
        verify(tx, scope, old)?;
    } else if get(tx, scope, &new.item)?.is_some() {
        return Err(Error::Conflict);
    }
    tx.execute("INSERT INTO native_incoming_heads_v2(scope,wire_item,kind,head_revision,document_id,target_item,baseline_generation,tombstone_revision) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(scope,wire_item) DO UPDATE SET kind=excluded.kind,head_revision=excluded.head_revision,document_id=excluded.document_id,target_item=excluded.target_item,baseline_generation=excluded.baseline_generation,tombstone_revision=excluded.tombstone_revision",params![scope,new.item,new.kind,new.revision,new.document,new.target,new.generation,new.tombstone])?;
    verify(tx, scope, new)
}
/// Preserve original v1 identity/receipt bytes. Unknown historic edit baselines
/// remain NULL; no current file/state observation is invented during migration.
pub(super) fn backfill(tx: &Transaction<'_>) -> Result<()> {
    let records=tx.prepare("SELECT scope,identity,receipt FROM native_incoming_items_v1 WHERE state='applied' ORDER BY sequence LIMIT 4097")?.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if records.len() > 4096 {
        return Err(Error::Limit);
    }
    for (scope, identity, receipt) in records {
        let identity: Identity = serde_json::from_str(&identity)?;
        let receipt: Receipt = serde_json::from_str(&receipt)?;
        super::ledger::validate_receipt(&identity, &receipt)?;
        save(
            tx,
            &scope,
            None,
            &Head {
                item: receipt.wire_item_id,
                kind: "content".into(),
                revision: receipt.wire_revision,
                document: receipt.document.id,
                target: None,
                generation: None,
                tombstone: None,
            },
        )?;
    }
    Ok(())
}
pub(super) fn parents(identity: &Identity, head: Option<&Head>) -> bool {
    match head {
        Some(head) => identity.change.parents == [head.revision.clone()],
        None => identity.change.parents.is_empty(),
    }
}
pub(super) fn receipt(tx: &Transaction<'_>, scope: &str, head: &Head) -> Result<Receipt> {
    let encoded:String=tx.query_row("SELECT receipt FROM native_incoming_items_v1 WHERE scope=?1 AND wire_revision=?2 AND state='applied'",params![scope,head.revision],|r|r.get(0))?;
    let receipt: Receipt = serde_json::from_str(&encoded)?;
    if receipt.document.id != head.document {
        return Err(Error::Conflict);
    }
    Ok(receipt)
}
/// Persist observed content deletion before any potentially conflicting native
/// transition. A held cursor cannot erase this anti-resurrection fence.
pub(super) fn observe_delete(
    tx: &Transaction<'_>,
    scope: &str,
    head: &Head,
    revision: &str,
) -> Result<Head> {
    let mut observed = head.clone();
    if observed
        .tombstone
        .as_deref()
        .is_some_and(|old| old != revision)
    {
        return Err(Error::Conflict);
    }
    observed.tombstone = Some(revision.into());
    save(tx, scope, Some(head), &observed)?;
    Ok(observed)
}

pub(super) fn content_admission(identity: &Identity, head: Option<&Head>) -> Option<PendingReason> {
    if !parents(identity, head) {
        Some(PendingReason::HeadConflict)
    } else if head.is_some_and(|head| head.kind != "content" || head.tombstone.is_some()) {
        Some(PendingReason::Tombstoned)
    } else if head.is_some_and(|head| head.generation.is_none()) {
        Some(PendingReason::BaselineUnknown)
    } else {
        None
    }
}

/// Native identity lookup, never raw-hash or title/path matching.
pub(super) fn for_document(
    tx: &Transaction<'_>,
    scope: &str,
    document: &str,
) -> Result<Option<Head>> {
    let items=tx.prepare("SELECT wire_item FROM native_incoming_heads_v2 WHERE scope=?1 AND document_id=?2 AND kind='content' LIMIT 2")?.query_map(params![scope,document],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    match items.as_slice() {
        [] => Ok(None),
        [item] => get(tx, scope, item),
        _ => Err(Error::Conflict),
    }
}
