//! Additive reader namespace: strict new patches, raw-preserving existing values.
use super::{array, keys, object, value};
use crate::profile_transition::persistence::types::{PersistenceError as Error, Result};
use serde_json::Value;

fn fields<'a>(v: &'a Value, allowed: &[&str]) -> Result<&'a serde_json::Map<String, Value>> {
    keys(v, allowed)?;
    let fields = v.as_object().ok_or(Error::Invalid)?;
    if fields.len() != allowed.len() {
        return Err(Error::Invalid);
    }
    Ok(fields)
}
fn hash(v: Option<&Value>) -> bool {
    v.and_then(Value::as_str).is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn uuid(v: Option<&Value>) -> bool {
    v.and_then(Value::as_str)
        .and_then(|s| uuid::Uuid::parse_str(s).ok().map(|id| (s, id)))
        .is_some_and(|(s, id)| id.get_version_num() == 4 && id.to_string() == s)
}
fn uint(v: Option<&Value>, min: u64, max: u64) -> bool {
    v.and_then(Value::as_u64)
        .is_some_and(|v| v >= min && v <= max)
}
fn list(v: Option<&Value>, max: usize) -> Result<&Vec<Value>> {
    let list = v.and_then(Value::as_array).ok_or(Error::Invalid)?;
    if list.len() > max {
        return Err(Error::TooLarge);
    }
    Ok(list)
}
fn reference(v: &Value) -> Result<()> {
    let kind = v
        .get("kind")
        .and_then(Value::as_str)
        .ok_or(Error::Invalid)?;
    if !hash(v.get("revision")) {
        return Err(Error::Invalid);
    }
    if kind == "device" {
        fields(v, &["kind", "documentId", "sourceId", "revision"])?;
        return if uuid(v.get("documentId")) && hash(v.get("sourceId")) {
            Ok(())
        } else {
            Err(Error::Invalid)
        };
    }
    fields(v, &["kind", "sourceId", "path", "revision"])?;
    let path = v
        .get("path")
        .and_then(Value::as_str)
        .ok_or(Error::Invalid)?;
    if path.is_empty()
        || path.len() > 4096
        || path.starts_with('/')
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(Error::Invalid);
    }
    let valid = match kind {
        "local" => hash(v.get("sourceId")),
        "provider" => {
            let source = v
                .get("sourceId")
                .and_then(Value::as_str)
                .and_then(|s| s.strip_prefix("provider:"));
            let generated = path
                .strip_prefix("records/")
                .and_then(|s| s.strip_suffix(".md"));
            [source, generated]
                .iter()
                .all(|s| s.is_some_and(|s| hash(Some(&Value::String(s.into())))))
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(Error::Invalid)
    }
}
fn entry(v: &Value) -> Result<()> {
    fields(v, &["reference", "title", "heading", "position"])?;
    let reference_value = v.get("reference").ok_or(Error::Invalid)?;
    reference(reference_value)?;
    if !v
        .get("title")
        .and_then(Value::as_str)
        .is_some_and(|s| s.len() <= 512)
    {
        return Err(Error::Invalid);
    }
    if !v
        .get("heading")
        .is_some_and(|v| v.is_null() || v.as_str().is_some_and(|s| s.len() <= 512))
    {
        return Err(Error::Invalid);
    }
    let position = v.get("position").ok_or(Error::Invalid)?;
    if position.is_null() {
        return Ok(());
    }
    fields(position, &["revision", "line", "block", "offset"])?;
    if position.get("revision") != reference_value.get("revision")
        || !uint(position.get("line"), 1, 0x7fff_ffff)
        || !uint(position.get("block"), 0, 0x7fff_ffff)
        || !position
            .get("offset")
            .and_then(Value::as_f64)
            .is_some_and(|v| v.is_finite() && (0.0..=1_000_000.0).contains(&v))
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
pub(super) fn validate(v: &Value) -> Result<()> {
    fields(v, &["version", "activeTabId", "tabs", "recent", "mru"])?;
    if v.get("version") != Some(&Value::from(1)) {
        return Err(Error::UnsupportedSchema);
    }
    let mut ids = std::collections::BTreeSet::new();
    for tab in list(v.get("tabs"), 32)? {
        fields(tab, &["id", "entries", "index", "pinned"])?;
        if !uuid(tab.get("id"))
            || !ids.insert(tab["id"].as_str().ok_or(Error::Invalid)?)
            || !tab.get("pinned").is_some_and(Value::is_boolean)
        {
            return Err(Error::Invalid);
        }
        let entries = list(tab.get("entries"), 32)?;
        if entries.is_empty() || !uint(tab.get("index"), 0, entries.len() as u64 - 1) {
            return Err(Error::Invalid);
        }
        for item in entries {
            entry(item)?;
        }
    }
    let active = v.get("activeTabId").ok_or(Error::Invalid)?;
    if !active.is_null() && !active.as_str().is_some_and(|id| ids.contains(id)) {
        return Err(Error::Invalid);
    }
    let mut mru = std::collections::BTreeSet::new();
    for id in list(v.get("mru"), 32)? {
        let id = id.as_str().ok_or(Error::Invalid)?;
        if !ids.contains(id) || !mru.insert(id) {
            return Err(Error::Invalid);
        }
    }
    if mru != ids {
        return Err(Error::Invalid);
    }
    for item in list(v.get("recent"), 100)? {
        entry(item)?;
    }
    Ok(())
}
/// Project only known fields to validate without parsing unknown numeric lexemes.
fn project(raw: &str, shape: &str) -> Result<Value> {
    let allowed: &[&str] = match shape {
        "namespace" => &["version", "activeTabId", "tabs", "recent", "mru"],
        "tab" => &["id", "entries", "index", "pinned"],
        "entry" => &["reference", "title", "heading", "position"],
        "reference" => &["kind", "documentId", "sourceId", "path", "revision"],
        "position" => &["revision", "line", "block", "offset"],
        _ => return Err(Error::Invalid),
    };
    let mut out = serde_json::Map::new();
    for (key, raw) in object(raw)? {
        if !allowed.contains(&key.as_str()) {
            continue;
        }
        let parsed = match (shape, key.as_str()) {
            ("namespace", "tabs") => Value::Array(
                array(raw.get())?
                    .iter()
                    .map(|v| project(v.get(), "tab"))
                    .collect::<Result<_>>()?,
            ),
            ("namespace", "recent") | ("tab", "entries") => Value::Array(
                array(raw.get())?
                    .iter()
                    .map(|v| project(v.get(), "entry"))
                    .collect::<Result<_>>()?,
            ),
            ("entry", "reference") => project(raw.get(), "reference")?,
            ("entry", "position") if raw.get().trim() != "null" => project(raw.get(), "position")?,
            _ => value(raw.get())?,
        };
        out.insert(key, parsed);
    }
    Ok(Value::Object(out))
}
pub(super) fn validate_existing(raw: &str) -> Result<()> {
    validate(&project(raw, "namespace")?)
}
pub(super) fn identity(raw: &str, context: &str) -> Result<Option<String>> {
    let fields = object(raw)?;
    if context == "tabs" {
        return fields
            .get("id")
            .map(|v| {
                serde_json::from_str::<String>(v.get())
                    .map(Some)
                    .map_err(|_| Error::Invalid)
            })
            .unwrap_or(Ok(None));
    }
    let reference = fields.get("reference").map(|v| v.get()).unwrap_or(raw);
    let projected = project(reference, "reference")?;
    reference_identity(&projected)
}
fn reference_identity(v: &Value) -> Result<Option<String>> {
    reference(v)?;
    serde_json::to_string(v)
        .map(Some)
        .map_err(|_| Error::Invalid)
}
