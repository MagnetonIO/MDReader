//! Mandatory privacy matching; filesystem traversal remains descriptor-owned.
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

/// Mandatory exclusions use canonical Unicode case matching, conservatively
/// including uppercase-table equivalence. This narrows case-sensitive hosts,
/// covers Unicode aliases without changing descriptor traversal, and preserves
/// exact shared ancestors and unrelated folders.
pub(crate) fn exclusion_relative(path: &Path, prefix: &Path) -> Option<PathBuf> {
    let mut parts = path.components();
    for required in prefix.components() {
        let actual = parts.next()?;
        if !private_component_matches(actual.as_os_str(), required.as_os_str()) {
            return None;
        }
    }
    Some(parts.collect())
}
/// DOS generated short-name spelling is ambiguous filesystem authority. Native
/// Windows protected sources conservatively refuse it; never infer a private
/// path is disjoint because its long spelling differs.
#[cfg(any(windows, test))]
pub(crate) fn short_name_component(component: &OsStr) -> bool {
    let bytes = component.as_encoded_bytes();
    if !bytes.is_ascii() {
        return false;
    }
    let mut parts = bytes.split(|b| *b == b'.');
    let base = parts.next().unwrap_or_default();
    let extension = parts.next();
    if base.len() > 8 || extension.is_some_and(|e| e.len() > 3) || parts.next().is_some() {
        return false;
    }
    let Some(tilde) = base.iter().position(|b| *b == b'~') else {
        return false;
    };
    tilde > 0 && tilde + 1 < base.len() && base[tilde + 1..].iter().all(u8::is_ascii_digit)
}
pub(crate) fn private_path_ambiguous(path: &Path) -> bool {
    #[cfg(windows)]
    if path
        .components()
        .any(|c| short_name_component(c.as_os_str()))
    {
        return true;
    }
    path.as_os_str().as_encoded_bytes().len() > 32768 || path.components().count() > 512
}

fn private_component_matches(actual: &OsStr, required: &OsStr) -> bool {
    let a = actual.as_encoded_bytes();
    let b = required.as_encoded_bytes();
    if a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(b);
    }
    let (Some(a), Some(b)) = (actual.to_str(), required.to_str()) else {
        // Unrepresentable private names cannot prove component disjointness.
        return true;
    };
    caseless::canonical_caseless_match_str(a, b)
        || caseless::canonical_caseless_match_str(&a.to_uppercase(), &b.to_uppercase())
}
