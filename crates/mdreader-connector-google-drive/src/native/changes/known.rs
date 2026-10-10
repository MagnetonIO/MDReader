//! Bounded derived ancestry knowledge. Every actual read rechecks native scope.
use super::super::*;

impl Client {
    /// Metadata is supplied only by the owner's exact durable observation.
    pub fn with_known_metadata(self, known: Vec<Metadata>) -> Result<Self> {
        self.remember_metadata(&known, true)?;
        Ok(self)
    }
    pub(in super::super) fn remember_metadata(
        &self,
        rows: &[Metadata],
        replaces: bool,
    ) -> Result<()> {
        let mut known = self.known.lock().map_err(|_| ProviderError::Persistence)?;
        if replaces {
            known.clear();
        }
        let mut unique_rows = std::collections::BTreeSet::new();
        for row in rows {
            folder::valid_id(&row.key)?;
            if !unique_rows.insert(&row.key) {
                return Err(ProviderError::IdentityMismatch);
            }
            if let Some(path) = &row.membership_ancestors {
                if path.is_empty() || path.len() > 64 || path.contains(&row.key) {
                    return Err(ProviderError::Protocol);
                }
                let mut unique = std::collections::BTreeSet::new();
                for key in path {
                    folder::valid_id(key)?;
                    if !unique.insert(key) {
                        return Err(ProviderError::Protocol);
                    }
                }
            }
            known.insert(row.key.clone(), row.membership_ancestors.clone());
        }
        if known.len() > 16_384 {
            return Err(ProviderError::TooLarge);
        }
        Ok(())
    }
    pub(super) fn known_structure(&self, root: &RecordKey) -> Result<(bool, bool)> {
        let known = self.known.lock().map_err(|_| ProviderError::Persistence)?;
        Ok((
            known
                .values()
                .any(|path| path.as_ref().is_some_and(|p| p.contains(root))),
            known.values().any(Option::is_none),
        ))
    }
    pub(super) fn affected_keys(
        &self,
        root: &RecordKey,
        after: Option<&RecordKey>,
        limit: usize,
    ) -> Result<(Vec<RecordKey>, bool)> {
        let known = self.known.lock().map_err(|_| ProviderError::Persistence)?;
        let mut keys: Vec<_> = known
            .iter()
            .filter(|(key, path)| {
                after.is_none_or(|after| *key > after)
                    && path.as_ref().is_some_and(|p| p.contains(root))
            })
            .take(limit + 1)
            .map(|(key, _)| key.clone())
            .collect();
        let more = keys.len() > limit;
        keys.truncate(limit);
        Ok((keys, more))
    }
}
