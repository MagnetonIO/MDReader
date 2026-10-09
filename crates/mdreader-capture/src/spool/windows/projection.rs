use super::*;
use crate::platform::windows::trace_step;
use crate::projection::parse_projection_bytes;
impl Spool {
    fn materialize(&self, content: &str, bytes: &[u8]) -> Result<bool> {
        let (metadata, _) = parse_projection_bytes(bytes, &self.limits)?;
        if metadata.content_id != content {
            return Err(CaptureError::Invalid);
        }
        let path = self.markdown_root().join(format!("{content}.md"));
        if trace_step!("projection.destination_exists", regular_exists(&path))?
            && trace_step!(
                "projection.destination_read",
                read_bounded(&path, self.limits.max_projection_bytes)
            )? == bytes
        {
            return Ok(false);
        }
        let temporary = self.root.join("temporary").join(format!(
            "{}-{}.tmp",
            std::process::id(),
            blake3::hash(bytes).to_hex()
        ));
        // An interrupted prior write remains inspectable; never truncate an
        // unverified temporary object. The separate recovery action removes it.
        let mut file = trace_step!("projection.temp_create", private_open(&temporary, true))?;
        trace_step!("projection.temp_write", file.write_all(bytes))?;
        trace_step!("projection.temp_flush", file.sync_all())?;
        trace_step!(
            "projection.replace",
            platform::replace(&file, &temporary, &path)
        )?;
        drop(file);
        if trace_step!(
            "projection.readback",
            read_bounded(&path, self.limits.max_projection_bytes)
        )? != bytes
        {
            return Err(CaptureError::UnsafePath);
        }
        trace_step!("projection.spool_verify", self.verify())?;
        Ok(true)
    }
    pub fn project(&self, id: &str, policy: &ProjectPolicy) -> Result<ProjectedCapture> {
        let _lock = trace_step!("project.queue_lock", self.lock())?;
        let journal = trace_step!("project.journal_open", Journal::open(self))?;
        let record = journal
            .pending(id, &self.limits)?
            .ok_or(CaptureError::Invalid)?;
        let p = &record.provenance;
        let current =
            policy.authorize(&p.cwd, &p.session_id, p.subagent_id.as_deref(), p.origin)?;
        if p.project_id != policy.project_id
            || current.project != p.project_root
            || current.repository != p.repository_root
            || current.worktree != p.worktree_root
        {
            return Err(CaptureError::UnsafePath);
        }
        let mut metadata = if let Some(bytes) = journal.body(&record.content_id, &self.limits)? {
            let (metadata, body) = parse_projection_bytes(&bytes, &self.limits)?;
            if body != record.body || metadata.content_revision != record.content_revision {
                return Err(CaptureError::Invalid);
            }
            metadata
        } else {
            ProjectionMetadata {
                schema_version: 1,
                content_id: record.content_id.clone(),
                content_revision: record.content_revision.clone(),
                capture_is_not_approval: true,
                occurrences: vec![],
            }
        };
        if metadata
            .occurrences
            .iter()
            .any(|existing| existing.occurrence_id == id && existing != p)
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
            metadata.occurrences.push(p.clone());
        }
        let json = serde_json::to_string(&metadata).map_err(|_| CaptureError::Invalid)?;
        let bytes = format!("{HEADER}{json}\n---\n\n{}", record.body).into_bytes();
        if bytes.len() > self.limits.max_projection_bytes {
            return Err(CaptureError::TooLarge);
        }
        // The canonical body survives process/power loss before materialization.
        // Pending remains until the real reader owner has committed admission.
        trace_step!("project.canonical_commit", journal.connection.execute("INSERT INTO capture_bodies VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET projection=excluded.projection,bytes=excluded.bytes",(&record.content_id,&bytes,bytes.len())))?;
        self.materialize(&record.content_id, &bytes)?;
        Ok(ProjectedCapture {
            id: id.into(),
            path: self
                .markdown_root()
                .join(format!("{}.md", record.content_id)),
            record,
        })
    }
    pub fn acknowledge(&self, id: &str) -> Result<()> {
        let _lock = self.lock()?;
        let mut journal = Journal::open(self)?;
        let record = journal.pending(id, &self.limits)?;
        let content = if let Some(record) = &record {
            record.content_id.clone()
        } else {
            journal
                .connection
                .query_row(
                    "SELECT content_id FROM capture_receipts WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or(CaptureError::Invalid)?
        };
        let bytes = journal
            .body(&content, &self.limits)?
            .ok_or(CaptureError::NotProjected)?;
        let (metadata, _) = parse_projection_bytes(&bytes, &self.limits)?;
        if !metadata
            .occurrences
            .iter()
            .any(|p| p.occurrence_id == id && record.as_ref().is_none_or(|r| p == &r.provenance))
        {
            return Err(CaptureError::NotProjected);
        }
        let path = self.markdown_root().join(format!("{content}.md"));
        if read_bounded(&path, self.limits.max_projection_bytes)? != bytes {
            return Err(CaptureError::NotProjected);
        }
        let transaction = journal.connection.transaction()?;
        let previous: Option<String> = transaction
            .query_row(
                "SELECT content_id FROM capture_receipts WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        if previous.as_ref().is_some_and(|p| p != &content) {
            return Err(CaptureError::Invalid);
        }
        transaction.execute(
            "INSERT OR IGNORE INTO capture_receipts VALUES(?1,?2)",
            (id, &content),
        )?;
        transaction.execute("DELETE FROM capture_pending WHERE id=?1", [id])?;
        transaction.commit()?;
        self.verify()
    }
    /// Explicit service repair, bounded and restricted to currently granted
    /// projects. Hooks only enqueue; they never publish or scan Markdown.
    pub fn repair_projections(
        &self,
        policies: &[ProjectPolicy],
        limit: usize,
    ) -> Result<Vec<String>> {
        let _lock = self.lock()?;
        let journal = Journal::open(self)?;
        let after: String = journal.connection.query_row(
            "SELECT repair_after FROM capture_meta WHERE id=1 AND length(repair_after)<=64",
            [],
            |r| r.get(0),
        )?;
        if !after.is_empty() && !valid_id(&after) {
            return Err(CaptureError::Invalid);
        }
        let mut statement = journal
            .connection
            .prepare("SELECT id FROM capture_bodies WHERE id>?1 ORDER BY id LIMIT ?2")?;
        let ids = statement
            .query_map((&after, limit.min(8)), |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        let mut changed = Vec::new();
        for id in &ids {
            let bytes = journal
                .body(id, &self.limits)?
                .ok_or(CaptureError::Invalid)?;
            let (metadata, _) = parse_projection_bytes(&bytes, &self.limits)?;
            let project = &metadata
                .occurrences
                .first()
                .ok_or(CaptureError::Invalid)?
                .project_id;
            if let Some(policy) = policies
                .iter()
                .find(|p| p.enabled && &p.project_id == project)
            {
                let eligible = metadata.occurrences.iter().all(|p| {
                    policy
                        .authorize(&p.cwd, &p.session_id, p.subagent_id.as_deref(), p.origin)
                        .is_ok_and(|roots| {
                            roots.project == p.project_root
                                && roots.repository == p.repository_root
                                && roots.worktree == p.worktree_root
                        })
                });
                if eligible && self.materialize(id, &bytes)? && !changed.contains(project) {
                    changed.push(project.clone());
                }
            }
            journal
                .connection
                .execute("UPDATE capture_meta SET repair_after=?1 WHERE id=1", [id])?;
        }
        if ids.is_empty() {
            journal
                .connection
                .execute("UPDATE capture_meta SET repair_after='' WHERE id=1", [])?;
        }
        Ok(changed)
    }
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
        // Temporary names are not canonical state; no queue receipt is issued.
        self.verify()?;
        Ok(count)
    }
}
