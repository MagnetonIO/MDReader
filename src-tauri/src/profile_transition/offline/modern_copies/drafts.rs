//! Exact intended/conflicting provider bodies become independent labelled copies.
//! No write permit, credential, provider authority or retry is restored.
use super::*;
pub(super) fn capture(owner: &mut ReaderStore, documents: &mut Vec<Document>) -> Result<()> {
    let records = owner
        .with_library_connection(|db| mdreader_provider::journal::archive(db))
        .map_err(|_| Error::Integrity)?;
    for record in records {
        let id = record.pending.operation_id.to_string();
        let state = serde_json::to_value(record.pending.state)
            .map_err(|_| Error::Integrity)?
            .as_str()
            .ok_or(Error::Integrity)?
            .to_owned();
        let version = record
            .pending
            .receipt
            .as_ref()
            .map(|r| &r.version)
            .unwrap_or(&record.pending.expected);
        let revision = blake3::hash(&serde_json::to_vec(version).map_err(|_| Error::Integrity)?)
            .to_hex()
            .to_string();
        let lineage = Lineage {
            kind: "provider_draft".into(),
            source_id: format!(
                "provider:{}",
                mdreader_provider::hex(&record.binding.digest())
            ),
            path: record.pending.key.as_str().into(),
            revision,
            document_id: None,
            operation_id: Some(id.clone()),
            state: Some(state.clone()),
        };
        let body = String::from_utf8(record.pending.body.as_bytes().to_vec())
            .map_err(|_| Error::Integrity)?;
        owned::push(
            documents,
            &format!("Recovered provider draft ({state})"),
            body,
            lineage,
            1,
        )?;
        if let Some(conflict) = record.conflict {
            let revision =
                blake3::hash(&serde_json::to_vec(&conflict.version).map_err(|_| Error::Integrity)?)
                    .to_hex()
                    .to_string();
            let body = String::from_utf8(conflict.body.as_bytes().to_vec())
                .map_err(|_| Error::Integrity)?;
            owned::push(
                documents,
                "Recovered conflicting provider copy",
                body,
                Lineage {
                    kind: "provider_conflict".into(),
                    source_id: format!(
                        "provider:{}",
                        mdreader_provider::hex(&record.binding.digest())
                    ),
                    path: conflict.key.as_str().into(),
                    revision,
                    document_id: None,
                    operation_id: Some(id),
                    state: Some("conflict".into()),
                },
                1,
            )?;
        }
    }
    Ok(())
}
