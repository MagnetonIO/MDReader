//! Retained macOS application identities captured BEFORE the original quits.
//! This is one part of legacy-owner closure, not a substitute for DB/JSON fences.
use super::legacy_launch::LegacyLaunch;
use super::{Error, Result};
use mdreader_core::SourceRoot;
use std::path::Path;

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use objc2::rc::Retained;
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::NSString;

    const ORIGINAL_BUNDLE: &str = "dev.mdreader.app";
    const MAX_APPLICATIONS: usize = 16;

    pub(super) struct Applications {
        profile: SourceRoot,
        instances: Vec<Retained<NSRunningApplication>>,
        launch: LegacyLaunch,
    }
    impl Applications {
        pub(super) fn capture(path: &Path) -> Result<Option<Self>> {
            let profile = SourceRoot::capture(path.to_owned()).map_err(|_| Error::Conflict)?;
            // Only the original standard native profile is attributable to this
            // bundle identifier. Custom profiles require their own owner proof.
            let home = std::env::var_os("HOME").ok_or(Error::Unsupported)?;
            let standard = std::path::PathBuf::from(home)
                .join("Library/Application Support")
                .join(ORIGINAL_BUNDLE)
                .canonicalize()
                .map_err(|_| Error::Unsupported)?;
            if profile.path.canonicalize().map_err(|_| Error::Conflict)? != standard {
                return Ok(None);
            }
            let current = NSRunningApplication::currentApplication();
            let running = NSRunningApplication::runningApplicationsWithBundleIdentifier(
                &NSString::from_str(ORIGINAL_BUNDLE),
            );
            if running.len() > MAX_APPLICATIONS {
                return Err(Error::Bounds);
            }
            let instances = running
                .iter()
                .filter(|app| **app != *current && !app.isTerminated())
                .collect::<Vec<_>>();
            // Absence now does not prove that an original writer was stopped.
            // The native caller must keep this object through close and retry.
            if instances.is_empty() {
                return Err(Error::NotQuiescent);
            }
            let mut launches = instances
                .iter()
                .map(|app| {
                    let bundle = app.bundleURL().ok_or(Error::Unsupported)?;
                    let executable = app.executableURL().ok_or(Error::Unsupported)?;
                    if !bundle.isFileURL() || !executable.isFileURL() {
                        return Err(Error::Unsupported);
                    }
                    LegacyLaunch::capture(
                        bundle.path().ok_or(Error::Unsupported)?.to_string().into(),
                        executable
                            .path()
                            .ok_or(Error::Unsupported)?
                            .to_string()
                            .into(),
                    )
                })
                .collect::<Result<Vec<_>>>()?;
            let launch = launches.pop().ok_or(Error::NotQuiescent)?;
            if launches.iter().any(|other| !launch.same_application(other)) {
                return Err(Error::Unsupported);
            }
            profile.verify().map_err(|_| Error::Conflict)?;
            Ok(Some(Self {
                profile,
                instances,
                launch,
            }))
        }
        pub(super) fn launch_identity(&self) -> Result<LegacyLaunch> {
            self.verify_closed()?;
            self.launch.verify()?;
            Ok(self.launch.clone())
        }
        pub(super) fn verify_closed(&self) -> Result<()> {
            self.profile.verify().map_err(|_| Error::Conflict)?;
            if self.instances.iter().any(|app| !app.isTerminated()) {
                return Err(Error::NotQuiescent);
            }
            // NSRunningApplication remains valid after exit and represents the
            // instance, unlike a reusable PID. Refuse newly launched writers too.
            let current = NSRunningApplication::currentApplication();
            let running = NSRunningApplication::runningApplicationsWithBundleIdentifier(
                &NSString::from_str(ORIGINAL_BUNDLE),
            );
            if running.len() > MAX_APPLICATIONS {
                return Err(Error::Bounds);
            }
            if running
                .iter()
                .any(|app| *app != *current && !app.isTerminated())
            {
                return Err(Error::NotQuiescent);
            }
            self.profile.verify().map_err(|_| Error::Conflict)
        }
    }
}

/// Captured only by native folder selection, with the current app's profile
/// already excluded. No renderer PID, success reply or caller boolean is used.
pub(super) struct LegacyApplications {
    #[cfg(target_os = "macos")]
    inner: macos::Applications,
}
impl LegacyApplications {
    /// Keep the main AppKit run loop running while waiting for a graceful window
    /// close; termination properties update as that run loop advances. This
    /// method never requests termination or discards unsaved buffers.
    pub(super) fn capture(profile: &Path) -> Result<Option<Self>> {
        #[cfg(target_os = "macos")]
        {
            Ok(macos::Applications::capture(profile)?.map(|inner| Self { inner }))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = profile;
            Err(Error::Unsupported)
        }
    }
    pub(super) fn verify_closed(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            self.inner.verify_closed()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(Error::Unsupported)
        }
    }
    pub(super) fn launch_identity(&self) -> Result<LegacyLaunch> {
        #[cfg(target_os = "macos")]
        {
            self.inner.launch_identity()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(Error::Unsupported)
        }
    }
}
