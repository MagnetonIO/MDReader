//! SQLite-backed metadata source for foreground Library queries.
//!
//! This adapter only reads the publication owned by the background cloud-sync
//! coordinator. It has no credential, provider client, or body-read path.

use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use catdb_connectors::{
    DataSource, ReadContext, SourceAttr, SourceCapabilities, SourceCursor, SourceError,
    SourceFreshness, SourceId, SourceModel, SourceOrderDirection, SourcePage, SourcePlan,
    SourceRow,
};
use catdb_core::{BaseType, Schema};
use catdb_query::EntityQuery;
use catdb_storage::ObjectFilterOp;
use serde::{Deserialize, Serialize};

use super::store::{
    CloudDocumentStore, CloudDocumentStoreError, MaterializedMetadataField,
    MaterializedMetadataFilter, MaterializedMetadataOrder, MaterializedMetadataPageQuery,
    PublishedCloudDocument, PublishedCloudSnapshot,
};

const CURSOR_VERSION: u8 = 1;

/// Provider-specific physical contract retained by a materialized source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MaterializedMarkdownProvider {
    /// Google Drive's reviewed physical metadata fields.
    GoogleDrive,
    /// Dropbox's reviewed physical metadata fields.
    Dropbox,
}

/// Exact published versions reused when a Library runtime binds this source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MaterializedVersionPins {
    /// Physical schema identity used to materialize the rows.
    pub(crate) schema_version_id: uuid::Uuid,
    /// Mapping identity used to interpret the rows.
    pub(crate) mapping_version_id: uuid::Uuid,
    /// Provider source identity used to materialize the rows.
    pub(crate) source_version_id: uuid::Uuid,
}

/// A source over exactly one cloud account's published SQLite metadata.
#[derive(Clone)]
pub(crate) struct MaterializedMarkdownSource {
    store: Arc<CloudDocumentStore>,
    source_id: SourceId,
    provider: MaterializedMarkdownProvider,
    model: SourceModel,
    source_spec: serde_json::Value,
}

impl MaterializedMarkdownSource {
    /// Binds the published metadata for one Google Drive account.
    pub(crate) fn google_drive(store: Arc<CloudDocumentStore>, source_id: SourceId) -> Self {
        Self::new(store, source_id, MaterializedMarkdownProvider::GoogleDrive)
    }

    /// Binds the published metadata for one Dropbox account.
    pub(crate) fn dropbox(store: Arc<CloudDocumentStore>, source_id: SourceId) -> Self {
        Self::new(store, source_id, MaterializedMarkdownProvider::Dropbox)
    }

    fn new(
        store: Arc<CloudDocumentStore>,
        source_id: SourceId,
        provider: MaterializedMarkdownProvider,
    ) -> Self {
        let (schema, attrs) = match provider {
            MaterializedMarkdownProvider::GoogleDrive => (
                "mdreader",
                vec![
                    text_attr("id"),
                    text_attr("name"),
                    text_attr("mimeType"),
                    ordered_parsed_attr("modifiedAt", BaseType::Timestamp),
                    ordered_parsed_attr("size", BaseType::Int),
                    text_attr("checksum"),
                    SourceAttr::text("body"),
                ],
            ),
            MaterializedMarkdownProvider::Dropbox => (
                "mdreader",
                vec![
                    text_attr("id"),
                    text_attr("name"),
                    text_attr("path"),
                    ordered_parsed_attr("modifiedAt", BaseType::Timestamp),
                    ordered_parsed_attr("size", BaseType::Int),
                    text_attr("rev"),
                    text_attr("contentHash"),
                    SourceAttr::text("body"),
                ],
            ),
        };
        let model = SourceModel::new(source_id.clone(), schema, "Document", attrs).pushes_limit();
        let provider_name = match provider {
            MaterializedMarkdownProvider::GoogleDrive => "google-drive",
            MaterializedMarkdownProvider::Dropbox => "dropbox",
        };
        Self {
            store,
            source_id: source_id.clone(),
            provider,
            model,
            source_spec: serde_json::json!({
                "kind": "mdreader-materialized-markdown/v1",
                "provider": provider_name,
                "source_id": source_id.as_str(),
                "storage": "published-sqlite-metadata",
            }),
        }
    }

    /// Reuses the exact semantic versions embedded in the published snapshot.
    pub(crate) fn published_version_pins(
        &self,
    ) -> Result<Option<MaterializedVersionPins>, SourceError> {
        let snapshot = self
            .store
            .published_snapshot(&self.source_id)
            .map_err(|error| store_error(&self.source_id, error))?;
        snapshot
            .map(|snapshot| version_pins(&self.source_id, &snapshot))
            .transpose()
    }

