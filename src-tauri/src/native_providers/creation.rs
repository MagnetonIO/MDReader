//! Native DTO validation for create-only folder operations.
use super::*;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreationWire {
    operation_id: String,
    state: MutationStateDto,
    projection: Option<ProjectionReceipt>,
    #[serde(default)]
    diagnostic: Option<String>,
}
impl NativeProviders {
    async fn creation_result(
        &self,
        source: &SourceId,
        id: &str,
        value: Value,
        inspect: bool,
    ) -> Result<NativeProviderResult> {
        let value: CreationWire = decode(value)?;
        if value.operation_id != id
            || (value.projection.is_some() != matches!(value.state, MutationStateDto::Published))
        {
            return Err(NativeProviderError::InvalidResponse);
        }
        if value.diagnostic.as_deref().is_some_and(|value| {
            !matches!(
                value,
                "no_matching_operation"
                    | "ambiguous_operation"
                    | "metadata_mismatch"
                    | "body_mismatch"
                    | "missing_write_validator"
            )
        }) {
            return Err(NativeProviderError::InvalidResponse);
        }
        let projection = value.projection.map(projection_dto).transpose()?;
        if let Some(receipt) = &projection {
            self.source_projection(source, receipt).await?;
        }
        Ok(if inspect {
            NativeProviderResult::InspectCreate {
                operation_id: id.into(),
                state: value.state,
                projection,
                diagnostic: value.diagnostic,
            }
        } else {
            NativeProviderResult::CreateDocument {
                operation_id: id.into(),
                state: value.state,
                projection,
                diagnostic: value.diagnostic,
            }
        })
    }
    pub(super) async fn create_document(
        &self,
        source_id: String,
        operation_id: String,
        filename: String,
        markdown: String,
    ) -> Result<NativeProviderResult> {
        let source = valid_source(&source_id)?;
        let id = valid_uuid(&operation_id)?;
        if id.get_version_num() != 4 {
            return Err(NativeProviderError::Invalid);
        }
        mdreader_provider::creation::validate_filename(&filename)?;
        if markdown.len() > MAX_MARKDOWN {
            return Err(NativeProviderError::TooLarge);
        }
        let result = self
            .rpc
            .provider(ProviderRequest::CreateDocument {
                source_id: source.clone(),
                operation_id: id,
                filename,
                markdown,
            })
            .await;
        let value = match result {
            Err(NativeProviderError::Timeout | NativeProviderError::ServiceUnavailable) => {
                return Err(NativeProviderError::UnknownWrite { operation_id });
            }
            value => value?,
        };
        self.creation_result(&source, &operation_id, value, false)
            .await
    }
    pub(super) async fn inspect_create(
        &self,
        source_id: String,
        operation_id: String,
    ) -> Result<NativeProviderResult> {
        let source = valid_source(&source_id)?;
        let id = valid_uuid(&operation_id)?;
        if id.get_version_num() != 4 {
            return Err(NativeProviderError::Invalid);
        }
        let value = self
            .rpc
            .provider(ProviderRequest::InspectCreate {
                source_id: source.clone(),
                operation_id: id,
            })
            .await?;
        self.creation_result(&source, &operation_id, value, true)
            .await
    }
}
