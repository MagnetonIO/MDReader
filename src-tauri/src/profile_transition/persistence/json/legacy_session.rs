//! Validate only explicitly patched legacy fields; omitted original rows stay raw.
use super::{keys, reference, string_or_null};
use crate::profile_transition::persistence::types::{PersistenceError as Error, Result};
use serde_json::Value;
pub(super) fn validate(v: &Value) -> Result<()> {
    for key in ["root", "activeTabId"] {
        if v.get(key).is_some() && !string_or_null(v.get(key)) {
            return Err(Error::Invalid);
        }
    }
    if let Some(tabs) = v.get("tabs") {
        let tabs = tabs.as_array().ok_or(Error::Invalid)?;
        if tabs.len() > 512 {
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
            for item in history {
                reference(item)?;
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
    }
    if let Some(recent) = v.get("recent") {
        let recent = recent.as_array().ok_or(Error::Invalid)?;
        if recent.len() > 2048 {
            return Err(Error::TooLarge);
        }
        for item in recent {
            reference(item)?;
        }
    }
    Ok(())
}
