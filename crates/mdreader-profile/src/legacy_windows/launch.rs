//! Exact executable metadata is return provenance, never profile writer closure.
//! The native controller must separately own its frozen lifecycle capability.
use super::*;
use crate::{RelativeLocator, RootBinding};
use mdreader_capture::WindowsObjectStamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaunchRecord {
    version: u8,
    profile: RootBinding,
    executable_directory: RootBinding,
    executable: RelativeLocator,
    executable_stamp: WindowsObjectStamp,
    blake3: String,
}
/// Read/write-delete denial on the verified original executable is retained
/// through dispatch. It cannot stand in for original JSON/database owner fences.
pub struct RetainedLaunch {
    binary: Binary,
    record: LaunchRecord,
}
impl LegacyApplicationFence {
    pub fn launch_record(&mut self) -> Result<LaunchRecord> {
        self.verify_closed()?;
        let record = LaunchRecord {
            version: 1,
            profile: RootBinding::capture(&self.profile.path)?,
            executable_directory: RootBinding::capture(&self.binary.root.path)?,
            executable: RelativeLocator::capture(&self.binary.relative)?,
            executable_stamp: observe_object(&self.binary.file, false, false)
                .map_err(|_| Error::Conflict)?,
            blake3: self.binary.hash.to_hex().to_string(),
        };
        record.validate()?;
        Ok(record)
    }
}
impl LaunchRecord {
    pub fn profile(&self) -> &RootBinding {
        &self.profile
    }
    pub fn validate(&self) -> Result<()> {
        self.profile.validate()?;
        self.executable_directory.validate()?;
        let relative = self.executable.restore()?;
        if self.version != 1
            || relative.components().count() != 1
            || relative
                .to_str()
                .is_none_or(|name| !name.eq_ignore_ascii_case("MDReader.exe"))
            || self.blake3.len() != 64
            || !self
                .blake3
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.executable_stamp.2 == 0
            || self.executable_stamp.2 > 1024 * 1024 * 1024
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    /// Reopen only the exact installed executable selected before migration.
    /// This restores executable identity; it does not restore owner closure.
    pub fn retain(&self) -> Result<RetainedLaunch> {
        self.validate()?;
        let path = self
            .executable_directory
            .locator
            .restore()?
            .join(self.executable.restore()?);
        let mut launch = RetainedLaunch {
            binary: Binary::capture(&path)?,
            record: self.clone(),
        };
        launch.verify()?;
        Ok(launch)
    }
}
impl RetainedLaunch {
    pub fn verify(&mut self) -> Result<()> {
        self.record.validate()?;
        self.binary.verify()?;
        if RootBinding::capture(&self.binary.root.path)? != self.record.executable_directory
            || observe_object(&self.binary.file, false, false).map_err(|_| Error::Conflict)?
                != self.record.executable_stamp
            || self.binary.hash.to_hex().as_str() != self.record.blake3
            || RootBinding::capture(&self.record.profile.locator.restore()?)? != self.record.profile
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    /// Invoke only after the native Lifecycle journal committed RolledBack and
    /// the controller has released its original data sharing-denial handles.
    /// No profile arguments or old grants are reconstructed from archive rows.
    pub fn launch(&mut self) -> Result<std::process::Child> {
        self.verify()?;
        let profile = self.record.profile.locator.restore()?;
        if !process::matching(&self.binary.path(), &profile)?.is_empty() {
            return Err(Error::NotQuiescent);
        }
        let child = std::process::Command::new(self.binary.path())
            .spawn()
            .map_err(|_| Error::Storage)?;
        // Keep the executable guard after dispatch; replacing the path cannot
        // silently turn a retried return operation into a different program.
        self.binary.verify()?;
        Ok(child)
    }
}
