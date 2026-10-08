//! Immutable root transfer, durable receiving and applied-only acknowledgement.
use super::*;
impl Transport {
    pub async fn run<O: Owner, A: Authority>(
        &self,
        owner: &O,
        authority: &A,
        budget: Budget,
    ) -> TransportResult<RunReport> {
        tokio::time::timeout(
            Duration::from_secs(60),
            self.run_inner(owner, authority, budget),
        )
        .await
        .map_err(|_| TransportError::Retryable)?
    }
    async fn run_inner<O: Owner, A: Authority>(
        &self,
        owner: &O,
        authority: &A,
        budget: Budget,
    ) -> TransportResult<RunReport> {
        budget.validate()?;
        let scope = authority.scope()?;
        let generation = authority.generation()?;
        self.check(authority, &scope, generation)?;
        let mut session = Session {
            authority,
            scope: scope.clone(),
            generation,
            remaining: budget.transfer_bytes,
        };
        {
            let mut scopes = self.running.lock().map_err(|_| TransportError::Busy)?;
            if !scopes.insert(scope.clone()) {
                return Err(TransportError::Busy);
            }
        }
        let _guard = RunGuard {
            running: self.running,
            scope: scope.clone(),
        };
        let entries = owner.with_store(&scope, |s| s.pending(budget.outgoing.max(1)))?;
        let mut report = RunReport {
            remote_pending: budget.pages == 0,
            ..RunReport::default()
        };
        let mut paused = false;
        for entry in entries.into_iter().take(budget.outgoing) {
            if !self.upload_outbox(owner, &mut session, &entry).await? {
                paused = true;
                report.remote_pending = true;
                break;
            }
            self.check(authority, &scope, generation)?;
            // Recheck the exact immutable snapshot after network, including owner binding.
            owner.with_store(&scope, |s| {
                let current = s
                    .entry(&entry.operation.operation_id)?
                    .ok_or(SyncError::NotReady)?;
                if current.operation != entry.operation {
                    return Err(SyncError::Reused);
                }
                s.require_dependencies_uploaded(&entry.operation.operation_id)
            })?;
            let (status, bytes) = self
                .request(
                    owner,
                    &mut session,
                    Method::POST,
                    self.url("/v1/commit", &[])?,
                    entry.operation.wire_bytes()?,
                    4096,
                )
                .await?;
            let receipt = owner.with_store(&scope, |s| s.commit_receipt(&scope, status, &bytes))?;
            report.committed += 1;
            report.conflicts += usize::from(receipt.conflict);
        }
        for _ in 0..if paused { 0 } else { budget.pages } {
            self.check(authority, &scope, generation)?;
            let after = owner.with_store(&scope, |s| s.cursors())?.received;
            let (status, bytes) = self
                .request(
                    owner,
                    &mut session,
                    Method::GET,
                    self.url(
                        "/v1/changes",
                        &[
                            ("after", after.to_string()),
                            ("limit", budget.page_size.to_string()),
                        ],
                    )?,
                    Vec::new(),
                    256 * 1024,
                )
                .await?;
            let page = crate::store::ChangesPage::parse(&bytes, after)?;
            if page.changes.len() > budget.page_size {
                return Err(TransportError::Protocol);
            }
            let mut roots = Vec::new();
            for change in &page.changes {
                if let Some(blob) = &change.blob {
                    let (_, body) = self
                        .request(
                            owner,
                            &mut session,
                            Method::GET,
                            self.url(&format!("/v1/blobs/{blob}"), &[])?,
                            Vec::new(),
                            crypto::MAX_ENVELOPE_BYTES,
                        )
                        .await?;
                    roots.push(authority.authenticate_root(&scope, change, &body)?);
                }
            }
            self.check(authority, &scope, generation)?;
            owner.with_store(&scope, |s| {
                s.stage_changes(&scope, status, after, &bytes, &roots)
            })?;
            report.remote_pending = page.has_more;
            if !page.has_more {
                break;
            }
        }
        let cursors = owner.with_store(&scope, |s| s.cursors())?;
        if !paused && cursors.applied > cursors.server_acknowledged {
            let body = serde_json::to_vec(&serde_json::json!({"cursor":cursors.applied}))
                .map_err(|_| TransportError::Protocol)?;
            let (status, bytes) = self
                .request(
                    owner,
                    &mut session,
                    Method::POST,
                    self.url("/v1/ack", &[])?,
                    body,
                    4096,
                )
                .await?;
            owner.with_store(&scope, |s| {
                s.confirm_cursor_ack(&scope, status, cursors.applied, &bytes)
            })?;
        }
        self.check(authority, &scope, generation)?;
        let cursors = owner.with_store(&scope, |s| s.cursors())?;
        report.received = cursors.received;
        report.applied = cursors.applied;
        report.acknowledged = cursors.server_acknowledged;
        report.local_pending = owner.with_store(&scope, |s| s.pending(1))?.len() == 1;
        Ok(report)
    }
}
