//! Restore ordinary reading preferences from a verified offline archive.
//! Unknown configuration stays in the raw archive; it never becomes a grant.
use super::{Error, Result};
use mdreader_core::SourceRoot;
use mdreader_legacy_export::{reviews::ArchiveIdentity, Conversion, Limits, Role};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::Path,
};

const MAX_BYTES: usize = 1024 * 1024;

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn reading_preferences(value: Value) -> Map<String, Value> {
    let Some(outer) = value.as_object() else {
        return Map::new();
    };
    let fields = match outer.get("settings") {
        Some(value) => match value.as_object() {
            Some(fields) => fields,
            None => return Map::new(),
        },
        None => outer,
    };
    let mut result = Map::new();
    for (key, allowed) in [
        ("theme", &["light", "dark", "system"][..]),
        ("readingFont", &["sans", "serif"][..]),
    ] {
        if let Some(text) = fields.get(key).and_then(Value::as_str) {
            if allowed.contains(&text) {
                result.insert(key.into(), Value::String(text.into()));
            }
        }
    }
    if let Some(index) = fields
        .get("textScaleIndex")
        .and_then(Value::as_u64)
        .filter(|n| *n < 7)
    {
        result.insert("textScaleIndex".into(), index.into());
    }
    for key in [
        "sidebarCollapsed",
        "outlineVisible",
        "relatedRailOpen",
        "inspectorOpen",
    ] {
        if let Some(value) = fields.get(key).and_then(Value::as_bool) {
            result.insert(key.into(), Value::Bool(value));
        }
    }
    if let Some(name) = fields.get("reviewer").and_then(Value::as_str) {
        let name = name.trim();
        if name.len() <= 256 && !name.chars().any(char::is_control) {
            result.insert("reviewer".into(), Value::String(name.into()));
        }
    }
    result
}

/// Called only while preparing a new fixed candidate, before its ready marker.
/// No original settings, existing destination, or active profile is replaced.
pub(super) fn restore(
    archive: &Path,
    identity: &ArchiveIdentity,
    destination: &Path,
    limits: &Limits,
) -> Result<usize> {
    let root = SourceRoot::capture(archive.to_owned()).map_err(|_| Error::Integrity)?;
    let manifest_file = root
        .open_optional_file_relative(Path::new("manifest.json"))
        .map_err(|_| Error::Integrity)?
        .ok_or(Error::Integrity)?;
    let mut manifest_bytes = Vec::new();
    (&manifest_file)
        .take(limits.max_manifest_bytes as u64 + 1)
        .read_to_end(&mut manifest_bytes)
        .map_err(|_| Error::Storage)?;
    if manifest_bytes.len() > limits.max_manifest_bytes
        || digest(&manifest_bytes) != identity.manifest_sha256
    {
        return Err(Error::Integrity);
    }
    let manifest = mdreader_legacy_export::verify(archive, limits).map_err(|_| Error::Integrity)?;
    if manifest.export_id != identity.export_id {
        return Err(Error::Conflict);
    }
    let settings = manifest
        .artifacts
        .iter()
        .filter(|a| a.role == Role::Settings)
        .collect::<Vec<_>>();
    if settings.len() > 1 {
        return Err(Error::Invalid);
    }
    let Some(artifact) = settings.first() else {
        return Ok(0);
    };
    if !matches!(artifact.conversion, Conversion::Known { .. }) {
        return Ok(0);
    }
    let member = manifest
        .members
        .iter()
        .find(|m| m.name == artifact.raw)
        .ok_or(Error::Integrity)?;
    // Larger/unsupported preferences remain preserved, without blocking other
    // useful record conversion or silently installing truncated configuration.
    if member.bytes > MAX_BYTES.min(limits.max_json_bytes) as u64 {
        return Ok(0);
    }
    let file = root
        .open_optional_file_relative(Path::new(&artifact.raw))
        .map_err(|_| Error::Integrity)?
        .ok_or(Error::Integrity)?;
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Storage)?;
    if bytes.len() as u64 != member.bytes || digest(&bytes) != member.sha256 {
        return Err(Error::Integrity);
    }
    let value = serde_json::from_slice(&bytes).map_err(|_| Error::Integrity)?;
    let preferences = reading_preferences(value);
    let count = preferences.len();
    if count == 0 {
        return Ok(0);
    }
    root.verify().map_err(|_| Error::Conflict)?;
    let output = serde_json::to_vec(&serde_json::json!({"settings": preferences}))
        .map_err(|_| Error::Invalid)?;
    let candidate =
        SourceRoot::capture(destination.to_owned()).map_err(|_| Error::InvalidDestination)?;
    #[cfg(unix)]
    let mut target = candidate
        .create_new_relative(Path::new("settings.json"))
        .map_err(|_| Error::Conflict)?;
    #[cfg(windows)]
    let mut target = mdreader_capture::PrivateDirectory::open(&candidate.path)
        .map_err(|_| Error::Conflict)?
        .create_database_file(Path::new("settings.json"))
        .map_err(|_| Error::Conflict)?;
    target.write_all(&output).map_err(|_| Error::Storage)?;
    target.sync_all().map_err(|_| Error::Storage)?;
    #[cfg(unix)]
    candidate
        .try_clone_directory_handle()
        .map_err(|_| Error::Storage)?
        .sync_all()
        .map_err(|_| Error::Storage)?;
    candidate.verify().map_err(|_| Error::Conflict)?;
    root.verify().map_err(|_| Error::Conflict)?;
    // The owning archive preparer revalidates the complete archive and source
    // identity again before publishing the completed candidate marker.
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn allowlists_wrapped_reading_preferences_without_reactivating_old_authority() {
        let result = reading_preferences(serde_json::json!({"settings": {
            "theme":"dark", "readingFont":"sans", "textScaleIndex":4,
            "sidebarCollapsed":true, "outlineVisible":false, "reviewer":"  Reader  ",
            "lensSchema":"retired", "token":"synthetic", "root":"/synthetic/private",
            "cloudAccounts":["synthetic"]
        }, "extra":"retained only in raw archive"}));
        assert_eq!(result.len(), 6);
        assert_eq!(result["reviewer"], "Reader");
        assert!(!result.contains_key("token"));
        assert!(!result.contains_key("lensSchema"));
        assert!(!result.contains_key("root"));
    }
    #[test]
    fn malformed_values_do_not_install_defaults_or_fall_back_outside_wrapper() {
        assert!(
            reading_preferences(serde_json::json!({"settings":null,"theme":"dark"})).is_empty()
        );
        let result = reading_preferences(serde_json::json!({
            "theme":"unsupported", "readingFont":false, "textScaleIndex":7,
            "outlineVisible":"false", "reviewer":"bad\nname"
        }));
        assert!(result.is_empty());
        assert_eq!(
            reading_preferences(serde_json::json!({"theme":"light"}))["theme"],
            "light"
        );
    }
}
