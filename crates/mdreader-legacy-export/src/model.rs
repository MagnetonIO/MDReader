use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Proposals,
    Catalog,
    CloudCatalog,
    Library,
    Settings,
    Session,
    Enterprise,
}
impl Role {
    pub(crate) fn sqlite(self) -> bool {
        matches!(self, Self::Proposals | Self::Catalog | Self::CloudCatalog)
    }
}
#[derive(Debug, Clone)]
pub struct Input {
    pub role: Role,
    pub path: PathBuf,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotSpec {
    pub version: u8,
    pub offline_snapshots: bool,
    pub inputs: Vec<SpecifiedInput>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecifiedInput {
    pub role: Role,
    pub locator: Locator,
}
impl SnapshotSpec {
    pub fn from_inputs(inputs: &[Input]) -> Result<Self> {
        Ok(Self {
            version: 1,
            offline_snapshots: true,
            inputs: inputs
                .iter()
                .map(|i| {
                    Ok(SpecifiedInput {
                        role: i.role,
                        locator: Locator::capture(&i.path)?,
                    })
                })
                .collect::<Result<_>>()?,
        })
    }
    pub fn selected_inputs(self, limits: &Limits) -> Result<Vec<Input>> {
        limits.validate()?;
        if self.version != 1
            || !self.offline_snapshots
            || self.inputs.is_empty()
            || self.inputs.len() > limits.max_artifacts
        {
            return Err(Error::Invalid);
        }
        self.inputs
            .into_iter()
            .map(|i| {
                Ok(Input {
                    role: i.role,
                    path: i.locator.restore()?,
                })
            })
            .collect()
    }
}
impl Input {
    pub(crate) fn validate(&self) -> Result<()> {
        validate_path(&self.path)
    }
}
pub(crate) fn validate_path(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.components().any(|c| {
            matches!(c, Component::ParentDir | Component::CurDir) || c.as_os_str() == "reader-v1"
        })
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
/// Native path units preserve Unix non-UTF-8 and Windows wide paths in archives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Locator {
    pub encoding: Encoding,
    pub units: Vec<u32>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    UnixBytes,
    WindowsWide,
}
impl Locator {
    pub fn capture(path: &Path) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let result = Self {
                encoding: Encoding::UnixBytes,
                units: path
                    .as_os_str()
                    .as_bytes()
                    .iter()
                    .map(|&x| u32::from(x))
                    .collect(),
            };
            result.validate()?;
            Ok(result)
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            let result = Self {
                encoding: Encoding::WindowsWide,
                units: path.as_os_str().encode_wide().map(u32::from).collect(),
            };
            result.validate()?;
            Ok(result)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            Err(Error::Unsupported)
        }
    }
    fn validate(&self) -> Result<()> {
        let maximum = match self.encoding {
            Encoding::UnixBytes => 255,
            Encoding::WindowsWide => 65535,
        };
        if self.units.is_empty()
            || self.units.len() > 16384
            || self.units.iter().any(|&x| x == 0 || x > maximum)
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    pub fn restore(&self) -> Result<PathBuf> {
        self.validate()?;
        #[cfg(unix)]
        {
            use std::{ffi::OsString, os::unix::ffi::OsStringExt};
            if self.encoding != Encoding::UnixBytes {
                return Err(Error::Unsupported);
            };
            let p = PathBuf::from(OsString::from_vec(
                self.units.iter().map(|&x| x as u8).collect(),
            ));
            validate_path(&p)?;
            Ok(p)
        }
        #[cfg(windows)]
        {
            use std::{ffi::OsString, os::windows::ffi::OsStringExt};
            if self.encoding != Encoding::WindowsWide {
                return Err(Error::Unsupported);
            };
            let p = PathBuf::from(OsString::from_wide(
                &self.units.iter().map(|&x| x as u16).collect::<Vec<_>>(),
            ));
            validate_path(&p)?;
            Ok(p)
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(Error::Unsupported)
        }
    }
}
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_artifacts: usize,
    pub max_artifact_bytes: u64,
    pub max_total_bytes: u64,
    pub max_json_bytes: usize,
    pub max_rows: u64,
    pub max_row_bytes: usize,
    pub max_records_bytes: u64,
    pub max_manifest_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_artifacts: 128,
            max_artifact_bytes: 256 * 1024 * 1024,
            max_total_bytes: 1024 * 1024 * 1024,
            max_json_bytes: 8 * 1024 * 1024,
            max_rows: 1_000_000,
            max_row_bytes: 4 * 1024 * 1024,
            max_records_bytes: 256 * 1024 * 1024,
            max_manifest_bytes: 512 * 1024,
        }
    }
}
impl Limits {
    pub(crate) fn validate(&self) -> Result<()> {
        let cap = Self::default();
        if self.max_artifacts == 0
            || self.max_artifacts > cap.max_artifacts
            || self.max_artifact_bytes == 0
            || self.max_artifact_bytes > cap.max_artifact_bytes
            || self.max_total_bytes == 0
            || self.max_total_bytes > cap.max_total_bytes
            || self.max_json_bytes == 0
            || self.max_json_bytes > cap.max_json_bytes
            || self.max_rows == 0
            || self.max_rows > cap.max_rows
            || self.max_row_bytes == 0
            || self.max_row_bytes > cap.max_row_bytes
            || self.max_records_bytes == 0
            || self.max_records_bytes > cap.max_records_bytes
            || self.max_manifest_bytes == 0
            || self.max_manifest_bytes > cap.max_manifest_bytes
        {
            Err(Error::Bounds)
        } else {
            Ok(())
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: String,
    pub version: u8,
    pub export_id: uuid::Uuid,
    pub complete: bool,
    pub artifacts: Vec<Artifact>,
    pub members: Vec<Member>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub role: Role,
    pub locator: Locator,
    pub raw: String,
    pub records: Option<String>,
    pub conversion: Conversion,
    pub sqlite: Option<SqliteInfo>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Conversion {
    Known { rows: u64 },
    Unavailable { reason: Unavailable },
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unavailable {
    UnknownSchema,
    UnknownVersion,
    MalformedKnownRecord,
    /// The row is retained raw; unsupported values or bounds prevent typed export.
    UnsupportedRecord,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SqliteInfo {
    pub user_version: i64,
    pub schema_sha256: String,
    pub integrity: SqliteIntegrity,
    pub tables: Vec<TableInfo>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqliteIntegrity {
    Checked,
    UnsupportedVirtualModule,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableInfo {
    pub name: String,
    pub rows: Option<u64>,
}
impl Manifest {
    pub(crate) fn new() -> Self {
        Self {
            format: "mdreader-legacy-export".into(),
            version: 1,
            export_id: uuid::Uuid::new_v4(),
            complete: true,
            artifacts: vec![],
            members: vec![],
        }
    }
    pub(crate) fn validate(&self, limits: &Limits) -> Result<()> {
        if self.format != "mdreader-legacy-export"
            || self.version != 1
            || self.export_id.get_version_num() != 4
            || !self.complete
            || self.artifacts.is_empty()
            || self.artifacts.len() > limits.max_artifacts
            || self.members.len() > self.artifacts.len() * 2
        {
            return Err(Error::Integrity);
        }
        let mut expected = std::collections::BTreeSet::new();
        for (index, a) in self.artifacts.iter().enumerate() {
            a.locator.validate()?;
            if a.raw != format!("{index:04}.raw") {
                return Err(Error::Integrity);
            }
            expected.insert(a.raw.clone());
            match (&a.records, &a.conversion) {
                (Some(name), Conversion::Known { rows })
                    if name == &format!("{index:04}.records.jsonl") && *rows <= limits.max_rows =>
                {
                    expected.insert(name.clone());
                }
                (None, Conversion::Unavailable { .. }) => {}
                _ => return Err(Error::Integrity),
            }
            if a.role.sqlite() != a.sqlite.is_some() {
                return Err(Error::Integrity);
            }
            if let Some(info) = &a.sqlite {
                if info.schema_sha256.len() != 64
                    || !info.schema_sha256.bytes().all(|b| b.is_ascii_hexdigit())
                    || info.tables.len() > 1024
                    || info
                        .tables
                        .iter()
                        .any(|t| t.rows.is_some_and(|r| r > limits.max_rows))
                {
                    return Err(Error::Integrity);
                }
            }
        }
        let actual = self
            .members
            .iter()
            .map(|m| m.name.clone())
            .collect::<std::collections::BTreeSet<_>>();
        if actual != expected
            || actual.len() != self.members.len()
            || self
                .members
                .iter()
                .any(|m| m.sha256.len() != 64 || !m.sha256.bytes().all(|x| x.is_ascii_hexdigit()))
        {
            return Err(Error::Integrity);
        }
        Ok(())
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum KnownRecord {
    Catalog {
        record: Box<crate::catalog::ArchivedRecord>,
    },
    CloudMarkdown {
        record: Box<crate::cloud::ArchivedMarkdown>,
    },
    CloudUnavailable {
        table: String,
        row_id: Option<String>,
        reason: Unavailable,
    },
    CloudArchiveScope {
        supported_tables: Vec<String>,
        raw_only_tables: Vec<String>,
        source_binding: String,
        write_replay: String,
    },
    Proposal {
        row_id: i64,
        fields: std::collections::BTreeMap<String, SqlValue>,
    },
    Library {
        version: u64,
        value: serde_json::Value,
    },
    Session {
        version: u64,
        value: serde_json::Value,
    },
    Settings {
        value: serde_json::Value,
    },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub(crate) enum SqlValue {
    Null,
    Integer(String),
    RealBits(String),
    TextBytes(String),
    BlobBytes(String),
}
