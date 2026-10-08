use super::*;

impl Spool {
    /// Service-only operation. Revalidates the current grant, then atomically creates
    /// or updates ordinary Markdown. Repeating after a crash preserves one occurrence.
    pub fn project(&self, id: &str, policy: &ProjectPolicy) -> Result<ProjectedCapture> {
        let _lock = self.lock()?;
        let record = self.read_pending_locked(id)?;
        let p = &record.provenance;
        if p.project_id != policy.project_id {
            return Err(CaptureError::UnsafePath);
        }
        let current =
            policy.authorize(&p.cwd, &p.session_id, p.subagent_id.as_deref(), p.origin)?;
        if current.project != p.project_root
            || current.repository != p.repository_root
            || current.worktree != p.worktree_root
        {
            return Err(CaptureError::UnsafePath);
        }
        let path = self
            .markdown_root()
            .join(format!("{}.md", record.content_id));
        let mut metadata = if regular_exists(&path)? {
            let (metadata, body) = parse_projection(&path, &self.limits)?;
            if metadata.content_id != record.content_id
                || metadata.content_revision != record.content_revision
                || body != record.body
            {
                return Err(CaptureError::Invalid);
            }
            metadata
        } else {
            ProjectionMetadata {
                schema_version: 1,
                content_id: record.content_id.clone(),
                content_revision: record.content_revision.clone(),
                capture_is_not_approval: true,
                occurrences: Vec::new(),
            }
        };
        if metadata
            .occurrences
            .iter()
            .any(|existing| existing.occurrence_id == id && existing != &record.provenance)
        {
            return Err(CaptureError::Invalid);
        }
        if !metadata
            .occurrences
            .iter()
            .any(|existing| existing.occurrence_id == id)
        {
            if metadata.occurrences.len() >= self.limits.max_occurrences_per_body {
                return Err(CaptureError::ProvenanceFull);
            }
            metadata.occurrences.push(record.provenance.clone());
            let json = serde_json::to_string(&metadata).map_err(|_| CaptureError::Invalid)?;
            let bytes = format!("{HEADER}{json}\n---\n\n{}", record.body).into_bytes();
            if bytes.len() > self.limits.max_projection_bytes {
                return Err(CaptureError::TooLarge);
            }
            self.atomic_write(&path, &bytes)?;
        } else {
            // A previous rename may have succeeded before directory sync failed.
            self.confirm_durable(&path)?;
        }
        Ok(ProjectedCapture {
            id: id.into(),
            path,
            record,
        })
    }
    /// Call only after the service commits capture admission and its index job in
    /// library.sqlite. No index success or model availability is required for durability.
    pub fn acknowledge(&self, id: &str) -> Result<()> {
        let _lock = self.lock()?;
        let receipt = self.receipt_path(id)?;
        let pending = self.pending_path(id)?;
        if regular_exists(&receipt)? && !regular_exists(&pending)? {
            let bytes = read_bounded(&receipt, 64)?;
            let content_id = std::str::from_utf8(&bytes).map_err(|_| CaptureError::Invalid)?;
            if !valid_id(content_id) {
                return Err(CaptureError::Invalid);
            }
            let metadata = read_projection(
                &self.markdown_root().join(format!("{content_id}.md")),
                &self.limits,
            )?;
            if !metadata.occurrences.iter().any(|p| p.occurrence_id == id) {
                return Err(CaptureError::NotProjected);
            }
            self.confirm_durable(&self.markdown_root().join(format!("{content_id}.md")))?;
            self.confirm_durable(&receipt)?;
            return Ok(());
        }
        let record = self.read_pending_locked(id)?;
        let metadata = read_projection(
            &self
                .markdown_root()
                .join(format!("{}.md", record.content_id)),
            &self.limits,
        )
        .map_err(|_| CaptureError::NotProjected)?;
        if !metadata.occurrences.iter().any(|p| p == &record.provenance) {
            return Err(CaptureError::NotProjected);
        }
        // Receipt is durable before removing the queue event. A crash here is replayable.
        if !regular_exists(&receipt)? {
            self.atomic_write(&receipt, record.content_id.as_bytes())?;
        } else if read_bounded(&receipt, 64)? != record.content_id.as_bytes() {
            return Err(CaptureError::Invalid);
        }
        self.confirm_durable(&receipt)?;
        fs::remove_file(pending)?;
        sync_directory(&self.root.join("pending"))?;
        Ok(())
    }
    /// Only incomplete temporary writes are discarded. Pending events and Markdown
    /// are never removed by recovery. This runs under the same nonblocking lock.
    pub fn recover_temporary(&self) -> Result<usize> {
        let _lock = self.lock()?;
        let mut count = 0;
        for entry in
            fs::read_dir(self.root.join("temporary"))?.take(self.limits.max_pending_events + 1)
        {
            let entry = entry?;
            ensure_regular_or_missing(&entry.path())?;
            fs::remove_file(entry.path())?;
            count += 1;
        }
        sync_directory(&self.root.join("temporary"))?;
        Ok(count)
    }
}
