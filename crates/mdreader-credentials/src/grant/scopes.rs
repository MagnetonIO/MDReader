//! Native, verified OAuth scope contract. Existing v3 Dropbox grants remain
//! read-only; editing requires fresh explicit consent and returned write scope.
use super::*;
use std::collections::BTreeSet;

pub const DRIVE_FILE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
pub const DROPBOX_READ_SCOPES: &str = "account_info.read files.metadata.read files.content.read";
// https://docs.dropboxapi.com/dropbox-api/api-reference/user-endpoints/files/upload
pub const DROPBOX_EDIT_SCOPES: &str =
    "account_info.read files.metadata.read files.content.read files.content.write";

fn exact(actual: &str, expected: &str) -> bool {
    if actual.len() > 512 {
        return false;
    }
    let words: Vec<_> = actual.split_whitespace().collect();
    let unique: BTreeSet<_> = words.iter().copied().collect();
    words.len() == unique.len() && unique == expected.split_whitespace().collect()
}
impl ProviderGrant {
    /// Caller is the native OAuth token verifier, never a renderer claim. Only
    /// the two reviewed provider permission contracts can enter custody.
    pub fn with_verified_scopes(mut self, scopes: &str) -> Result<Self> {
        let canonical = match self.registration.provider() {
            ProviderKind::GoogleDrive if exact(scopes, DRIVE_FILE_SCOPE) => DRIVE_FILE_SCOPE,
            ProviderKind::Dropbox if exact(scopes, DROPBOX_READ_SCOPES) => DROPBOX_READ_SCOPES,
            ProviderKind::Dropbox if exact(scopes, DROPBOX_EDIT_SCOPES) => DROPBOX_EDIT_SCOPES,
            _ => return Err(ProviderError::Unauthorized),
        };
        self.verified_scopes = Some(canonical.into());
        Ok(self)
    }
    pub fn verified_scopes(&self) -> Option<&str> {
        self.verified_scopes.as_deref()
    }
    pub fn permits_guarded_write(&self) -> bool {
        match self.registration.provider() {
            ProviderKind::GoogleDrive => true, // v3 also verified drive.file exactly.
            ProviderKind::Dropbox => self.verified_scopes() == Some(DROPBOX_EDIT_SCOPES),
        }
    }
    pub fn matches_verified_scopes(&self, scopes: &str) -> bool {
        let expected = self
            .verified_scopes()
            .unwrap_or(match self.registration.provider() {
                ProviderKind::GoogleDrive => DRIVE_FILE_SCOPE,
                ProviderKind::Dropbox => DROPBOX_READ_SCOPES,
            });
        exact(scopes, expected)
    }
}
