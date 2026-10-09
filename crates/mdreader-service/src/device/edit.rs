//! Device copy editing; slow file/image work runs outside the owner mutex.
use super::*;
use mdreader_core::device::{assets, publication as p};

fn error(value: p::PublicationError) -> DeviceError {
    match value {
        p::PublicationError::Conflict => DeviceError::Stale,
        p::PublicationError::InvalidInput => DeviceError::Invalid,
        p::PublicationError::UnsupportedPlatform => DeviceError::Unsupported,
        p::PublicationError::IntentLimit => DeviceError::Quota,
        p::PublicationError::Device(mdreader_core::device::Error::Quota) => DeviceError::Quota,
        p::PublicationError::Device(value) => document_error(value),
        p::PublicationError::Asset(value) => asset_error(value),
        p::PublicationError::Incomplete => DeviceError::Unavailable,
    }
}
impl Coordinator {
    pub(super) fn save(
        &self,
        receipt: catalog::Receipt,
        operation_id: String,
        markdown: String,
    ) -> Result<DeviceReply> {
        let operation = uuid::Uuid::parse_str(&operation_id).map_err(|_| DeviceError::Invalid)?;
        let selection = p::EditSelection::new(receipt.clone(), operation, markdown.as_bytes())
            .map_err(error)?;
        let origin = selection.origin();
        let (assets, current, archive) = {
            let mut owner = self.owner()?;
            if let Some(saved) = p::replay_edit(&mut owner, &selection).map_err(error)? {
                self.check(&owner, &catalog::receipt(&saved))?;
                if owner
                    .index_job(&saved.source_id)
                    .is_ok_and(|job| job.is_none())
                {
                    let _ = owner.queue_index(&saved.source_id);
                }
                return Ok(DeviceReply::Saved {
                    receipt: catalog::receipt(&saved),
                    name: saved.name,
                });
            }
            self.check(&owner, &receipt)?;
            let assets = self
                .runtime
                .assets(&mut owner, &self.assets)
                .map_err(asset_error)?;
            let current = p::prepare_current(&mut owner, &receipt.document_id, &assets, &origin)
                .map_err(error)?;
            let archive =
                p::prepare_archive(&mut owner, &receipt.document_id, &assets, &self.runtime)
                    .map_err(error)?;
            (assets, current, archive)
        };
        let current = current.verify().map_err(error)?;
        let archive = archive.read().map_err(error)?;
        let targets = assets::incoming_targets(&markdown);
        let mut copied = Vec::new();
        for asset in archive.assets() {
            // No incomplete attachment is silently lost; the old archive remains.
            if asset.state != "ready"
                || asset.received != asset.expected_bytes
                || asset.bytes().is_none()
            {
                return Err(DeviceError::Unavailable);
            }
            if targets.contains(&asset.target) {
                copied.push(
                    p::IncomingAsset::new(
                        asset.target.clone(),
                        asset.mime.clone().ok_or(DeviceError::Unavailable)?,
                        asset.bytes().ok_or(DeviceError::Unavailable)?.to_vec(),
                    )
                    .map_err(error)?,
                );
            }
        }
        let input = p::IncomingDocument::new(
            archive.state().document.name.clone(),
            markdown.into_bytes(),
            copied,
        )
        .map_err(error)?;
        let plan = {
            let mut owner = self.owner()?;
            if let Some(saved) = p::replay_edit(&mut owner, &selection).map_err(error)? {
                self.check(&owner, &catalog::receipt(&saved))?;
                return Ok(DeviceReply::Saved {
                    receipt: catalog::receipt(&saved),
                    name: saved.name,
                });
            }
            self.check(&owner, &receipt)?;
            p::reserve_new(&mut owner, &self.documents, &assets, &origin, &input).map_err(error)?
        };
        let ready = plan
            .write(input)
            .map_err(error)?
            .validate(self.runtime.validation_permit().map_err(asset_error)?)
            .map_err(error)?;
        let mut owner = self.owner()?;
        if let Some(saved) = p::replay_edit(&mut owner, &selection).map_err(error)? {
            self.check(&owner, &catalog::receipt(&saved))?;
            return Ok(DeviceReply::Saved {
                receipt: catalog::receipt(&saved),
                name: saved.name,
            });
        }
        self.check(&owner, &receipt)?;
        let saved =
            p::commit_edit(&mut owner, &ready, &current, &archive, &selection).map_err(error)?;
        let _ = owner.queue_index(&saved.source_id);
        Ok(DeviceReply::Saved {
            receipt: catalog::receipt(&saved),
            name: saved.name,
        })
    }
}
