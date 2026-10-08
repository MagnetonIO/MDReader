use super::*;

impl AssetStore {
    pub(super) fn read_file(&self, item: &Item) -> Result<Vec<u8>> {
        let file = self
            .root
            .open_optional_file_relative(&item.filename())
            .map_err(|_| unavailable())?
            .ok_or_else(unavailable)?;
        if file.metadata().map_err(|_| unavailable())?.len() != u64::from(item.bytes) {
            return Err(unavailable());
        }
        let mut bytes = Vec::with_capacity(item.bytes as usize + 1);
        file.take(u64::from(item.bytes) + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| unavailable())?;
        if bytes.len() != item.bytes as usize {
            return Err(unavailable());
        }
        self.verify()?;
        Ok(bytes)
    }
    pub fn prepare_open(
        &self,
        store: &mut ReaderStore,
        doc: &str,
        revision: &str,
        id: &str,
        manifest: &str,
        permit: ReadPermit,
    ) -> Result<PreparedSnapshot> {
        let current = self.manifest(store, doc, revision)?;
        let item = self.item(store, id)?;
        if current.manifest != manifest
            || item.doc != doc
            || item.revision != revision
            || item.state != "ready"
        {
            return Err(unavailable());
        }
        Ok(PreparedSnapshot {
            permit,
            bytes: self.read_file(&item)?,
            receipt: item.receipt()?,
            manifest: manifest.into(),
            root: self.root.clone(),
        })
    }
}
