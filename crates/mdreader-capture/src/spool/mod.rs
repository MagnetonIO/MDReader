use crate::*;
use fs2::FileExt;

mod freeze;
mod projection;
mod queue;
pub use freeze::FrozenSpool;

// flock belongs to the open file description. Explicit unlock on owner drop
// prevents an unrelated fork's briefly inherited descriptor from extending the
// critical section after our operation has already returned.
struct QueueLock(File);
impl std::ops::Deref for QueueLock {
    type Target = File;
    fn deref(&self) -> &File {
        &self.0
    }
}
impl Drop for QueueLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

pub struct Spool {
    root: PathBuf,
    limits: Limits,
    authority: platform::DirectoryAuthority,
}
impl Spool {
    pub fn open(root: impl Into<PathBuf>, limits: Limits) -> Result<Self> {
        limits.validate()?;
        let root = root.into();
        // Unsupported hosts fail before creating a directory or touching content.
        #[cfg(not(unix))]
        sync_directory(&root)?;
        private_directory(&root)?;
        let root = fs::canonicalize(root)?;
        for name in ["pending", "markdown", "receipts", "temporary"] {
            private_directory(&root.join(name))?;
        }
        let authority = platform::DirectoryAuthority::capture(&root)?;
        let spool = Self {
            root,
            limits,
            authority,
        };
        let lock = spool.lock()?;
        drop(lock);
        Ok(spool)
    }
    pub fn markdown_root(&self) -> PathBuf {
        self.root.join("markdown")
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn limits(&self) -> &Limits {
        &self.limits
    }
    fn verify(&self) -> Result<()> {
        for path in [
            &self.root,
            &self.root.join("pending"),
            &self.root.join("markdown"),
            &self.root.join("receipts"),
            &self.root.join("temporary"),
        ] {
            let meta = fs::symlink_metadata(path)?;
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err(CaptureError::UnsafePath);
            }
        }
        self.authority.verify(&self.root)?;
        Ok(())
    }
    fn lock(&self) -> Result<QueueLock> {
        self.verify()?;
        let path = self.root.join("queue.lock");
        ensure_regular_or_missing(&path)?;
        let file = private_open(&path, false)?;
        file.try_lock_exclusive().map_err(|e| {
            if platform::lock_would_block(&e) {
                CaptureError::Busy
            } else {
                CaptureError::Io(e)
            }
        })?;
        self.verify()?;
        Ok(QueueLock(file))
    }
    fn pending_path(&self, id: &str) -> Result<PathBuf> {
        if !valid_id(id) {
            return Err(CaptureError::Invalid);
        }
        Ok(self.root.join("pending").join(format!("{id}.json")))
    }
    fn receipt_path(&self, id: &str) -> Result<PathBuf> {
        if !valid_id(id) {
            return Err(CaptureError::Invalid);
        }
        Ok(self.root.join("receipts").join(id))
    }
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
        let result = self.prepare(adapter, bytes, policy, origin);
        let record = match result {
            Ok(r) => r,
            Err(CaptureError::Excluded(reason)) => return Ok(EnqueueOutcome::Skipped { reason }),
            Err(e) => return Err(e),
        };
        let bytes = serde_json::to_vec(&record).map_err(|_| CaptureError::Invalid)?;
        if bytes.len() > self.limits.max_event_bytes {
            return Err(CaptureError::TooLarge);
        }
        let _lock = self.lock()?;
        let path = self.pending_path(&record.id)?;
        let receipt = self.receipt_path(&record.id)?;
        if regular_exists(&path)? {
            self.read_pending_locked(&record.id)?;
            self.confirm_durable(&path)?;
            return Ok(EnqueueOutcome::Duplicate { id: record.id });
        }
        if regular_exists(&receipt)? {
            if read_bounded(&receipt, 64)? != record.content_id.as_bytes() {
                return Err(CaptureError::Invalid);
            }
            let projection = self
                .markdown_root()
                .join(format!("{}.md", record.content_id));
            let metadata = read_projection(&projection, &self.limits)
                .map_err(|_| CaptureError::NotProjected)?;
            if metadata.content_id != record.content_id
                || metadata.content_revision != record.content_revision
                || !metadata
                    .occurrences
                    .iter()
                    .any(|p| p.occurrence_id == record.id)
            {
                return Err(CaptureError::NotProjected);
            }
            self.confirm_durable(&projection)?;
            self.confirm_durable(&receipt)?;
            return Ok(EnqueueOutcome::Duplicate { id: record.id });
        }
        let status = self.status_locked()?;
        if status.pending_events >= self.limits.max_pending_events
            || status.pending_bytes.saturating_add(bytes.len() as u64)
                > self.limits.max_pending_bytes
        {
            return Err(CaptureError::QueueFull);
        }
        self.atomic_write(&path, &bytes)?;
        Ok(EnqueueOutcome::Queued { id: record.id })
    }
    fn prepare(
        &self,
        adapter: Adapter,
        bytes: &[u8],
        policy: &ProjectPolicy,
        origin: CaptureOrigin,
    ) -> Result<CaptureRecord> {
        let output = adapt(adapter, bytes)?;
        let roots = policy.authorize(
            &output.cwd,
            &output.session,
            output.subagent.as_deref(),
            origin,
        )?;
        let mut record = CaptureRecord {
            schema_version: 1,
            id: String::new(),
            content_id: hash(&(&policy.project_id, &output.body))?,
            content_revision: blake3::hash(output.body.as_bytes()).to_hex().to_string(),
            body: output.body,
            provenance: Provenance {
                occurrence_id: String::new(),
                project_id: policy.project_id.clone(),
                project_root: roots.project,
                repository_root: roots.repository,
                worktree_root: roots.worktree,
                cwd: roots.cwd,
                adapter,
                lifecycle_event: output.event,
                session_id: output.session,
                turn_id: output.turn,
                turn_id_kind: output.turn_kind,
                subagent_id: output.subagent,
                subagent_type: output.agent_type,
                repository_revision: None,
                origin,
                captured_unix_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| CaptureError::Invalid)?
                    .as_millis()
                    .try_into()
                    .map_err(|_| CaptureError::Invalid)?,
            },
        };
        record.id = identity(&record)?;
        record.provenance.occurrence_id = record.id.clone();
        Ok(record)
    }
    fn confirm_durable(&self, path: &Path) -> Result<()> {
        platform::confirm_file(path)?;
        sync_directory(path.parent().ok_or(CaptureError::UnsafePath)?)
    }
    fn atomic_write(&self, destination: &Path, bytes: &[u8]) -> Result<()> {
        ensure_regular_or_missing(destination)?;
        let temporary = self.root.join("temporary").join(format!(
            "{}-{}.tmp",
            std::process::id(),
            blake3::hash(bytes).to_hex()
        ));
        let mut file = private_open(&temporary, true)?;
        let result = (|| {
            file.write_all(bytes)?;
            file.sync_all()?;
            platform::replace(&file, &temporary, destination)?;
            sync_directory(destination.parent().ok_or(CaptureError::UnsafePath)?)?;
            sync_directory(&self.root.join("temporary"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

#[cfg(all(test, unix))]
mod lock_lifetime_tests {
    use super::*;
    #[test]
    fn owner_drop_releases_lock_even_when_a_fork_inherits_the_description() {
        let temp = tempfile::tempdir().unwrap();
        let spool = Spool::open(temp.path().join("spool"), Limits::default()).unwrap();
        let guard = spool.lock().unwrap();
        // dup models the same open file description inherited between fork and
        // exec by an unrelated concurrently launched process.
        let inherited = guard.try_clone().unwrap();
        drop(guard);
        assert!(
            spool.pending(1).is_ok(),
            "owner drop must unlock independently of inherited handles"
        );
        drop(inherited);
    }
}
