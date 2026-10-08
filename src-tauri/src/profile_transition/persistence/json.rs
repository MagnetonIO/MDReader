//! Bounded raw JSON validation and recognized-field merge. Object values that
//! are not patched retain their exact lexemes (including large numeric values).
use super::types::{JsonKind, PersistenceError as Error, Result, MAX_JSON_BYTES, MAX_NODES};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{value::RawValue, Value};
use std::collections::{BTreeMap, VecDeque};

type Object = BTreeMap<String, Box<RawValue>>;

struct UniqueObject(Object);
impl<'de> Deserialize<'de> for UniqueObject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct Unique;
        impl<'de> Visitor<'de> for Unique {
            type Value = UniqueObject;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object with unique keys")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut values = Object::new();
                while let Some((key, value)) = map.next_entry::<String, Box<RawValue>>()? {
                    if values.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate JSON key"));
                    }
                    if values.len() > MAX_NODES {
                        return Err(serde::de::Error::custom("JSON node bound"));
                    }
                }
                Ok(UniqueObject(values))
            }
        }
        deserializer.deserialize_map(Unique)
    }
}

fn object(raw: &str) -> Result<Object> {
    serde_json::from_str::<UniqueObject>(raw)
        .map(|v| v.0)
        .map_err(|_| Error::Invalid)
}
fn array(raw: &str) -> Result<Vec<Box<RawValue>>> {
    serde_json::from_str(raw).map_err(|_| Error::Invalid)
}
fn value(raw: &str) -> Result<Value> {
    serde_json::from_str(raw).map_err(|_| Error::Invalid)
}
fn raw(value: &str) -> Result<Box<RawValue>> {
    RawValue::from_string(value.to_owned()).map_err(|_| Error::Invalid)
}
fn encode(values: &Object) -> Result<String> {
    serde_json::to_string(values).map_err(|_| Error::Invalid)
}

pub(super) fn validate(raw: &[u8]) -> Result<()> {
    if raw.is_empty() || raw.len() > MAX_JSON_BYTES {
        return Err(Error::TooLarge);
    }
    let text = std::str::from_utf8(raw).map_err(|_| Error::Invalid)?;
    let mut nodes = 0;
    validate_node(text, 0, &mut nodes)?;
    object(text)?;
    Ok(())
}
fn validate_node(text: &str, depth: usize, nodes: &mut usize) -> Result<()> {
    *nodes += 1;
    if depth > 32 || *nodes > MAX_NODES {
        return Err(Error::TooLarge);
    }
    match text.trim_start().as_bytes().first() {
        Some(b'{') => {
            for v in object(text)?.values() {
                validate_node(v.get(), depth + 1, nodes)?;
            }
        }
        Some(b'[') => {
            for v in array(text)? {
                validate_node(v.get(), depth + 1, nodes)?;
            }
        }
        _ => {
            serde_json::from_str::<Box<RawValue>>(text).map_err(|_| Error::Invalid)?;
        }
    }
    Ok(())
}

pub(super) fn merge(kind: JsonKind, original: Option<&[u8]>, patch: &str) -> Result<Vec<u8>> {
    validate(patch.as_bytes())?;
    let incoming = value(patch)?;
    validate_patch(kind, &incoming)?;
    let mut outer = match original {
        Some(bytes) => {
            validate(bytes)?;
            object(std::str::from_utf8(bytes).map_err(|_| Error::Invalid)?)?
        }
        None => Object::new(),
    };
    let old = outer.get(kind.key()).map(|v| v.get()).unwrap_or("{}");
    let old_fields = object(old)?;
    if kind == JsonKind::Session {
        if let Some(version) = old_fields.get("version") {
            if value(version.get())? != 1 {
                return Err(Error::UnsupportedSchema);
            }
        } else if !old_fields.is_empty() {
            return Err(Error::UnsupportedSchema);
        }
    }
    let merged = merge_object(old, patch)?;
    outer.insert(kind.key().to_owned(), raw(&merged)?);
    let bytes = encode(&outer)?.into_bytes();
    validate(&bytes)?;
    Ok(bytes)
}

