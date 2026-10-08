use super::*;

pub struct PreparedSnapshot {
    pub(super) permit: ReadPermit,
    pub(super) bytes: Vec<u8>,
    pub(super) receipt: AssetReceipt,
    pub(super) manifest: String,
    pub(super) root: SourceRoot,
    pub(super) owner: Arc<()>,
}
pub struct VerifiedSnapshot {
    pub(super) _permit: ReadPermit,
    pub(super) bytes: Vec<u8>,
    pub(super) receipt: AssetReceipt,
    pub(super) manifest: String,
    pub(super) root: SourceRoot,
    pub(super) owner: Arc<()>,
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
            owner: self.owner,
        })
    }
}
struct Session {
    snapshot: VerifiedSnapshot,
    expires: Instant,
}
/// One owner profile: at most two read permits and one image validation at a time.
/// Clones share limits and sessions; use fresh instances after owner reopen.
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
    pub(in crate::device) fn archive_permit(
        &self,
        store: &mut ReaderStore,
        assets: &AssetStore,
    ) -> Result<ReadPermit> {
        self.bind_assets(store, assets)?;
        self.read_permit(Instant::now())
    }
    fn bind_assets(&self, store: &mut ReaderStore, assets: &AssetStore) -> Result<()> {
        assets.verify_owner(store)?;
        let mut binding = self.roots.lock().map_err(|_| unavailable())?;
        if let Some(current) = binding.as_ref() {
            current.same_owner_root(&assets.owner, &assets.root)?;
        } else {
            *binding = Some(assets.clone());
        }
        Ok(())
    }
    pub fn assets(&self, store: &mut ReaderStore, directory: &Path) -> Result<AssetStore> {
        let mut root = self.roots.lock().map_err(|_| unavailable())?;
        if root.is_none() {
            *root = Some(AssetStore::open(store, directory)?);
        }
        let assets = root.as_ref().ok_or_else(unavailable)?;
        if assets.root.path != directory.canonicalize().map_err(|_| unavailable())? {
            return Err(unavailable());
        }
        assets.verify_owner(store)?;
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
            .map_err(|_| Error::ReadsBusy)?;
        Ok(ReadPermit(self.reading.clone()))
    }
    pub fn validation_permit(&self) -> Result<ValidationPermit> {
        self.validating
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| Error::ValidationBusy)?;
        Ok(ValidationPermit(self.validating.clone()))
    }
    pub fn install(
        &self,
        assets: &AssetStore,
        store: &mut ReaderStore,
        snapshot: VerifiedSnapshot,
        now: Instant,
    ) -> Result<(String, AssetReceipt)> {
        self.bind_assets(store, assets)?;
        let mut sessions = self.sessions.lock().map_err(|_| unavailable())?;
        sessions.retain(|_, session| session.expires > now);
        if sessions.len() >= 2 {
            return Err(Error::ReadsBusy);
        }
        assets.verify_owner(store)?;
        assets.same_owner_root(&snapshot.owner, &snapshot.root)?;
        if !Arc::ptr_eq(&snapshot._permit.0, &self.reading) {
            return Err(unavailable());
        }
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
    ) -> Result<AssetChunk> {
        valid_id(id)?;
        if let Err(error) = self.bind_assets(store, assets) {
            self.sessions.lock().map_err(|_| unavailable())?.remove(id);
            return Err(error);
        }
        let mut sessions = self.sessions.lock().map_err(|_| unavailable())?;
        sessions.retain(|_, session| session.expires > now);
        let session = sessions.get(id).ok_or_else(unavailable)?;
        let snapshot = &session.snapshot;
        let current = assets
            .same_owner_root(&snapshot.owner, &snapshot.root)
            .and_then(|()| assets.check_selection(store, &snapshot.receipt, &snapshot.manifest));
        if let Err(error) = current {
            sessions.remove(id);
            return Err(error);
        }
        if offset as usize >= snapshot.bytes.len() || !(offset as usize).is_multiple_of(MAX_CHUNK) {
            sessions.remove(id);
            return Err(unavailable());
        }
        let end = (offset as usize + MAX_CHUNK).min(snapshot.bytes.len());
        let complete = end == snapshot.bytes.len();
        let base64 = STANDARD.encode(&snapshot.bytes[offset as usize..end]);
        if complete {
            sessions.remove(id);
        }
        Ok(AssetChunk {
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
