//! Remote limits are approximate. Freeze overflow once in the owner, never
//! refetch an origin cursor to reconstruct an unconsumed page.
use super::*;
use serde::{Deserialize, Serialize};
const PREFIX: &str = "mdreader-dropbox-page-v2:";
const LEGACY: &str = "mdreader-dropbox-page-v1:";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Continuation {
    slot: uuid::Uuid,
    offset: usize,
    digest: [u8; 32],
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenPage {
    slot: uuid::Uuid,
    origin: Option<OpaqueValue>,
    limit: usize,
    page: Page,
}
pub(super) struct Chunk {
    pub entries: Vec<serde_json::Value>,
    pub cursor: ProviderCursor,
    pub has_more: bool,
}
impl Client {
    // Validate the complete raw page before filtering. Changes must retain files
    // renamed away from Markdown, unknown deletions and folder events.
    fn relevant_entries(
        &self,
        kind: CursorKind,
        entries: &[serde_json::Value],
    ) -> Result<Vec<serde_json::Value>> {
        let mut relevant = Vec::new();
        for entry in entries {
            match entry.get(".tag").and_then(|v| v.as_str()) {
                Some("file") => {
                    let file: File = serde_json::from_value(entry.clone())
                        .map_err(|_| ProviderError::Protocol)?;
                    file.validate()?;
                    if !self.allowed_path(&file.path_lower) {
                        return Err(ProviderError::Unauthorized);
                    }
                    if kind == CursorKind::Changes || markdown(&file.name) {
                        relevant.push(entry.clone());
                    }
                }
                Some("folder") => {
                    if kind == CursorKind::Changes {
                        relevant.push(entry.clone());
                    }
                }
                Some("deleted") if kind == CursorKind::Changes => {
                    let path = entry
                        .get("path_lower")
                        .and_then(|v| v.as_str())
                        .ok_or(ProviderError::Protocol)?;
                    if !self.allowed_path(path) {
                        return Err(ProviderError::Unauthorized);
                    }
                    relevant.push(entry.clone());
                }
                _ => return Err(ProviderError::Protocol),
            }
        }
        Ok(relevant)
    }
    pub(super) async fn page_chunk(
        &self,
        kind: CursorKind,
        cursor: Option<&ProviderCursor>,
        context: &IoContext,
    ) -> Result<Chunk> {
        context.check()?;
        revalidate_authority(&*self.authority, self.descriptor.binding(), context, None).await?;
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
        // Old replay cursors are deliberately rebuilt once; never refetch their
        // possibly changed origin page and report an incomplete tail as complete.
        if decoded
            .as_ref()
            .is_some_and(|token| token.starts_with(LEGACY))
        {
            return Err(ProviderError::ResetCursor);
        }
        let continuation: Option<Continuation> = decoded
            .as_ref()
            .filter(|s| s.starts_with(PREFIX))
            .map(|s| serde_json::from_str(&s[PREFIX.len()..]).map_err(|_| ProviderError::Invalid))
            .transpose()?;
        let (frozen, digest, offset) = if let Some(next) = continuation {
            if next.offset == 0 || next.slot.get_version_num() != 4 {
                return Err(ProviderError::Invalid);
            }
            let bytes = self
                .authority
                .load_page(self.descriptor.binding(), kind, next.slot, next.digest)
                .await?;
            context.check()?;
            revalidate_authority(&*self.authority, self.descriptor.binding(), context, None)
                .await?;
            if bytes.is_empty()
                || bytes.len() > MAX_PAGE_SPOOL_BYTES
                || bytes.len() > context.byte_limit()
                || body_hash(&bytes) != next.digest
            {
                return Err(ProviderError::Protocol);
            }
            let frozen: FrozenPage =
                serde_json::from_slice(&bytes).map_err(|_| ProviderError::Protocol)?;
            if frozen.slot != next.slot
                || frozen.limit == 0
                || frozen.limit > MAX_PAGE_ROWS
                || kind == CursorKind::Changes && frozen.origin.is_none()
            {
                return Err(ProviderError::Protocol);
            }
            (frozen, Some(next.digest), next.offset)
        } else {
            let origin = decoded.map(OpaqueValue::new).transpose()?;
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
                        self.root_arguments(context.page_rows())?,
                        context,
                        &mut spent,
                    )
                    .await?
                }
            };
            if response.status() == 409 {
                let value: serde_json::Value =
                    serde_json::from_slice(response.body()).map_err(|_| ProviderError::Protocol)?;
                if value.pointer("/error/.tag").and_then(|v| v.as_str()) == Some("reset") {
                    return Err(ProviderError::ResetCursor);
                }
            }
            if response.status() != 200 {
                return Err(transport::read_failure(&response));
            }
            let page: Page =
                serde_json::from_slice(response.body()).map_err(|_| ProviderError::Protocol)?;
            let after = OpaqueValue::new(page.cursor.clone())?;
            if page.has_more && origin.as_ref() == Some(&after) {
                return Err(ProviderError::Protocol);
            }
            (
                FrozenPage {
                    slot: uuid::Uuid::new_v4(),
                    origin,
                    limit: context.page_rows(),
                    page,
                },
                None,
                0,
            )
        };
        let relevant = self.relevant_entries(kind, &frozen.page.entries)?;
        if offset > 0 && offset >= relevant.len() {
            return Err(ProviderError::Invalid);
        }
        let end = offset
            .saturating_add(context.page_rows())
            .min(relevant.len());
        let local_more = end < relevant.len();
        let next = if local_more {
            let digest = if let Some(digest) = digest {
                digest
            } else {
                let bytes = serde_json::to_vec(&frozen).map_err(|_| ProviderError::Protocol)?;
                if bytes.len() > MAX_PAGE_SPOOL_BYTES || bytes.len() > context.byte_limit() {
                    return Err(ProviderError::TooLarge);
                }
                let digest = body_hash(&bytes);
                self.authority
                    .store_page(self.descriptor.binding(), kind, frozen.slot, digest, &bytes)
                    .await?;
                digest
            };
            let token = PREFIX.to_owned()
                + &serde_json::to_string(&Continuation {
                    slot: frozen.slot,
                    offset: end,
                    digest,
                })
                .map_err(|_| ProviderError::Protocol)?;
            self.codec.encode_traversal(
                ProviderKind::Dropbox,
                self.descriptor.binding(),
                kind,
                token,
            )?
        } else {
            self.cursor(kind, &frozen.page.cursor)?
        };
        context.check()?;
        revalidate_authority(&*self.authority, self.descriptor.binding(), context, None).await?;
        Ok(Chunk {
            entries: relevant[offset..end].to_vec(),
            cursor: next,
            has_more: local_more || frozen.page.has_more,
        })
    }
}
