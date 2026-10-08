//! Durable dependency-first progress within one foreground run's existing gate.
use super::*;
impl Transport {
    /// false means an explicit next foreground run is needed, not a failed upload.
    /// Successfully received scoped receipts stay durable before budget exhaustion.
    pub(super) async fn upload_outbox<O: Owner, A: Authority>(
        &self,
        owner: &O,
        session: &mut Session<'_, A>,
        entry: &OutboxEntry,
    ) -> TransportResult<bool> {
        let scope = session.scope.clone();
        let operation = &entry.operation.operation_id;
        let dependencies = owner.with_store(&scope, |s| s.dependencies(operation))?;
        for dependency in dependencies {
            if dependency.uploaded {
                continue;
            }
            if dependency.envelope_utf8.len().saturating_add(4096) > session.remaining {
                return Ok(false);
            }
            let url = self.url(
                &format!("/v1/blobs/{}", dependency.blob),
                &[
                    ("operationId", operation.clone()),
                    ("libraryId", scope.library_id().into()),
                ],
            )?;
            let (status, receipt) = self
                .request(
                    owner,
                    session,
                    Method::PUT,
                    url,
                    dependency.envelope_utf8,
                    4096,
                )
                .await?;
            owner.with_store(&scope, |s| {
                s.mark_dependency_uploaded(&scope, operation, &dependency.blob, status, &receipt)
            })?;
        }
        let commit_bytes = entry.operation.wire_bytes()?.len();
        if let Some(body) = &entry.envelope_utf8 {
            // Legacy root-only entries always obtain a fresh strict receipt. New
            // graphs retain the exact scoped root receipt atomically with state.
            let confirmed = owner.with_store(&scope, |s| s.graph_root_uploaded(operation))?;
            let required = commit_bytes
                .saturating_add(4096)
                .saturating_add(if confirmed {
                    0
                } else {
                    body.len().saturating_add(4096)
                });
            if required > session.remaining {
                return Ok(false);
            }
            if !confirmed {
                let blob = entry
                    .operation
                    .blob
                    .as_deref()
                    .ok_or(TransportError::Protocol)?;
                let url = self.url(
                    &format!("/v1/blobs/{blob}"),
                    &[
                        ("operationId", operation.clone()),
                        ("libraryId", scope.library_id().into()),
                    ],
                )?;
                let (status, receipt) = self
                    .request(owner, session, Method::PUT, url, body.clone(), 4096)
                    .await?;
                owner.with_store(&scope, |s| {
                    s.mark_uploaded_scoped(&scope, operation, status, &receipt)
                })?;
            }
        } else if commit_bytes.saturating_add(4096) > session.remaining {
            return Ok(false);
        }
        Ok(true)
    }
}