    fn snapshot(&self) -> Result<PublishedCloudSnapshot, SourceError> {
        self.store
            .published_snapshot(&self.source_id)
            .map_err(|error| store_error(&self.source_id, error))?
            .ok_or_else(|| SourceError::Unavailable {
                source_id: self.source_id.clone(),
                reason: "cloud metadata has not been published yet".to_string(),
            })
    }

    fn decode_cursor(
        &self,
        cursor: &SourceCursor,
        context: &ReadContext,
        snapshot: &PublishedCloudSnapshot,
        plan: &SourcePlan,
    ) -> Result<u64, SourceError> {
        let cursor: MaterializedCursor =
            serde_json::from_slice(cursor.as_bytes()).map_err(|_| SourceError::InvalidCursor)?;
        if cursor.version != CURSOR_VERSION
            || cursor.source_id != self.source_id.as_str()
            || cursor.generation != snapshot.generation
            || cursor.query_id != context.query_id()
            || cursor.plan_digest != plan_digest(plan)?
        {
            return Err(SourceError::InvalidCursor);
        }
        Ok(cursor.offset)
    }

    fn encode_cursor(
        &self,
        offset: u64,
        context: &ReadContext,
        snapshot: &PublishedCloudSnapshot,
        plan: &SourcePlan,
    ) -> Result<SourceCursor, SourceError> {
        let bytes = serde_json::to_vec(&MaterializedCursor {
            version: CURSOR_VERSION,
            source_id: self.source_id.as_str().to_string(),
            generation: snapshot.generation,
            offset,
            query_id: context.query_id(),
            plan_digest: plan_digest(plan)?,
        })
        .map_err(|_| SourceError::InvalidCursor)?;
        SourceCursor::new(bytes)
    }

    fn page_query(
        &self,
        plan: &SourcePlan,
        offset: u64,
        limit: usize,
    ) -> Result<MaterializedMetadataPageQuery, SourceError> {
        let filters = plan
            .pushed
            .filters
            .iter()
            .map(|filter| {
                let field = field_for(self.provider, &filter.attr).ok_or_else(|| {
                    SourceError::InvalidPlan {
                        source_id: self.source_id.clone(),
                    }
                })?;
                Ok(MaterializedMetadataFilter {
                    field,
                    op: filter.op,
                    value: filter.value.clone(),
                })
            })
            .collect::<Result<Vec<_>, SourceError>>()?;
        let order = plan
            .pushed_order
            .iter()
            .map(|ordering| {
                let field = field_for(self.provider, &ordering.attr).ok_or_else(|| {
                    SourceError::InvalidPlan {
                        source_id: self.source_id.clone(),
                    }
                })?;
                Ok(MaterializedMetadataOrder {
                    field,
                    direction: ordering.direction,
                })
            })
            .collect::<Result<Vec<_>, SourceError>>()?;
        Ok(MaterializedMetadataPageQuery {
            filters,
            order,
            offset,
            limit,
        })
    }

    fn row(
        &self,
        document: &PublishedCloudDocument,
        projected: &[String],
    ) -> Result<SourceRow, SourceError> {
        let mut raw = document_fields(self.provider, document);
        raw.retain(|field, _| projected.contains(field));
        Ok(SourceRow {
            source: self.source_id.clone(),
            entity: self.model.entity().to_string(),
            key: document.key.as_str().to_string(),
            value: self.model.type_row(document.key.as_str(), raw)?,
        })
    }
}

#[async_trait]
impl DataSource for MaterializedMarkdownSource {
    fn id(&self) -> &SourceId {
        &self.source_id
    }

    fn schema(&self) -> &Schema {
        self.model.schema()
    }

    fn capabilities(&self) -> &SourceCapabilities {
        self.model.capabilities()
    }

    fn source_spec(&self) -> serde_json::Value {
        self.source_spec.clone()
    }

    async fn freshness(&self, context: &ReadContext) -> Result<SourceFreshness, SourceError> {
        context.check()?;
        let source = self.clone();
        let snapshot = tokio::task::spawn_blocking(move || source.snapshot())
            .await
            .map_err(|_| SourceError::WorkerFailed)??;
        context.check()?;
        Ok(SourceFreshness {
            observed_at_ms: snapshot.observed_at_ms,
            freshness_ms: now_ms()
                .saturating_sub(snapshot.observed_at_ms)
                .unsigned_abs(),
        })
    }

