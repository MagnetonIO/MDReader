use super::*;

pub struct PreparedSnapshot {
    pub(super) permit: ReadPermit,
    pub(super) bytes: Vec<u8>,
    pub(super) receipt: AssetReceipt,
    pub(super) manifest: String,
    pub(super) root: SourceRoot,
}
pub struct VerifiedSnapshot {
    pub(super) _permit: ReadPermit,
    pub(super) bytes: Vec<u8>,
    pub(super) receipt: AssetReceipt,
    pub(super) manifest: String,
    pub(super) root: SourceRoot,
}
impl PreparedSnapshot {
    pub fn verify(self) -> Result<VerifiedSnapshot> {
        if digest(&self.bytes) != self.receipt.content_hash {
            return Err(unavailable());
        }
        self.root.verify().map_err(|_| unavailable())?;
        Ok(VerifiedSnapshot {
            _permit: self.permit,
            bytes: self.bytes,
            receipt: self.receipt,
            manifest: self.manifest,
            root: self.root,
        })
    }
}
struct Session {
    snapshot: VerifiedSnapshot,
    expires: Instant,
}
#[derive(Default, Clone)]
pub struct AssetRuntime {
    roots: Arc<Mutex<Option<AssetStore>>>,
    reading: Arc<AtomicUsize>,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    validating: Arc<AtomicBool>,
}
pub struct ReadPermit(Arc<AtomicUsize>);
impl Drop for ReadPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
pub struct ValidationPermit(Arc<AtomicBool>);
impl Drop for ValidationPermit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl AssetRuntime {
    pub fn assets(&self, store: &mut ReaderStore, directory: &Path) -> Result<AssetStore> {
        let mut root = self.roots.lock().map_err(|_| unavailable())?;
        if root.is_none() {
            *root = Some(AssetStore::open(store, directory)?);
        }
        let assets = root.as_ref().ok_or_else(unavailable)?;
        if assets.root.path != directory.canonicalize().map_err(|_| unavailable())? {
            return Err(unavailable());
        }
        assets.verify()?;
        Ok(assets.clone())
    }
    pub fn read_permit(&self, now: Instant) -> Result<ReadPermit> {
        self.sessions
            .lock()
            .map_err(|_| unavailable())?
            .retain(|_, session| session.expires > now);
        self.reading
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < 2).then_some(count + 1)
            })
            .map_err(|_| "Two image reads are already active.".to_string())?;
        Ok(ReadPermit(self.reading.clone()))
    }
    pub fn validation_permit(&self) -> Result<ValidationPermit> {
        self.validating
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "An image is being validated. Try again when it completes.".to_string())?;
        Ok(ValidationPermit(self.validating.clone()))
    }
    pub fn install(
        &self,
        assets: &AssetStore,
        store: &mut ReaderStore,
        snapshot: VerifiedSnapshot,
        now: Instant,
    ) -> Result<(String, AssetReceipt)> {
        let mut sessions = self.sessions.lock().map_err(|_| unavailable())?;
        sessions.retain(|_, session| session.expires > now);
        if sessions.len() >= 2 {
            return Err("Two image reads are already active.".into());
        }
        snapshot.root.verify().map_err(|_| unavailable())?;
        assets.check_selection(store, &snapshot.receipt, &snapshot.manifest)?;
        let id = uuid::Uuid::new_v4().to_string();
        let receipt = snapshot.receipt.clone();
        sessions.insert(
            id.clone(),
            Session {
                snapshot,
                expires: now + SESSION_TTL,
            },
        );
        Ok((id, receipt))
    }
    pub fn chunk(
        &self,
        assets: &AssetStore,
        store: &mut ReaderStore,
        id: &str,
        offset: u32,
        now: Instant,
    ) -> Result<AssetReply> {
        valid_id(id)?;
        let mut sessions = self.sessions.lock().map_err(|_| unavailable())?;
        sessions.retain(|_, session| session.expires > now);
        let session = sessions.get(id).ok_or_else(unavailable)?;
        let snapshot = &session.snapshot;
        let current = snapshot
            .root
            .verify()
            .map_err(|_| unavailable())
            .and_then(|()| assets.check_selection(store, &snapshot.receipt, &snapshot.manifest));
        let valid = current.is_ok();
        if !valid
            || offset as usize >= snapshot.bytes.len()
            || !(offset as usize).is_multiple_of(MAX_CHUNK)
        {
            sessions.remove(id);
            return Err(unavailable());
        }
        let end = (offset as usize + MAX_CHUNK).min(snapshot.bytes.len());
        let complete = end == snapshot.bytes.len();
        let base64 = STANDARD.encode(&snapshot.bytes[offset as usize..end]);
        if complete {
            sessions.remove(id);
        }
        Ok(AssetReply::Chunk {
            offset,
            base64,
            complete,
        })
    }
    pub fn close(&self, id: &str) -> Result<()> {
        valid_id(id)?;
        self.sessions.lock().map_err(|_| unavailable())?.remove(id);
        Ok(())
    }
}
