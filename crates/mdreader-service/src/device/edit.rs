//! Device copy editing; slow file/image work runs outside the owner mutex.
use super::*;
use base64::Engine;
use mdreader_core::device::publication as p;

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
        markdown_base64: String,
    ) -> Result<DeviceReply> {
        self.save_guarded(receipt,operation_id,markdown_base64,None)
    }
    pub(crate) fn save_agent(&self,receipt:catalog::Receipt,operation_id:String,markdown_base64:String,fence:crate::agents::WriteFence)->Result<DeviceReply>{
        self.save_guarded(receipt,operation_id,markdown_base64,Some(fence))
    }
    fn save_guarded(&self,receipt:catalog::Receipt,operation_id:String,markdown_base64:String,fence:Option<crate::agents::WriteFence>)->Result<DeviceReply>{
        let _saving = self.saving.try_lock().map_err(|_| DeviceError::Busy)?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(markdown_base64)
            .map_err(|_| DeviceError::Invalid)?;
        let markdown = String::from_utf8(decoded).map_err(|_| DeviceError::Invalid)?;
        let operation = uuid::Uuid::parse_str(&operation_id).map_err(|_| DeviceError::Invalid)?;
        let selection = p::EditSelection::new(receipt.clone(), operation, markdown.as_bytes())
            .map_err(error)?;
        let origin = selection.origin();
        let (assets, current, archive) = {
            let mut owner = self.owner()?;
            if let Some(fence)=&fence{owner.with_library_connection(|db|fence.check(db)).map_err(|_|DeviceError::Unavailable)?;}
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
        let input = archive.edited_input(markdown.into_bytes()).map_err(error)?;
        let plan = {
            let mut owner = self.owner()?;
            if let Some(fence)=&fence{owner.with_library_connection(|db|fence.check(db)).map_err(|_|DeviceError::Unavailable)?;}
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
        if let Some(fence)=&fence{owner.with_library_connection(|db|fence.check(db)).map_err(|_|DeviceError::Unavailable)?;}
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
    pub(crate) fn create_agent(&self,name:String,operation:uuid::Uuid,markdown:String,fence:crate::agents::WriteFence)->Result<DeviceReply>{
        let _admission=self.admission.begin()?;
        let _saving=self.saving.try_lock().map_err(|_|DeviceError::Busy)?;
        let mut owner=self.owner()?;
        owner.with_library_connection(|db|fence.check(db)).map_err(|_|DeviceError::Unavailable)?;
        // Names in the selected library are unique for agent creation.
        let documents=mdreader_core::device::list(&mut owner).map_err(document_error)?;
        if documents.iter().any(|d|d.name==name&&d.id!=operation.to_string()){return Err(DeviceError::Stale);}
        let saved=mdreader_core::device::import_selected(&mut owner,&self.documents,operation,name,markdown).map_err(document_error)?;
        owner.with_library_connection(|db|fence.check(db)).map_err(|_|DeviceError::Unavailable)?;
        let _=owner.queue_index(&saved.source_id);
        Ok(DeviceReply::Saved{receipt:catalog::receipt(&saved),name:saved.name})
    }
}