fn merge_object(old: &str, patch: &str) -> Result<String> {
    let mut out = object(old)?;
    for (key, next) in object(patch)? {
        let merged = match out.get(&key) {
            Some(prior) if next.get().trim_start().starts_with('{') => {
                let same_reference = key != "docRef"
                    || identity(prior.get(), "history")? == identity(next.get(), "history")?;
                if same_reference && prior.get().trim_start().starts_with('{') {
                    merge_object(prior.get(), next.get())?
                } else {
                    next.get().to_owned()
                }
            }
            Some(prior) if matches!(key.as_str(), "tabs" | "recent" | "history") => {
                merge_array(prior.get(), next.get(), &key)?
            }
            _ => next.get().to_owned(),
        };
        out.insert(key, raw(&merged)?);
    }
    encode(&out)
}
fn identity(row: &str, context: &str) -> Result<Option<String>> {
    if !row.trim_start().starts_with('{') {
        return Ok(None);
    }
    let fields = object(row)?;
    let string = |key: &str| -> Option<String> {
        fields
            .get(key)
            .and_then(|raw| serde_json::from_str::<String>(raw.get()).ok())
    };
    if context == "tabs" {
        return Ok(string("id"));
    }
    if string("kind").as_deref() == Some("cloud") {
        // Cloud refs cannot revive native-issued authorities. They may occur in
        // legacy recents, but a matching key only preserves unrelated raw fields.
        return Ok(
            match (
                string("sourceId"),
                string("recordKey"),
                string("documentKey"),
            ) {
                (Some(source), Some(record), Some(document)) => {
                    serde_json::to_string(&("cloud", source, record, document)).ok()
                }
                _ => None,
            },
        );
    }
    Ok(
        string("path")
            .and_then(|path| serde_json::to_string(&("local", path, string("root"))).ok()),
    )
}
fn merge_array(old: &str, patch: &str, context: &str) -> Result<String> {
    let prior = if old.trim_start().starts_with('[') {
        array(old)?
    } else {
        Vec::new()
    };
    let mut keyed: BTreeMap<String, VecDeque<Box<RawValue>>> = BTreeMap::new();
    for entry in prior {
        // Unrepresentable/removed originals remain in exact preflush evidence.
        if let Some(key) = identity(entry.get(), context)? {
            let occurrences = keyed.entry(key).or_default();
            if context == "tabs" && !occurrences.is_empty() {
                return Err(Error::Invalid);
            }
            occurrences.push_back(entry);
        }
    }
    let mut out = Vec::new();
    for entry in array(patch)? {
        let key = identity(entry.get(), context)?.ok_or(Error::Invalid)?;
        let merged = match keyed.get_mut(&key).and_then(VecDeque::pop_front) {
            Some(prior) => merge_object(prior.get(), entry.get())?,
            None => entry.get().to_owned(),
        };
        out.push(raw(&merged)?);
    }
    serde_json::to_string(&out).map_err(|_| Error::Invalid)
}