    fn compile(
        &self,
        query: &EntityQuery,
        authorized_fields: &[String],
    ) -> Result<SourcePlan, SourceError> {
        if authorized_fields.iter().any(|field| field == "body")
            || query.filters.iter().any(|filter| filter.attr == "body")
        {
            return Err(SourceError::UnsupportedFilter {
                source_id: self.source_id.clone(),
                attr: "body".to_string(),
                detail: "published metadata does not contain document bodies; use the on-demand document read"
                    .to_string(),
            });
        }
        self.model.compile(query, authorized_fields)
    }

    async fn execute_page(
        &self,
        plan: &SourcePlan,
        cursor: Option<&SourceCursor>,
        context: &ReadContext,
    ) -> Result<SourcePage, SourceError> {
        context.check()?;
        self.model.admit(plan)?;
        let page_size = context.page_size().min(context.remaining_rows()).min(
            plan.pushed.limit.map_or(usize::MAX, |limit| {
                usize::try_from(limit).unwrap_or(usize::MAX)
            }),
        );
        if page_size == 0 {
            return Err(SourceError::RowBudgetExceeded);
        }
        let source = self.clone();
        let plan = plan.clone();
        let cursor = cursor.cloned();
        let context = context.clone();
        tokio::task::spawn_blocking(move || {
            context.check()?;
            let snapshot = source.snapshot()?;
            let offset = cursor.as_ref().map_or(Ok(0), |cursor| {
                source.decode_cursor(cursor, &context, &snapshot, &plan)
            })?;
            let query = source.page_query(&plan, offset, page_size)?;
            let page = source
                .store
                .materialized_page(&source.source_id, &snapshot, &query)
                .map_err(|error| store_error(&source.source_id, error))?;
            let rows = page
                .rows
                .iter()
                .map(|document| source.row(document, &plan.projected))
                .collect::<Result<Vec<_>, _>>()?;
            let next_cursor = (page.has_more && plan.pushed.limit.is_none())
                .then(|| {
                    let next = offset
                        .checked_add(
                            u64::try_from(rows.len()).map_err(|_| SourceError::InvalidCursor)?,
                        )
                        .ok_or(SourceError::InvalidCursor)?;
                    source.encode_cursor(next, &context, &snapshot, &plan)
                })
                .transpose()?;
            context.check()?;
            Ok(SourcePage {
                source: source.source_id.clone(),
                scanned: rows.len().max(usize::from(page.examined_candidate)),
                rows,
                next_cursor,
                observed_at_ms: snapshot.observed_at_ms,
                freshness_ms: now_ms()
                    .saturating_sub(snapshot.observed_at_ms)
                    .unsigned_abs(),
            })
        })
        .await
        .map_err(|_| SourceError::WorkerFailed)?
    }
}

fn text_attr(name: &str) -> SourceAttr {
    SourceAttr::text(name)
        .pushes([
            ObjectFilterOp::Eq,
            ObjectFilterOp::Lt,
            ObjectFilterOp::Le,
            ObjectFilterOp::Gt,
            ObjectFilterOp::Ge,
            ObjectFilterOp::Prefix,
        ])
        .orders([
            SourceOrderDirection::Ascending,
            SourceOrderDirection::Descending,
        ])
}

fn ordered_parsed_attr(name: &str, ty: BaseType) -> SourceAttr {
    SourceAttr::parsed(name, ty)
        .pushes([
            ObjectFilterOp::Eq,
            ObjectFilterOp::Lt,
            ObjectFilterOp::Le,
            ObjectFilterOp::Gt,
            ObjectFilterOp::Ge,
        ])
        .orders([
            SourceOrderDirection::Ascending,
            SourceOrderDirection::Descending,
        ])
}

fn field_for(
    provider: MaterializedMarkdownProvider,
    attribute: &str,
) -> Option<MaterializedMetadataField> {
    match (provider, attribute) {
        (MaterializedMarkdownProvider::GoogleDrive, "id") => {
            Some(MaterializedMetadataField::RecordKey)
        }
        (MaterializedMarkdownProvider::GoogleDrive, "name") => {
            Some(MaterializedMetadataField::Name)
        }
        (MaterializedMarkdownProvider::GoogleDrive, "mimeType") => {
            Some(MaterializedMetadataField::MediaType)
        }
        (MaterializedMarkdownProvider::GoogleDrive, "modifiedAt") => {
            Some(MaterializedMetadataField::ModifiedAt)
        }
        (MaterializedMarkdownProvider::GoogleDrive, "size") => {
            Some(MaterializedMetadataField::Size)
        }
        (MaterializedMarkdownProvider::GoogleDrive, "checksum") => {
            Some(MaterializedMetadataField::Revision)
        }
        (MaterializedMarkdownProvider::Dropbox, "id") => Some(MaterializedMetadataField::RecordKey),
        (MaterializedMarkdownProvider::Dropbox, "name") => Some(MaterializedMetadataField::Name),
        (MaterializedMarkdownProvider::Dropbox, "path") => {
            Some(MaterializedMetadataField::DisplayPath)
        }
        (MaterializedMarkdownProvider::Dropbox, "modifiedAt") => {
            Some(MaterializedMetadataField::ModifiedAt)
        }
        (MaterializedMarkdownProvider::Dropbox, "size") => Some(MaterializedMetadataField::Size),
        (MaterializedMarkdownProvider::Dropbox, "rev") => Some(MaterializedMetadataField::Revision),
        (MaterializedMarkdownProvider::Dropbox, "contentHash") => {
            Some(MaterializedMetadataField::ContentHash)
        }
        _ => None,
    }
}

