use super::types::*;
use mdreader_core::device::publication::{IncomingAsset, IncomingDocument, IncomingOrigin};
use mdreader_sync::{
    incoming::{IncomingPayload, PreparedIncoming},
    RemoteChange, SyncScope,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    pub change: RemoteChange,
    pub title: String,
    pub markdown_sha256: String,
    pub assets: Vec<AssetIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_state: Option<mdreader_sync::codec::UserStatePayload>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
}
impl Identity {
    pub fn from_prepared(input: &PreparedIncoming) -> Result<Self> {
        let (title, markdown_sha256, assets, user_state, deleted) = match input.payload() {
            IncomingPayload::Content(content) => (
                content.title().to_owned(),
                content.sha256().to_owned(),
                content
                    .assets()
                    .iter()
                    .map(|a| AssetIdentity {
                        target: a.path().into(),
                        mime: a.mime().into(),
                        sha256: a.sha256().into(),
                        bytes: a.bytes().len() as u32,
                    })
                    .collect(),
                None,
                false,
            ),
            IncomingPayload::UserState(state) => (
                String::new(),
                String::new(),
                Vec::new(),
                Some(state.clone()),
                false,
            ),
            IncomingPayload::OpaqueTombstone => {
                (String::new(), String::new(), Vec::new(), None, true)
            }
        };
        let identity = Self {
            change: input.change().clone(),
            title,
            markdown_sha256,
            assets,
            user_state,
            deleted,
        };
        if identity.encode()?.len() > 16384 {
            return Err(Error::Limit);
        }
        Ok(identity)
    }
    pub fn encode(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }
}
pub(super) fn document(input: &PreparedIncoming) -> Result<IncomingDocument> {
    let IncomingPayload::Content(c) = input.payload() else {
        return Err(Error::Conflict);
    };
    let assets = c
        .assets()
        .iter()
        .map(|a| IncomingAsset::new(a.path().into(), a.mime().into(), a.bytes().to_vec()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(IncomingDocument::new(
        c.title().into(),
        c.markdown().to_vec(),
        assets,
    )?)
}
pub(in crate::sync) fn scope_key(scope: &SyncScope) -> Result<String> {
    Ok(mdreader_sync::blob_digest(&serde_json::to_vec(&(
        "mdreader-native-incoming",
        1,
        scope.account_digest(),
        scope.library_id(),
        scope.device_id(),
    ))?))
}
pub(super) fn origin(
    scope: &SyncScope,
    input: &PreparedIncoming,
    session: uuid::Uuid,
    selection: uuid::Uuid,
) -> Result<IncomingOrigin> {
    let key = scope_key(scope)?;
    let mut digest = [0; 32];
    for (i, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&key[i * 2..i * 2 + 2], 16).map_err(|_| Error::Scope)?;
    }
    Ok(IncomingOrigin {
        scope_digest: digest,
        item_id: uuid::Uuid::parse_str(&input.change().item_id).map_err(|_| Error::Conflict)?,
        wire_revision: uuid::Uuid::parse_str(&input.change().revision_id)
            .map_err(|_| Error::Conflict)?,
        session_epoch: session,
        selection_epoch: selection,
    })
}
pub(super) fn unsupported(input: &PreparedIncoming) -> Option<PendingReason> {
    match input.payload() {
        IncomingPayload::UserState(state)
            if matches!(
                state.position,
                Some(mdreader_sync::codec::ReadingPosition::BlockPixels { .. })
            ) =>
        {
            Some(PendingReason::PositionUnsupported)
        }
        _ => None,
    }
}