fn keys(v: &Value, allowed: &[&str]) -> Result<()> {
    let fields = v.as_object().ok_or(Error::Invalid)?;
    if fields.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn string_or_null(v: Option<&Value>) -> bool {
    v.is_some_and(|v| v.is_null() || v.as_str().is_some_and(|s| s.len() <= 16384))
}
fn validate_patch(kind: JsonKind, v: &Value) -> Result<()> {
    match kind {
        JsonKind::Settings => {
            keys(
                v,
                &[
                    "theme",
                    "readingFont",
                    "textScaleIndex",
                    "sidebarCollapsed",
                    "outlineVisible",
                    "relatedRailOpen",
                    "inspectorOpen",
                    "lensSchema",
                    "reviewer",
                ],
            )?;
            for (key, value) in v.as_object().ok_or(Error::Invalid)? {
                let valid = match key.as_str() {
                    "theme" => matches!(value.as_str(), Some("system" | "light" | "dark")),
                    "readingFont" => matches!(value.as_str(), Some("sans" | "serif")),
                    "textScaleIndex" => value.as_u64().is_some_and(|v| v < 7),
                    "lensSchema" => string_or_null(Some(value)),
                    "reviewer" => value.as_str().is_some_and(|v| v.len() <= 16384),
                    _ => value.is_boolean(),
                };
                if !valid {
                    return Err(Error::Invalid);
                }
            }
        }
        JsonKind::Session => {
            keys(v, &["version", "root", "activeTabId", "tabs", "recent"])?;
            if v.get("version") != Some(&Value::from(1)) {
                return Err(Error::UnsupportedSchema);
            }
            if !string_or_null(v.get("root")) || !string_or_null(v.get("activeTabId")) {
                return Err(Error::Invalid);
            }
            let tabs = v
                .get("tabs")
                .and_then(Value::as_array)
                .ok_or(Error::Invalid)?;
            let recent = v
                .get("recent")
                .and_then(Value::as_array)
                .ok_or(Error::Invalid)?;
            if tabs.len() > 512 || recent.len() > 2048 {
                return Err(Error::TooLarge);
            }
            let mut ids = std::collections::BTreeSet::new();
            for tab in tabs {
                keys(
                    tab,
                    &[
                        "id",
                        "docRef",
                        "history",
                        "historyIndex",
                        "scroll",
                        "pinned",
                        "originRoot",
                    ],
                )?;
                let id = tab
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or(Error::Invalid)?;
                if id.is_empty() || id.len() > 1024 || !ids.insert(id) {
                    return Err(Error::Invalid);
                }
                reference(tab.get("docRef").ok_or(Error::Invalid)?)?;
                let history = tab
                    .get("history")
                    .and_then(Value::as_array)
                    .ok_or(Error::Invalid)?;
                if history.is_empty() || history.len() > 256 {
                    return Err(Error::Invalid);
                }
                for entry in history {
                    reference(entry)?;
                }
                if tab
                    .get("historyIndex")
                    .and_then(Value::as_u64)
                    .is_none_or(|i| i >= history.len() as u64)
                    || !tab.get("pinned").is_some_and(Value::is_boolean)
                    || !string_or_null(tab.get("originRoot"))
                {
                    return Err(Error::Invalid);
                }
                let scroll = tab.get("scroll").ok_or(Error::Invalid)?;
                keys(scroll, &["block", "offset"])?;
                if !scroll
                    .get("block")
                    .and_then(Value::as_f64)
                    .is_some_and(|v| v >= 0.0)
                    || !scroll
                        .get("offset")
                        .and_then(Value::as_f64)
                        .is_some_and(f64::is_finite)
                {
                    return Err(Error::Invalid);
                }
            }
            for entry in recent {
                reference(entry)?;
            }
        }
    }
    Ok(())
}
fn reference(v: &Value) -> Result<()> {
    keys(
        v,
        &[
            "kind",
            "path",
            "name",
            "root",
            "sourceId",
            "recordKey",
            "documentKey",
        ],
    )?;
    if v.get("kind")
        .is_some_and(|kind| !matches!(kind.as_str(), Some("local" | "cloud")))
    {
        return Err(Error::Invalid);
    }
    if v.get("name")
        .and_then(Value::as_str)
        .is_none_or(|v| v.len() > 16384)
    {
        return Err(Error::Invalid);
    }
    let required = if v.get("kind").and_then(Value::as_str) == Some("cloud") {
        &["sourceId", "recordKey", "documentKey"][..]
    } else {
        &["path"][..]
    };
    if required.iter().any(|key| {
        !v.get(key)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty() && s.len() <= 16384)
    }) {
        return Err(Error::Invalid);
    }
    Ok(())
}
