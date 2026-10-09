use super::{identity::Identity, schema, types::*};
use mdreader_core::device::publication::IncomingOrigin;
use rusqlite::{params, OptionalExtension, Transaction};

pub(super) enum Entry {
    Reserved,
    Applied(Box<Receipt>),
    Pending(PendingReason),
}
pub(super) fn inspect(
    tx: &Transaction<'_>,
    scope: &str,
    identity: &Identity,
    origin: &IncomingOrigin,
) -> Result<Option<Entry>> {
    schema::initialize(tx)?;
    let row:Option<(String,String,String,Option<String>)>=tx.query_row("SELECT identity,origin,state,receipt FROM native_incoming_items_v1 WHERE scope=?1 AND wire_revision=?2",params![scope,identity.change.revision_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    if let Some((stored, stored_origin, state, receipt)) = row {
        if stored != identity.encode()? {
            return Err(Error::Conflict);
        }
        if state == "applied" {
            let receipt: Receipt = serde_json::from_str(receipt.as_deref().ok_or(Error::Schema)?)?;
            validate_receipt(identity, &receipt)?;
            return Ok(Some(Entry::Applied(Box::new(receipt))));
        }
        if stored_origin != serde_json::to_string(origin)? {
            return Ok(Some(Entry::Pending(PendingReason::ReauthorizationRequired)));
        }
        return Ok(Some(Entry::Reserved));
    }
    Ok(None)
}
pub(super) fn reserve(
    tx: &Transaction<'_>,
    scope: &str,
    identity: &Identity,
    origin: &IncomingOrigin,
) -> Result<()> {
    let count: u64 = tx.query_row("SELECT count(*) FROM native_incoming_items_v1", [], |r| {
        r.get(0)
    })?;
    if count >= 4096 {
        return Err(Error::Limit);
    }
    tx.execute("INSERT INTO native_incoming_items_v1(scope,wire_revision,wire_item,sequence,identity,origin,state) VALUES(?1,?2,?3,?4,?5,?6,'reserved')",params![scope,identity.change.revision_id,identity.change.item_id,identity.change.sequence,identity.encode()?,serde_json::to_string(origin)?])?;
    Ok(())
}
pub(super) fn associate(
    tx: &Transaction<'_>,
    scope: &str,
    identity: &Identity,
    origin: &IncomingOrigin,
    id: &str,
) -> Result<()> {
    if tx.execute("UPDATE native_incoming_items_v1 SET document_id=?1 WHERE scope=?2 AND wire_revision=?3 AND identity=?4 AND origin=?5 AND state='reserved' AND (document_id IS NULL OR document_id=?1)",params![id,scope,identity.change.revision_id,identity.encode()?,serde_json::to_string(origin)?])?!=1 { return Err(Error::Conflict); }
    Ok(())
}
pub(super) fn complete(
    tx: &Transaction<'_>,
    scope: &str,
    identity: &Identity,
    origin: &IncomingOrigin,
    receipt: &Receipt,
) -> Result<()> {
    validate_receipt(identity, receipt)?;
    let value = serde_json::to_string(receipt)?;
    if value.len() > 32768 {
        return Err(Error::Limit);
    }
    if tx.execute("UPDATE native_incoming_items_v1 SET state='applied',receipt=?1 WHERE scope=?2 AND wire_revision=?3 AND document_id=?4 AND identity=?5 AND origin=?6 AND state='reserved'",params![value,scope,identity.change.revision_id,receipt.document.id,identity.encode()?,serde_json::to_string(origin)?])?!=1 { return Err(Error::Conflict); }
    Ok(())
}
pub(super) fn verify(
    tx: &Transaction<'_>,
    scope: &str,
    identity: &Identity,
    origin: &IncomingOrigin,
    receipt: &Receipt,
) -> Result<()> {
    let row:Option<(String,String,String,String,String)>=tx.query_row("SELECT identity,origin,state,document_id,receipt FROM native_incoming_items_v1 WHERE scope=?1 AND wire_revision=?2",params![scope,identity.change.revision_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    if row
        != Some((
            identity.encode()?,
            serde_json::to_string(origin)?,
            "applied".into(),
            receipt.document.id.clone(),
            serde_json::to_string(receipt)?,
        ))
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
pub(super) fn validate_receipt(identity: &Identity, receipt: &Receipt) -> Result<()> {
    if receipt.sequence != identity.change.sequence
        || receipt.wire_item_id != identity.change.item_id
        || receipt.wire_revision != identity.change.revision_id
        || (receipt.operation == Operation::Content
            && (receipt.document.name != super::identity::local_name(&identity.title)
                || receipt.markdown_sha256 != identity.markdown_sha256
                || receipt.assets != identity.assets))
        || (!identity.deleted
            && matches!(
                receipt.operation,
                Operation::UserState | Operation::UserStatePreserved
            )
            && identity.user_state.is_none())
        || (identity.deleted
            != matches!(
                receipt.operation,
                Operation::DocumentDeleted | Operation::UserStateDeleted
            ))
        || uuid::Uuid::parse_str(&receipt.document.id).is_err()
        || (!identity.deleted
            && ((identity.user_state.is_some()
                && !matches!(
                    receipt.operation,
                    Operation::UserState | Operation::UserStatePreserved
                ))
                || (identity.user_state.is_none() && receipt.operation != Operation::Content)))
        || (receipt.operation == Operation::DocumentDeleted) != receipt.retirement.is_some()
        || receipt
            .generation
            .is_some_and(|value| value > 9_007_199_254_740_991)
        || receipt.document.revision.len() != 64
        || receipt.document.source_id.is_empty()
        || receipt.document.source_id.len() > 256
    {
        return Err(Error::Conflict);
    }
    let assets = if receipt.operation == Operation::Content {
        &identity.assets
    } else {
        &receipt.assets
    };
    if receipt.materialized_assets.len() != assets.len()
        || receipt
            .materialized_assets
            .iter()
            .zip(assets)
            .any(|(a, expected)| {
                &a.identity != expected
                    || uuid::Uuid::parse_str(&a.asset_id).is_err()
                    || a.width == 0
                    || a.height == 0
                    || a.width > 4096
                    || a.height > 4096
                    || u64::from(a.width) * u64::from(a.height) > 4_000_000
            })
    {
        return Err(Error::Conflict);
    }
    Ok(())
}
