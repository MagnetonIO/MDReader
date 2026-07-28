//! The canonical document shape used by MDReader's multi-source Library.
//!
//! Filesystem, Git, frontmatter, Drive, and Dropbox all target the same
//! path-keyed `document.Document` schema. Provider metadata lands in that one
//! shape rather than creating a second application-facing document identity.

use catdb_connectors::DataSource;
use catdb_core::{
    AttrTarget, Mapping, MappingWriteCapability, Path, PathProver, Schema, ValidMapping,
};

/// The Library schema name.
pub const SCHEMA: &str = crate::federated::SCHEMA;
/// The Library document entity.
pub const ENTITY: &str = crate::federated::ENTITY;

/// One supported cloud source family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudDocumentSource {
    /// Google Drive API v3 file records.
    GoogleDrive,
    /// Dropbox API v2 file records.
    Dropbox,
}

/// A cloud source could not be mapped completely into `document.Document`.
#[derive(Debug, thiserror::Error)]
pub enum CloudDocumentMappingError {
    /// The connector described something other than one `Document` entity.
    #[error("cloud source `{source_id}` does not describe exactly one Document entity")]
    UnexpectedEntity {
        /// Connector source identity.
        source_id: String,
    },
    /// The connector's declared physical schema drifted from the reviewed
    /// provider contract.
    #[error("cloud source `{source_id}` has an unexpected physical attribute set")]
    PhysicalSchemaDrift {
        /// Connector source identity.
        source_id: String,
    },
    /// A physical field and its semantic destination disagree on type.
    #[error("cloud source `{source_id}` field `{field}` has an incompatible type")]
    TypeMismatch {
        /// Connector source identity.
        source_id: String,
        /// Physical field.
        field: String,
    },
    /// The kernel rejected the complete read-only mapping.
    #[error("cloud source `{source_id}` mapping is invalid")]
    Invalid {
        /// Connector source identity.
        source_id: String,
    },
}

/// The application-facing Library document schema.
///
/// The required `path` field is the semantic record key: a workspace-relative
/// path for local documents, Drive file id for Drive, or provider path for
/// Dropbox. Other fields are nullable because observability does not guarantee
/// a value on every record. Stable cloud object identity also retains
/// `(source_id, source_record_key)` outside the value object.
pub fn document_schema() -> Schema {
    crate::federated::document_schema()
}

