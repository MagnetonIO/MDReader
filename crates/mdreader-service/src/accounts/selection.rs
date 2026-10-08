//! Exact public selection identity, never a credential or source grant.
use super::*;
use mdreader_core::device::catalog::Receipt;
use mdreader_sync::SyncScope;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedScope {
    pub account_digest: String,
    pub library_id: Uuid,
    pub device_id: String,
}
impl SelectedScope {
    pub(super) fn current(scope: &SyncScope) -> Result<Self> {
        Ok(Self {
            account_digest: scope.account_digest().into(),
            library_id: Uuid::parse_str(scope.library_id())
                .map_err(|_| AccountError::IdentityOrScope)?,
            device_id: scope.device_id().into(),
        })
    }
    fn validate(&self) -> Result<()> {
        if !hash(&self.account_digest)
            || self.library_id.get_version_num() != 4
            || SyncScope::new(
                self.account_digest.clone(),
                self.library_id.to_string(),
                self.device_id.clone(),
            )
            .is_err()
        {
            return Err(AccountError::Invalid);
        }
        Ok(())
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedOperation {
    pub scope: SelectedScope,
    pub receipt: Receipt,
    pub operation_id: Uuid,
}
impl SelectedOperation {
    pub(super) fn validate(&self) -> Result<()> {
        self.scope.validate()?;
        if self.operation_id.get_version_num() != 4 {
            return Err(AccountError::Invalid);
        }
        crate::sync::outgoing::validate_selected(&self.receipt)
            .map_err(|_| AccountError::Invalid)?;
        Ok(())
    }
    pub(super) fn require_scope(&self, scope: &SyncScope) -> Result<()> {
        if self.scope != SelectedScope::current(scope)? {
            return Err(AccountError::IdentityOrScope);
        }
        Ok(())
    }
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
