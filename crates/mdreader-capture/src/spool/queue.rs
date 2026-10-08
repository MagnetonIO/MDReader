use super::*;

impl Spool {
    pub(super) fn status_locked(&self) -> Result<QueueStatus> {
        let mut status = QueueStatus {
            pending_events: 0,
            pending_bytes: 0,
            orphan_temporary_files: 0,
        };
        for entry in
            fs::read_dir(self.root.join("pending"))?.take(self.limits.max_pending_events + 1)
        {
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if !meta.is_file() || meta.file_type().is_symlink() {
                return Err(CaptureError::UnsafePath);
            }
            status.pending_events += 1;
            status.pending_bytes = status.pending_bytes.saturating_add(meta.len());
        }
        status.orphan_temporary_files = fs::read_dir(self.root.join("temporary"))?
            .take(self.limits.max_pending_events + 1)
            .count();
        Ok(status)
    }
    pub fn status(&self) -> Result<QueueStatus> {
        let _lock = self.lock()?;
        self.status_locked()
    }
    /// Deterministic bounded batch. Invalid records remain pending for explicit recovery.
    pub fn pending(&self, limit: usize) -> Result<Vec<String>> {
        let _lock = self.lock()?;
        let mut ids = Vec::new();
        for entry in
            fs::read_dir(self.root.join("pending"))?.take(self.limits.max_pending_events + 1)
        {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_str().ok_or(CaptureError::Invalid)?;
            let id = name
                .strip_suffix(".json")
                .filter(|s| valid_id(s))
                .ok_or(CaptureError::Invalid)?;
            ensure_regular_or_missing(&entry.path())?;
            ids.push(id.into());
        }
        ids.sort();
        ids.truncate(limit.min(self.limits.max_pending_events));
        Ok(ids)
    }
    pub fn read_pending(&self, id: &str) -> Result<CaptureRecord> {
        let _lock = self.lock()?;
        self.read_pending_locked(id)
    }
    pub(super) fn read_pending_locked(&self, id: &str) -> Result<CaptureRecord> {
        let bytes = read_bounded(&self.pending_path(id)?, self.limits.max_event_bytes)?;
        let record: CaptureRecord =
            serde_json::from_slice(&bytes).map_err(|_| CaptureError::Invalid)?;
        validate_record(&record)?;
        if record.id != id {
            return Err(CaptureError::Invalid);
        }
        Ok(record)
    }
}
