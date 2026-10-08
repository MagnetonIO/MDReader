//! Native path units for manifest-relative names. The offline exporter's Locator
//! deliberately restores only absolute input paths and must not be relaxed.
use crate::{model::relative, Error, Result};
use mdreader_legacy_export::{Encoding, Locator};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelativeLocator {
    pub encoding: Encoding,
    pub units: Vec<u32>,
}
impl RelativeLocator {
    pub fn capture(path: &Path) -> Result<Self> {
        relative(path)?;
        let captured = Locator::capture(path).map_err(|_| Error::Invalid)?;
        Ok(Self {
            encoding: captured.encoding,
            units: captured.units,
        })
    }
    pub fn restore(&self) -> Result<PathBuf> {
        let path = self.native_path()?;
        relative(&path)?;
        Ok(path)
    }
    fn native_path(&self) -> Result<PathBuf> {
        let maximum = match self.encoding {
            Encoding::UnixBytes => 255,
            Encoding::WindowsWide => 65535,
        };
        if self.units.is_empty()
            || self.units.len() > 16384
            || self.units.iter().any(|&unit| unit == 0 || unit > maximum)
        {
            return Err(Error::Invalid);
        }
        #[cfg(unix)]
        let path = {
            use std::{ffi::OsString, os::unix::ffi::OsStringExt};
            if self.encoding != Encoding::UnixBytes {
                return Err(Error::Unsupported);
            }
            PathBuf::from(OsString::from_vec(
                self.units.iter().map(|&unit| unit as u8).collect(),
            ))
        };
        #[cfg(windows)]
        let path = {
            use std::{ffi::OsString, os::windows::ffi::OsStringExt};
            if self.encoding != Encoding::WindowsWide {
                return Err(Error::Unsupported);
            }
            PathBuf::from(OsString::from_wide(
                &self
                    .units
                    .iter()
                    .map(|&unit| unit as u16)
                    .collect::<Vec<_>>(),
            ))
        };
        #[cfg(not(any(unix, windows)))]
        return Err(Error::Unsupported);
        #[cfg(any(unix, windows))]
        {
            Ok(path)
        }
    }
}

/// An absolute owner root. Unlike legacy export input paths, this can name an
/// existing protected reader-v1 root; it never makes that root an export input.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RootLocator(RelativeLocator);
impl RootLocator {
    pub fn capture(path: &Path) -> Result<Self> {
        let captured = Locator::capture(path).map_err(|_| Error::Invalid)?;
        let result = Self(RelativeLocator {
            encoding: captured.encoding,
            units: captured.units,
        });
        result.restore()?;
        Ok(result)
    }
    pub fn restore(&self) -> Result<PathBuf> {
        let path = self.0.native_path()?;
        if !path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::CurDir | std::path::Component::ParentDir
                )
            })
        {
            return Err(Error::Invalid);
        }
        Ok(path)
    }
}
