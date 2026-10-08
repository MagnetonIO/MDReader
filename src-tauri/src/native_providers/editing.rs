//! Thin native exact-receipt edit and durable mutation inspection projection.
use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactEditWire {
    write_source_id: String,
    edit_id: String,
    markdown: String,
    expires_at_ms: i64,
    projection: ProjectionReceipt,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectionWire {
    operation_id: String,
    state: MutationStateDto,
    projection: Option<ProjectionReceipt>,
}
impl NativeProviders {
    pub(super) async fn begin_edit_exact(
        &self,
        source_id: String,
        path: String,
        revision: String,
    ) -> Result<NativeProviderResult> {
        let expected = ProjectionDto {
            job_id: String::new(),
            reader_source_id: source_id.clone(),
            path: path.clone(),
            revision: revision.clone(),
        };
        validate_projection(&expected)?;
        let value: ExactEditWire = decode(
            self.rpc
                .provider(ProviderRequest::BeginEditExact {
                    reader_source_id: SourceId::new(source_id.clone())?,
                    path: mdreader_provider::RecordKey::new(path.clone())?,
                    revision: mdreader_provider::OpaqueValue::new(revision.clone())?,
                })
                .await?,
        )?;
        valid_uuid(&value.edit_id)?;
        let write_source = valid_source(&value.write_source_id)?;
        let projection = projection_dto(value.projection)?;
        if projection.reader_source_id != source_id
            || projection.path != path
            || projection.revision != revision
            || value.markdown.len() > MAX_MARKDOWN
        {
            return Err(NativeProviderError::InvalidResponse);
        }
        // A cache receipt alone cannot authorize bytes: perform the ordinary
        // exact core read and require the edit ticket's body to agree.
        let document = self.rpc.read(&projection).await?;
        if document.source_id != source_id
            || document.path != path
            || document.revision != revision
            || document.markdown != value.markdown
        {
            return Err(NativeProviderError::InvalidResponse);
        }
        self.source_projection(&write_source, &projection).await?;
        Ok(NativeProviderResult::BeginEditExact {
            write_source_id: value.write_source_id,
            edit_id: value.edit_id,
            markdown: value.markdown,
            expires_at_ms: safe_number(value.expires_at_ms.into())?,
            projection,
        })
    }
    pub(super) async fn inspect_save(
        &self,
        source_id: String,
        operation_id: String,
    ) -> Result<NativeProviderResult> {
        let source = valid_source(&source_id)?;
        let operation = valid_uuid(&operation_id)?;
        let value: InspectionWire = decode(
            self.rpc
                .provider(ProviderRequest::InspectSave {
                    source_id: source.clone(),
                    operation_id: operation,
                })
                .await?,
        )?;
        if value.operation_id != operation_id {
            return Err(NativeProviderError::InvalidResponse);
        }
        let projection = value.projection.map(projection_dto).transpose()?;
        if projection.is_some() && !matches!(value.state, MutationStateDto::Published) {
            return Err(NativeProviderError::InvalidResponse);
        }
        if let Some(projection) = &projection {
            self.source_projection(&source, projection).await?;
        }
        Ok(NativeProviderResult::InspectSave {
            operation_id,
            state: value.state,
            projection,
        })
    }
}