/// Build the complete read-only mapping from one cloud connector declaration
/// into [`document_schema`].
///
/// The accepted physical field set is exact. A provider SDK adding, removing,
/// or retyping a field is schema drift, not a reason to quietly narrow the
/// semantic answer.
pub fn cloud_document_mapping(
    source: &dyn DataSource,
    provider: CloudDocumentSource,
) -> Result<Mapping, CloudDocumentMappingError> {
    let source_id = source.id().to_string();
    let source_schema = source.schema();
    if source_schema.entities.len() != 1
        || !source_schema
            .entities
            .iter()
            .any(|entity| entity.as_str() == ENTITY)
        || !source_schema.arrows.is_empty()
    {
        return Err(CloudDocumentMappingError::UnexpectedEntity { source_id });
    }

    let fields: &[(&str, &str)] = match provider {
        CloudDocumentSource::GoogleDrive => &[
            // Drive has no filesystem path. Its stable provider-local file id
            // is the source location used by open-original and by the row key.
            ("id", "path"),
            ("name", "title"),
            ("mimeType", "mediaType"),
            ("modifiedAt", "modifiedAt"),
            ("size", "size"),
            ("checksum", "checksum"),
            ("body", "body"),
        ],
        CloudDocumentSource::Dropbox => &[
            ("id", "providerId"),
            ("name", "title"),
            ("path", "path"),
            ("modifiedAt", "modifiedAt"),
            ("size", "size"),
            ("rev", "revision"),
            ("contentHash", "checksum"),
            ("body", "body"),
        ],
    };
    let declared = source_schema
        .attributes
        .keys()
        .map(|field| field.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let expected = fields
        .iter()
        .map(|(field, _)| *field)
        .collect::<std::collections::BTreeSet<_>>();
    if declared != expected {
        return Err(CloudDocumentMappingError::PhysicalSchemaDrift { source_id });
    }

    let target_schema = document_schema();
    let mut mapping = Mapping::new(source_schema.name.clone(), target_schema.name.clone());
    mapping.entity_map.insert(ENTITY.into(), ENTITY.into());
    for (physical, semantic) in fields {
        let source_attr = source_schema
            .attributes
            .get(&(*physical).into())
            .ok_or_else(|| CloudDocumentMappingError::PhysicalSchemaDrift {
                source_id: source_id.clone(),
            })?;
        let target_attr = target_schema
            .attributes
            .get(&(*semantic).into())
            .ok_or_else(|| CloudDocumentMappingError::PhysicalSchemaDrift {
                source_id: source_id.clone(),
            })?;
        if source_schema.typeside.resolve(&source_attr.ty)
            != target_schema.typeside.resolve(&target_attr.ty)
        {
            return Err(CloudDocumentMappingError::TypeMismatch {
                source_id,
                field: (*physical).to_string(),
            });
        }
        mapping.attr_map.insert(
            (*physical).into(),
            AttrTarget {
                path: Path::identity(ENTITY),
                attr: (*semantic).into(),
            },
        );
    }
    mapping.write_capability = MappingWriteCapability::ReadOnly;
    Ok(mapping)
}

/// Validate [`cloud_document_mapping`] with the kernel.
pub fn validated_cloud_document_mapping(
    source: &dyn DataSource,
    provider: CloudDocumentSource,
    prover: &dyn PathProver,
) -> Result<ValidMapping, CloudDocumentMappingError> {
    cloud_document_mapping(source, provider)?
        .validate(source.schema(), &document_schema(), prover)
        .map_err(|_| CloudDocumentMappingError::Invalid {
            source_id: source.id().to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use catdb_connectors::{
        ReadContext, SourceAttr, SourceCapabilities, SourceCursor, SourceError, SourceFreshness,
        SourceId, SourceModel, SourcePage, SourcePlan,
    };
    use catdb_query::EntityQuery;
    use catdb_validation::EggProver;

    struct DeclaredSource {
        model: SourceModel,
    }

    impl DeclaredSource {
        fn drive() -> Self {
            Self {
                model: SourceModel::new(
                    SourceId::new("google-drive"),
                    "drive_physical",
                    ENTITY,
                    vec![
                        SourceAttr::text("id"),
                        SourceAttr::text("name"),
                        SourceAttr::text("mimeType"),
                        SourceAttr::parsed("modifiedAt", catdb_core::BaseType::Timestamp),
                        SourceAttr::parsed("size", catdb_core::BaseType::Int),
                        SourceAttr::text("checksum"),
                        SourceAttr::text("body"),
                    ],
                ),
            }
        }

        fn dropbox() -> Self {
            Self {
                model: SourceModel::new(
                    SourceId::new("dropbox"),
                    "dropbox_physical",
                    ENTITY,
                    vec![
                        SourceAttr::text("id"),
                        SourceAttr::text("name"),
                        SourceAttr::text("path"),
                        SourceAttr::parsed("modifiedAt", catdb_core::BaseType::Timestamp),
                        SourceAttr::parsed("size", catdb_core::BaseType::Int),
                        SourceAttr::text("rev"),
                        SourceAttr::text("contentHash"),
                        SourceAttr::text("body"),
                    ],
                ),
            }
        }
    }

    #[async_trait]
    impl DataSource for DeclaredSource {
        fn id(&self) -> &SourceId {
            self.model.id()
        }

        fn schema(&self) -> &Schema {
            self.model.schema()
        }

        fn capabilities(&self) -> &SourceCapabilities {
            self.model.capabilities()
        }

        fn source_spec(&self) -> serde_json::Value {
            serde_json::json!({"kind": "test"})
        }

        async fn freshness(&self, _context: &ReadContext) -> Result<SourceFreshness, SourceError> {
            unreachable!("mapping validation never reads")
        }

        fn compile(
            &self,
            _query: &EntityQuery,
            _authorized_fields: &[String],
        ) -> Result<SourcePlan, SourceError> {
            unreachable!("mapping validation never compiles")
        }

        async fn execute_page(
            &self,
            _plan: &SourcePlan,
            _cursor: Option<&SourceCursor>,
            _context: &ReadContext,
        ) -> Result<SourcePage, SourceError> {
            unreachable!("mapping validation never executes")
        }
    }

    #[test]
    fn drive_mapping_is_total_validated_and_read_only() {
        let source = DeclaredSource::drive();
        let mapping = validated_cloud_document_mapping(
            &source,
            CloudDocumentSource::GoogleDrive,
            &EggProver::default(),
        )
        .expect("valid mapping");
        assert_eq!(
            mapping.mapping().write_capability,
            MappingWriteCapability::ReadOnly
        );
        assert_eq!(
            mapping.mapping().attr_map.len(),
            source.schema().attributes.len()
        );
        assert_eq!(
            mapping.mapping().attr_map[&"name".into()].attr.as_str(),
            "title"
        );
        assert_eq!(
            mapping.mapping().attr_map[&"id".into()].attr.as_str(),
            "path"
        );
    }

    #[test]
    fn every_library_source_targets_one_canonical_document_schema() {
        assert_eq!(SCHEMA, crate::federated::SCHEMA);
        assert_eq!(ENTITY, crate::federated::ENTITY);
        assert_eq!(
            document_schema(),
            crate::federated::document_schema(),
            "filesystem, Git, frontmatter, Drive, and Dropbox must not publish parallel Document concepts"
        );
    }

    #[test]
    fn cloud_precedence_is_pinned_to_the_validated_cloud_mappings() {
        let drive = DeclaredSource::drive();
        let dropbox = DeclaredSource::dropbox();
        let drive_mapping =
            cloud_document_mapping(&drive, CloudDocumentSource::GoogleDrive).expect("Drive maps");
        let dropbox_mapping =
            cloud_document_mapping(&dropbox, CloudDocumentSource::Dropbox).expect("Dropbox maps");

        for entry in crate::federated::precedence().fields() {
            for provider in &entry.order {
                let mapping = match provider.source.as_str() {
                    "google-drive" => &drive_mapping,
                    "dropbox" => &dropbox_mapping,
                    _ => continue,
                };
                assert_eq!(
                    mapping
                        .attr_map
                        .get(&provider.attr)
                        .unwrap_or_else(|| panic!("`{provider}` must be mapped"))
                        .attr,
                    entry.field,
                    "`{provider}` must land on `{}`",
                    entry.field
                );
            }
        }
    }

    #[test]
    fn a_new_physical_field_is_refused_instead_of_dropped() {
        let mut source = DeclaredSource::drive();
        source = DeclaredSource {
            model: SourceModel::new(
                source.model.id().clone(),
                "drive_physical",
                ENTITY,
                source
                    .model
                    .attrs()
                    .iter()
                    .cloned()
                    .chain([SourceAttr::text("silentlyNew")])
                    .collect(),
            ),
        };
        assert!(matches!(
            cloud_document_mapping(&source, CloudDocumentSource::GoogleDrive),
            Err(CloudDocumentMappingError::PhysicalSchemaDrift { .. })
        ));
    }
}
