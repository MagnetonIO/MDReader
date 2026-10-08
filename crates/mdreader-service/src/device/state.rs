//! Explicit local pin/line edits on the same retained device owner.
use super::*;
use mdreader_core::device::publication::{self, IncomingOrigin};
use uuid::Uuid;

impl Coordinator {
    pub(super) fn state(&self, receipt: catalog::Receipt) -> Result<DeviceReply> {
        let owner = self.owner()?;
        let body = catalog::read_in(&owner, &self.documents, &receipt).map_err(document_error)?;
        let state = catalog::state_in(&owner, &self.documents, &receipt).map_err(document_error)?;
        if state.document != body.document {
            return Err(DeviceError::Stale);
        }
        Ok(reply(
            receipt,
            &state,
            body.markdown.lines().count().max(1) as u32,
        ))
    }
    pub(super) fn set_state(
        &self,
        receipt: catalog::Receipt,
        generation: u64,
        pinned: Option<bool>,
        line: Option<u32>,
    ) -> Result<DeviceReply> {
        // A local domain operation, never a fabricated remote authorization.
        let origin = IncomingOrigin {
            scope_digest: [0; 32],
            item_id: Uuid::new_v4(),
            wire_revision: Uuid::new_v4(),
            session_epoch: Uuid::new_v4(),
            selection_epoch: Uuid::new_v4(),
        };
        let plan = {
            let mut owner = self.owner()?;
            self.check(&owner, &receipt)?;
            let state =
                catalog::state_in(&owner, &self.documents, &receipt).map_err(document_error)?;
            if state.generation != generation {
                return Err(DeviceError::Stale);
            }
            let assets = self
                .runtime
                .assets(&mut owner, &self.assets)
                .map_err(asset_error)?;
            publication::prepare_current(&mut owner, &receipt.document_id, &assets, &origin)
                .map_err(publication_error)?
        };
        // File/body verification happens off the live owner mutex.
        let current = plan.verify().map_err(publication_error)?;
        if current.state().generation != generation {
            return Err(DeviceError::Stale);
        }
        let mut owner = self.owner()?;
        self.check(&owner, &receipt)?;
        publication::with_transition(&mut owner, &current, &origin, |db, guard| -> Result<_> {
            let tx = db.transaction().map_err(|_| DeviceError::Persistence)?;
            let published = publication::publish_state(
                &tx,
                &guard,
                &current,
                publication::StateUpdate {
                    pinned: pinned.unwrap_or(current.state().document.pinned),
                    position: line
                        .map(publication::PositionUpdate::Line)
                        .unwrap_or(publication::PositionUpdate::Preserve),
                },
            )
            .map_err(publication_error)?;
            publication::revalidate_state(&tx, &guard, &current, &published)
                .map_err(publication_error)?;
            let result = reply(receipt, published.state(), current.max_line());
            tx.commit().map_err(|_| DeviceError::Persistence)?;
            Ok(result)
        })
        .map_err(publication_error)?
    }
}
fn reply(
    receipt: catalog::Receipt,
    state: &mdreader_core::device::state::DocumentState,
    line_end: u32,
) -> DeviceReply {
    DeviceReply::State {
        receipt,
        pinned: state.document.pinned,
        line: state.document.line,
        line_end,
        generation: state.generation,
    }
}
fn publication_error(error: publication::PublicationError) -> DeviceError {
    match error {
        publication::PublicationError::Device(error) => document_error(error),
        publication::PublicationError::Conflict => DeviceError::Stale,
        publication::PublicationError::InvalidInput => DeviceError::Invalid,
        publication::PublicationError::UnsupportedPlatform => DeviceError::Unsupported,
        _ => DeviceError::Unavailable,
    }
}
