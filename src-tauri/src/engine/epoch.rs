use super::*;
/// One native grant era, shared by filesystem reads, watcher and service work.
pub struct Epoch {
    root_authority: SourceRoot,
    source_id: Option<uuid::Uuid>,
    pub(crate) root: PathBuf,
    #[cfg(test)]
    pub(crate) root_identity: mdreader_core::SourceRootIdentity,
    pub(crate) generation: u64,
    pub(crate) read_receipt: uuid::Uuid,
    pub(super) status: Mutex<ImportProgress>,
    pub(super) active: Mutex<bool>,
}
impl Epoch {
    #[cfg(test)]
    pub(crate) fn new(root: SourceRoot, generation: u64) -> Arc<Self> {
        Self::new_with_source(root, generation, None)
    }
    pub(crate) fn new_with_source(
        root: SourceRoot,
        generation: u64,
        source_id: Option<uuid::Uuid>,
    ) -> Arc<Self> {
        Arc::new(Self {
            root: root.path.clone(),
            #[cfg(test)]
            root_identity: root.identity.clone(),
            root_authority: root,
            source_id,
            generation,
            read_receipt: uuid::Uuid::new_v4(),
            status: Mutex::new(ImportProgress::default()),
            active: Mutex::new(false),
        })
    }
    pub(crate) fn validate_root(&self) -> Result<(), String> {
        self.root_authority.verify().map_err(|e| e.to_string())
    }
    pub(crate) fn source_root(&self) -> SourceRoot {
        self.root_authority.clone()
    }
    pub(crate) fn source_id(&self) -> Option<uuid::Uuid> {
        self.source_id
    }
    pub(super) fn status(&self) -> ImportProgress {
        self.status.lock().map(|v| v.clone()).unwrap_or_default()
    }
    pub(crate) fn finish<T>(
        self: &Arc<Self>,
        registry: &WorkspaceState,
        value: T,
    ) -> Result<T, StaleEpoch> {
        if !registry.is_current(self) {
            return Err(StaleEpoch::Superseded);
        }
        self.validate_root()
            .map_err(StaleEpoch::SourceIdentityChanged)?;
        Ok(value)
    }
}
#[derive(Debug)]
pub(crate) enum StaleEpoch {
    Superseded,
    SourceIdentityChanged(String),
}
impl StaleEpoch {
    pub(crate) fn is_superseded(&self) -> bool {
        matches!(self, Self::Superseded)
    }
}
impl std::fmt::Display for StaleEpoch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Superseded => f.write_str("workspace changed during the operation"),
            Self::SourceIdentityChanged(reason) => f.write_str(reason),
        }
    }
}
