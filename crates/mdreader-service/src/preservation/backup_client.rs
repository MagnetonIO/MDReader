//! Exact issued target identity survives transport uncertainty; no retry loop.
use super::*;
use mdreader_profile::database_backup::{BackupReceipt, BackupRequest};

impl Attempt {
    pub fn backup(&mut self, request: &BackupRequest) -> Result<BackupStatus> {
        request.validate().map_err(|_| Error::Invalid)?;
        let lease = self.lease.ok_or(Error::NotHeld)?;
        if self.backup.is_some() {
            return Err(Error::Uncertain);
        }
        if self.revalidate()?.phase != Phase::Held {
            return Err(Error::NotHeld);
        }
        self.backup = Some(request.clone());
        self.receive_backup(ControlRequest::Backup {
            scope: self.scope.clone(),
            lease,
            request: Box::new(request.clone()),
        })
    }

    pub fn reconcile_backup(&self) -> Result<BackupStatus> {
        self.receive_backup(ControlRequest::ReconcileBackup {
            scope: self.scope.clone(),
            lease: self.lease.ok_or(Error::NotHeld)?,
            request: Box::new(self.backup.as_ref().ok_or(Error::UnknownOperation)?.clone()),
        })
    }

    fn receive_backup(&self, request: ControlRequest) -> Result<BackupStatus> {
        let ControlReply::Backup { status } = self.endpoint.call(request)? else {
            return Err(Error::Uncertain);
        };
        if let BackupStatus::Completed { receipt } = &status {
            self.verify_backup(receipt)?;
        }
        Ok(status)
    }

    fn verify_backup(&self, receipt: &BackupReceipt) -> Result<()> {
        receipt.validate().map_err(|_| Error::Uncertain)?;
        if self.backup.as_ref() != Some(&receipt.request) {
            return Err(Error::StaleOwner);
        }
        Ok(())
    }
}
