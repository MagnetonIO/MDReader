use super::*;
impl Spool {
    pub fn enqueue(
        &self,
        adapter: Adapter,
        bytes: &[u8],
        policy: &ProjectPolicy,
        origin: CaptureOrigin,
    ) -> Result<EnqueueOutcome> {
        if bytes.len() > self.limits.max_event_bytes {
            return Err(CaptureError::TooLarge);
        }
        let record = match self.prepare(adapter, bytes, policy, origin) {
            Ok(record) => record,
            Err(CaptureError::Excluded(reason)) => return Ok(EnqueueOutcome::Skipped { reason }),
            Err(error) => return Err(error),
        };
        let bytes = serde_json::to_vec(&record).map_err(|_| CaptureError::Invalid)?;
        if bytes.len() > self.limits.max_event_bytes {
            return Err(CaptureError::TooLarge);
        }
        let _lock = self.lock()?;
        let journal = Journal::open(self)?;
        if journal.pending(&record.id, &self.limits)?.is_some() {
            return Ok(EnqueueOutcome::Duplicate { id: record.id });
        }
        let receipt: Option<String> = journal
            .connection
            .query_row(
                "SELECT content_id FROM capture_receipts WHERE id=?1",
                [&record.id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(content) = receipt {
            if content != record.content_id {
                return Err(CaptureError::Invalid);
            }
            let bytes = journal
                .body(&content, &self.limits)?
                .ok_or(CaptureError::NotProjected)?;
            let (metadata, _) = crate::projection::parse_projection_bytes(&bytes, &self.limits)?;
            if metadata.content_revision != record.content_revision
                || !metadata
                    .occurrences
                    .iter()
                    .any(|p| p.occurrence_id == record.id)
            {
                return Err(CaptureError::NotProjected);
            }
            return Ok(EnqueueOutcome::Duplicate { id: record.id });
        }
        let status = Self::journal_status(&journal)?;
        if status.pending_events >= self.limits.max_pending_events
            || status.pending_bytes.saturating_add(bytes.len() as u64)
                > self.limits.max_pending_bytes
        {
            return Err(CaptureError::QueueFull);
        }
        journal.connection.execute(
            "INSERT INTO capture_pending VALUES(?1,?2,?3)",
            (&record.id, &bytes, bytes.len()),
        )?;
        self.verify()?;
        Ok(EnqueueOutcome::Queued { id: record.id })
    }
    fn journal_status(journal: &Journal) -> Result<QueueStatus> {
        let (count, bytes) = journal.connection.query_row(
            "SELECT count(*),coalesce(sum(bytes),0) FROM capture_pending",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(QueueStatus {
            pending_events: count,
            pending_bytes: bytes,
            orphan_temporary_files: 0,
        })
    }
    pub fn status(&self) -> Result<QueueStatus> {
        let _lock = self.lock()?;
        Self::journal_status(&Journal::open(self)?)
    }
    pub fn pending(&self, limit: usize) -> Result<Vec<String>> {
        let _lock = self.lock()?;
        let journal = Journal::open(self)?;
        let mut stmt = journal
            .connection
            .prepare("SELECT id FROM capture_pending ORDER BY id LIMIT ?1")?;
        let ids = stmt
            .query_map([limit.min(self.limits.max_pending_events)], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if ids.iter().any(|id| !valid_id(id)) {
            return Err(CaptureError::Invalid);
        }
        Ok(ids)
    }
    pub fn read_pending(&self, id: &str) -> Result<CaptureRecord> {
        let _lock = self.lock()?;
        Journal::open(self)?
            .pending(id, &self.limits)?
            .ok_or(CaptureError::Invalid)
    }
}