fn document_fields(
    provider: MaterializedMarkdownProvider,
    document: &PublishedCloudDocument,
) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    match provider {
        MaterializedMarkdownProvider::GoogleDrive => {
            fields.insert("id".to_string(), document.key.as_str().to_string());
            fields.insert("name".to_string(), document.name.clone());
            if let Some(media_type) = &document.media_type {
                fields.insert("mimeType".to_string(), media_type.clone());
            }
            if let Some(modified_at) = document.modified_at_ms {
                fields.insert("modifiedAt".to_string(), modified_at.to_string());
            }
            if let Some(size) = document.size_bytes {
                fields.insert("size".to_string(), size.to_string());
            }
            fields.insert("checksum".to_string(), document.revision.opaque.clone());
        }
        MaterializedMarkdownProvider::Dropbox => {
            fields.insert("id".to_string(), document.key.as_str().to_string());
            fields.insert("name".to_string(), document.name.clone());
            fields.insert("path".to_string(), document.display_path.clone());
            if let Some(modified_at) = document.modified_at_ms {
                fields.insert("modifiedAt".to_string(), modified_at.to_string());
            }
            if let Some(size) = document.size_bytes {
                fields.insert("size".to_string(), size.to_string());
            }
            fields.insert("rev".to_string(), document.revision.opaque.clone());
            if let Some(content_hash) = document.content_hash {
                fields.insert("contentHash".to_string(), lowercase_hex(&content_hash));
            }
        }
    }
    fields
}

fn lowercase_hex(bytes: &[u8; 32]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

fn version_pins(
    source_id: &SourceId,
    snapshot: &PublishedCloudSnapshot,
) -> Result<MaterializedVersionPins, SourceError> {
    Ok(MaterializedVersionPins {
        schema_version_id: uuid::Uuid::from_slice(snapshot.pins.schema_version_id()).map_err(
            |_| SourceError::Malformed {
                source_id: source_id.clone(),
                path: "published metadata".to_string(),
                attr: "schema_version_id".to_string(),
                detail: "published version pin is not a UUID".to_string(),
            },
        )?,
        mapping_version_id: uuid::Uuid::from_slice(snapshot.pins.mapping_version_id()).map_err(
            |_| SourceError::Malformed {
                source_id: source_id.clone(),
                path: "published metadata".to_string(),
                attr: "mapping_version_id".to_string(),
                detail: "published version pin is not a UUID".to_string(),
            },
        )?,
        source_version_id: uuid::Uuid::from_slice(snapshot.pins.source_version_id()).map_err(
            |_| SourceError::Malformed {
                source_id: source_id.clone(),
                path: "published metadata".to_string(),
                attr: "source_version_id".to_string(),
                detail: "published version pin is not a UUID".to_string(),
            },
        )?,
    })
}

fn store_error(source_id: &SourceId, error: CloudDocumentStoreError) -> SourceError {
    match error {
        CloudDocumentStoreError::UnknownGeneration => SourceError::InvalidCursor,
        CloudDocumentStoreError::ForeignSource => SourceError::Unavailable {
            source_id: source_id.clone(),
            reason: "cloud metadata has not been published yet".to_string(),
        },
        _ => SourceError::Transport {
            source_id: source_id.clone(),
        },
    }
}

fn plan_digest(plan: &SourcePlan) -> Result<[u8; 32], SourceError> {
    serde_json::to_vec(plan)
        .map(|bytes| *blake3::hash(&bytes).as_bytes())
        .map_err(|_| SourceError::InvalidCursor)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(1)
}

#[derive(Serialize, Deserialize)]
struct MaterializedCursor {
    version: u8,
    source_id: String,
    generation: [u8; 16],
    offset: u64,
    query_id: uuid::Uuid,
    plan_digest: [u8; 32],
}
