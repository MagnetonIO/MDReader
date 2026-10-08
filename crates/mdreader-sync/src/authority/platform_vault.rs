//! Native encrypted-byte store adapter. Custody encoding and generation/material
//! admission remain private to this crate; platform code cannot invent receipts.
use super::{
    custody::{EnrollmentState, admission, decode, encode},
    *,
};
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

pub trait SecureByteStore: Send + Sync {
    fn read(&self, reference: &str) -> AuthorityResult<Option<Zeroizing<Vec<u8>>>>;
    fn write(&self, reference: &str, bytes: &[u8]) -> AuthorityResult<()>;
}
pub struct SyncPlatformVault {
    store: Arc<dyn SecureByteStore>,
}
static CUSTODY: Mutex<()> = Mutex::new(());
impl SyncPlatformVault {
    pub fn new(store: Arc<dyn SecureByteStore>) -> Self {
        Self { store }
    }
    fn read(&self, reference: &SyncCredentialReference) -> AuthorityResult<Option<CustodyRecord>> {
        self.store
            .read(reference.opaque_reference())?
            .map(|bytes| decode(reference, bytes))
            .transpose()
    }
}
impl SyncVault for SyncPlatformVault {
    fn get(&self, reference: &SyncCredentialReference) -> AuthorityResult<Option<CustodyRecord>> {
        let _guard = CUSTODY.lock().map_err(|_| AuthorityError::Custody)?;
        self.read(reference)
    }
    fn create(&self, record: &CustodyRecord) -> AuthorityResult<()> {
        record.validate(&record.reference)?;
        let _guard = CUSTODY.lock().map_err(|_| AuthorityError::Custody)?;
        if let Some(old) = self.read(&record.reference)? {
            return admission(&old, record, None);
        }
        if record.state != EnrollmentState::Pending || record.generation != 1 {
            return Err(AuthorityError::Generation);
        }
        self.store
            .write(record.reference.opaque_reference(), &encode(record)?)
    }
    fn compare_exchange(&self, expected: u64, record: &CustodyRecord) -> AuthorityResult<()> {
        record.validate(&record.reference)?;
        let _guard = CUSTODY.lock().map_err(|_| AuthorityError::Custody)?;
        let old = self
            .read(&record.reference)?
            .ok_or(AuthorityError::Custody)?;
        admission(&old, record, Some(expected))?;
        self.store
            .write(record.reference.opaque_reference(), &encode(record)?)
    }
}
