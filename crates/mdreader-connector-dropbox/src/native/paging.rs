//! Dropbox's remote limit is approximate; local output remains strictly bounded.
//! Unconsumed entries are replayed from the exact origin cursor and fenced by a
//! page digest. Only offsets/tokens are signed, never a provider body buffer.
use super::*;
use serde::{Deserialize, Serialize};
const PREFIX: &str = "mdreader-dropbox-page-v1:";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Replay {
    origin: Option<OpaqueValue>,
    limit: usize,
    offset: usize,
    digest: [u8; 32],
    after: OpaqueValue,
    more: bool,
}
pub(super) struct Chunk {
    pub entries: Vec<serde_json::Value>,
    pub cursor: ProviderCursor,
    pub has_more: bool,
}
impl Client {
    pub(super) async fn page_chunk(
        &self,
        kind: CursorKind,
        cursor: Option<&ProviderCursor>,
        context: &IoContext,
    ) -> Result<Chunk> {
        let decoded = cursor
            .map(|c| {
                self.codec.decode_traversal(
                    ProviderKind::Dropbox,
                    self.descriptor.binding(),
                    kind,
                    c,
                )
            })
            .transpose()?;
        let replay: Option<Replay> = decoded
            .as_ref()
            .filter(|s| s.starts_with(PREFIX))
            .map(|s| serde_json::from_str(&s[PREFIX.len()..]).map_err(|_| ProviderError::Invalid))
            .transpose()?;
        if replay.as_ref().is_some_and(|r| {
            r.offset == 0
                || r.limit == 0
                || r.limit > MAX_PAGE_ROWS
                || kind == CursorKind::Changes && r.origin.is_none()
        }) {
            return Err(ProviderError::Invalid);
        }
        let origin = match &replay {
            Some(r) => r.origin.clone(),
            None => decoded.map(OpaqueValue::new).transpose()?,
        };
        let limit = replay.as_ref().map_or(context.page_rows(), |r| r.limit);
        let mut spent = 0;
        let response = match &origin {
            Some(token) => {
                self.api(
                    "list_folder/continue",
                    serde_json::json!({"cursor":token.as_str()}),
                    context,
                    &mut spent,
                )
                .await?
            }
            None => {
                self.api(
                    "list_folder",
                    self.root_arguments(limit)?,
                    context,
                    &mut spent,
                )
                .await?
            }
        };
        if response.status() == 409 {
            let value: serde_json::Value =
                serde_json::from_slice(response.body()).map_err(|_| ProviderError::Protocol)?;
            if value
                .get("error")
                .and_then(|e| e.get(".tag"))
                .and_then(|v| v.as_str())
                == Some("reset")
            {
                return Err(ProviderError::ResetCursor);
            }
        }
        if response.status() != 200 {
            return Err(transport::read_failure(&response));
        }
        let mut page: Page =
            serde_json::from_slice(response.body()).map_err(|_| ProviderError::Protocol)?;
        let after = OpaqueValue::new(page.cursor)?;
        if page.has_more && origin.as_ref() == Some(&after) {
            return Err(ProviderError::Protocol);
        }
        let digest =
            body_hash(&serde_json::to_vec(&page.entries).map_err(|_| ProviderError::Protocol)?);
        let offset = replay.as_ref().map_or(0, |r| r.offset);
        if let Some(r) = &replay {
            if r.digest != digest || r.more != page.has_more || offset >= page.entries.len() {
                return Err(ProviderError::ResetCursor);
            }
        }
        let end = offset
            .saturating_add(context.page_rows())
            .min(page.entries.len());
        let local_more = end < page.entries.len();
        let next = if local_more {
            let value = Replay {
                origin,
                limit,
                offset: end,
                digest,
                after: replay.as_ref().map_or(after.clone(), |r| r.after.clone()),
                more: page.has_more,
            };
            let value = PREFIX.to_owned()
                + &serde_json::to_string(&value).map_err(|_| ProviderError::Protocol)?;
            let cursor = self.codec.encode_traversal(
                ProviderKind::Dropbox,
                self.descriptor.binding(),
                kind,
                value,
            )?;
            if cursor.as_bytes().len() > 32 * 1024 {
                return Err(ProviderError::TooLarge);
            }
            cursor
        } else {
            self.cursor(
                kind,
                replay.as_ref().map_or(after.as_str(), |r| r.after.as_str()),
            )?
        };
        context.check()?;
        let entries = page.entries.drain(offset..end).collect();
        Ok(Chunk {
            entries,
            cursor: next,
            has_more: local_more || page.has_more,
        })
    }
}
