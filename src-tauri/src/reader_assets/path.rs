//! URI names only. This parser never opens or grants a filesystem path.
use super::{AssetFailure as F, AssetReadError};
use std::path::{Component, Path};

#[derive(PartialEq, Eq)]
pub(super) struct Target {
    pub path: String,
    pub fragment: Option<String>,
}
pub(super) fn origin(path: &str) -> Result<(), AssetReadError> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains([':', '\\'])
        || path.chars().any(char::is_control)
        || path.split('/').count() > 128
        || path.split('/').any(|part| matches!(part, "" | "." | ".."))
        || Path::new(path)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(F::Invalid.error());
    }
    Ok(())
}
pub(super) fn resolve(from: &str, raw: &str) -> Result<Target, AssetReadError> {
    origin(from)?;
    if raw.is_empty() || raw.len() > 4096 || raw.chars().any(char::is_control) {
        return Err(F::Invalid.error());
    }
    let (path, fragment) = raw
        .split_once('#')
        .map(|(p, f)| (p, Some(f)))
        .unwrap_or((raw, None));
    let decoded = mdreader_core::decode_markdown_fragment(path).map_err(|_| F::Invalid.error())?;
    if decoded.is_empty()
        || decoded.starts_with('/')
        || decoded.contains([':', '\\', '?', '#', '%'])
    {
        return Err(F::Invalid.error());
    }
    let fragment = fragment
        .map(mdreader_core::decode_markdown_fragment)
        .transpose()
        .map_err(|_| F::Invalid.error())?
        .filter(|f| !f.is_empty());
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for part in decoded.split('/') {
        match part {
            "" => return Err(F::Invalid.error()),
            "." => {}
            ".." => {
                parts.pop().ok_or_else(|| F::Unauthorized.error())?;
            }
            value => parts.push(value),
        }
    }
    let path = parts.join("/");
    origin(&path)?;
    Ok(Target { path, fragment })
}
