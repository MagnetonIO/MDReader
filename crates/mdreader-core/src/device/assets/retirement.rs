//! Transactional revocation retains exact files and charges every archived byte.
use super::*;
use rusqlite::Transaction;

#[derive(Clone)]
pub(in crate::device) struct CurrentAssets {
    doc: String,
    revision: String,
    items: Vec<Item>,
    generation: Option<u64>,
}
pub(in crate::device) struct ArchivedAssetPlan {
    pub id: String,
    pub target: String,
    pub expected_bytes: u32,
    pub received: u32,
    pub state: String,
    pub hash: Option<String>,
    pub mime: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub root: SourceRoot,
}
impl AssetStore {
    pub(in crate::device) fn current_assets_in(
        &self,
        tx: &Transaction<'_>,
        doc: &str,
        revision: &str,
    ) -> Result<CurrentAssets> {
        self.incoming_verify_tx(tx)?;
        // Bound metadata before extracting owned strings, including prototype
        // tables whose original CHECK clauses were not reliably validated.
        let invalid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM device_asset_items_v1 WHERE document_id=?1 AND revision=?2 AND (length(CAST(id AS BLOB))!=36 OR length(CAST(target AS BLOB))>512 OR length(CAST(expected_manifest AS BLOB))>128 OR length(CAST(coalesce(hash,'') AS BLOB))>64 OR length(CAST(coalesce(mime,'') AS BLOB))>32 OR state NOT IN('ready','reserved','retired') OR bytes NOT BETWEEN 1 AND 4194304 OR received NOT BETWEEN 0 AND bytes))",params![doc,revision],|r|r.get(0)).map_err(sql)?;
        if invalid {
            return Err(unavailable());
        }
        let items=tx.prepare(&format!("SELECT {ITEM_COLUMNS} FROM device_asset_items_v1 WHERE document_id=?1 AND revision=?2 ORDER BY target,id LIMIT 513")).map_err(sql)?.query_map(params![doc,revision],item_row).map_err(sql)?.collect::<rusqlite::Result<Vec<_>>>().map_err(sql)?;
        for item in &items {
            valid_id(&item.id)?;
            if canonical_target(&item.target)? != item.target {
                return Err(unavailable());
            }
        }
        if items.len() > 512
            || items
                .iter()
                .any(|item| item.bytes == 0 || item.bytes > MAX_FILE || item.received > item.bytes)
            || items.iter().map(|item| u64::from(item.bytes)).sum::<u64>() > MAX_DOC_BYTES
        {
            return Err(Error::Quota);
        }
        let generation=tx.query_row("SELECT generation FROM device_asset_generations_v1 WHERE document_id=?1 AND revision=?2",params![doc,revision],|r|r.get(0)).optional().map_err(sql)?;
        Ok(CurrentAssets {
            doc: doc.into(),
            revision: revision.into(),
            items,
            generation,
        })
    }
    pub(in crate::device) fn verify_current_assets_in(
        &self,
        tx: &Transaction<'_>,
        expected: &CurrentAssets,
    ) -> Result<()> {
        let current = self.current_assets_in(tx, &expected.doc, &expected.revision)?;
        if current.items != expected.items || current.generation != expected.generation {
            return Err(Error::ManifestChanged);
        }
        Ok(())
    }
    pub(in crate::device) fn retire_current_assets_in(
        &self,
        tx: &Transaction<'_>,
        expected: &CurrentAssets,
    ) -> Result<()> {
        self.verify_current_assets_in(tx, expected)?;
        let changed=tx.execute("UPDATE device_asset_items_v1 SET state='retired' WHERE document_id=?1 AND revision=?2 AND state IN('ready','reserved')",params![expected.doc,expected.revision]).map_err(sql)?;
        if changed > 0 {
            Self::advance(tx, &expected.doc, &expected.revision).map_err(sql)?;
        }
        self.verify_retired_assets_in(tx, expected)
    }
    pub(in crate::device) fn verify_retired_assets_in(
        &self,
        tx: &Transaction<'_>,
        expected: &CurrentAssets,
    ) -> Result<()> {
        let mut retired = expected.clone();
        let mut changed = false;
        for item in &mut retired.items {
            if item.state == "ready" || item.state == "reserved" {
                item.state = "retired".into();
                changed = true;
            }
        }
        if changed {
            retired.generation = Some(
                retired
                    .generation
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or(Error::ManifestChanged)?,
            );
        }
        self.verify_current_assets_in(tx, &retired)
    }
    pub(in crate::device) fn archived_asset_plans(
        &self,
        current: &CurrentAssets,
    ) -> Vec<ArchivedAssetPlan> {
        current
            .items
            .iter()
            .map(|item| ArchivedAssetPlan {
                id: item.id.clone(),
                target: item.target.clone(),
                expected_bytes: item.bytes,
                received: item.received,
                state: item.state.clone(),
                hash: item.hash.clone(),
                mime: item.mime.clone(),
                width: item.width,
                height: item.height,
                root: self.root.clone(),
            })
            .collect()
    }
}
