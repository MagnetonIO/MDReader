use super::custody::{admission, decode, encode, EnrollmentState};
use super::*;
use std::sync::Mutex;
use zeroize::Zeroizing;
pub struct SyncKeyringVault {
    service: String,
}
impl std::fmt::Debug for SyncKeyringVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        redacted("SyncKeyringVault", f)
    }
}
static OS_CUSTODY: Mutex<()> = Mutex::new(());
impl SyncKeyringVault {
    pub fn new(app_identifier: &str) -> AuthorityResult<Self> {
        if app_identifier.is_empty()
            || app_identifier.len() > 200
            || !app_identifier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
        {
            return Err(AuthorityError::Configuration);
        }
        Ok(Self {
            service: format!("{app_identifier}.native-sync-credentials-v1"),
        })
    }
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows", target_os = "linux"))]
    fn entry(&self, reference: &SyncCredentialReference) -> AuthorityResult<::keyring::Entry> {
        ::keyring::Entry::new(&self.service, &reference.0).map_err(|_| AuthorityError::Custody)
    }
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows", target_os = "linux"))]
    fn read(&self, reference: &SyncCredentialReference) -> AuthorityResult<Option<CustodyRecord>> {
        match self.entry(reference)?.get_secret() {
            Ok(bytes) => decode(reference, Zeroizing::new(bytes)).map(Some),
            Err(::keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(AuthorityError::Custody),
        }
    }
}
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows", target_os = "linux"))]
impl SyncVault for SyncKeyringVault {
    fn get(&self, reference: &SyncCredentialReference) -> AuthorityResult<Option<CustodyRecord>> {
        let _lock = OS_CUSTODY.lock().map_err(|_| AuthorityError::Custody)?;
        self.read(reference)
    }
    fn create(&self, record: &CustodyRecord) -> AuthorityResult<()> {
        record.validate(&record.reference)?;
        let _lock = OS_CUSTODY.lock().map_err(|_| AuthorityError::Custody)?;
        if let Some(old) = self.read(&record.reference)? {
            return admission(&old, record, None);
        }
        if record.state != EnrollmentState::Pending || record.generation != 1 {
            return Err(AuthorityError::Generation);
        }
        self.entry(&record.reference)?
            .set_secret(&encode(record)?)
            .map_err(|_| AuthorityError::Custody)
    }
    fn compare_exchange(&self, expected: u64, record: &CustodyRecord) -> AuthorityResult<()> {
        record.validate(&record.reference)?;
        let _lock = OS_CUSTODY.lock().map_err(|_| AuthorityError::Custody)?;
        let old = self
            .read(&record.reference)?
            .ok_or(AuthorityError::Custody)?;
        admission(&old, record, Some(expected))?;
        self.entry(&record.reference)?
            .set_secret(&encode(record)?)
            .map_err(|_| AuthorityError::Custody)
    }
}
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "windows", target_os = "linux")))]
impl SyncVault for SyncKeyringVault {
    fn get(&self, _: &SyncCredentialReference) -> AuthorityResult<Option<CustodyRecord>> {
        Err(AuthorityError::Unsupported)
    }
    fn create(&self, _: &CustodyRecord) -> AuthorityResult<()> {
        Err(AuthorityError::Unsupported)
    }
    fn compare_exchange(&self, _: u64, _: &CustodyRecord) -> AuthorityResult<()> {
        Err(AuthorityError::Unsupported)
    }
}
