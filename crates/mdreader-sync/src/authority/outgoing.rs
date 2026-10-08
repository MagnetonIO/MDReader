//! Native custody seals a selected snapshot; no raw key or active-key guessing.
use super::*;
use crate::{
    outgoing::{self, OutgoingContent, SealedGraph},
    EncryptionContext, LibraryKey,
};
impl NativeAuthority {
    /// The service still must atomically admit the graph with its exact selected
    /// native-document association. This method grants no owner/selection access.
    /// Key rotation has no accepted active-key policy yet: multiple keys fail closed.
    pub fn seal_selected_content(
        &self,
        item: &str,
        revision: &str,
        input: OutgoingContent<'_>,
    ) -> AuthorityResult<SealedGraph> {
        self.check()?;
        if self.record.keys.len() != 1 {
            return Err(AuthorityError::Unsupported);
        }
        let (id, raw) = self
            .record
            .keys
            .first_key_value()
            .ok_or(AuthorityError::Custody)?;
        let key =
            LibraryKey::from_bytes(id.clone(), raw.clone()).map_err(|_| AuthorityError::Custody)?;
        let context = EncryptionContext {
            library_id: self.record.scope.library_id().into(),
            item_id: item.into(),
            revision_id: revision.into(),
            key_id: id.clone(),
        };
        let graph = outgoing::seal_content(&self.record.scope, &key, &context, input).map_err(
            |e| match e {
                crate::SyncError::Bounds => AuthorityError::Bounds,
                _ => AuthorityError::Scope,
            },
        )?;
        self.check()?;
        Ok(graph)
    }
    pub fn seal_selected_state(
        &self,
        item: &str,
        revision: &str,
        input: &crate::codec::UserStatePayload,
    ) -> AuthorityResult<SealedGraph> {
        self.check()?;
        if self.record.keys.len() != 1 {
            return Err(AuthorityError::Unsupported);
        }
        let (id, raw) = self
            .record
            .keys
            .first_key_value()
            .ok_or(AuthorityError::Custody)?;
        let key =
            LibraryKey::from_bytes(id.clone(), raw.clone()).map_err(|_| AuthorityError::Custody)?;
        let context = EncryptionContext {
            library_id: self.record.scope.library_id().into(),
            item_id: item.into(),
            revision_id: revision.into(),
            key_id: id.clone(),
        };
        let graph =
            outgoing::seal_user_state(&self.record.scope, &key, &context, input).map_err(|e| {
                match e {
                    crate::SyncError::Bounds => AuthorityError::Bounds,
                    _ => AuthorityError::Scope,
                }
            })?;
        self.check()?;
        Ok(graph)
    }
}
