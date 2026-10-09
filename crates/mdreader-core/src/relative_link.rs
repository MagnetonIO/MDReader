//! Pure link parsing; a normalized relative name is never a source grant.
use crate::{ReaderError, Result};
use std::path::{Component, Path};

/// Candidate Markdown names within the already retained source. The reader still
/// must open them through its source authority and obtain actual byte revisions.
pub struct RelativeMarkdownTarget {
    pub paths: Vec<String>,
    pub heading: Option<String>,
}
fn invalid() -> ReaderError {
    ReaderError::Invalid("unsupported relative Markdown reference".into())
}
fn decode(raw: &str) -> Result<String> {
    let mut bytes = Vec::with_capacity(raw.len());
    let input = raw.as_bytes();
    let mut position = 0;
    while position < input.len() {
        if input[position] == b'%' {
            let pair = input.get(position + 1..position + 3).ok_or_else(invalid)?;
            let digit = |value: u8| (value as char).to_digit(16).ok_or_else(invalid);
            bytes.push((digit(pair[0])? * 16 + digit(pair[1])?) as u8);
            position += 3;
        } else {
            bytes.push(input[position]);
            position += 1;
        }
    }
    let decoded = String::from_utf8(bytes).map_err(|_| invalid())?;
    if decoded.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(decoded)
}
/// Strict one-time fragment decoding shared by indexed and relative navigation.
/// A decoded fragment is a renderer ID, never a native path or source authority.
pub fn decode_markdown_fragment(raw: &str) -> Result<String> {
    if raw.len() > 4096 || raw.chars().any(char::is_control) {
        return Err(invalid());
    }
    decode(raw)
}
impl RelativeMarkdownTarget {
    pub fn parse(from: &str, target: &str) -> Result<Self> {
        if from.is_empty()
            || from.len() > 4096
            || from.contains(['\\', ':'])
            || from.chars().any(char::is_control)
            || Path::new(from)
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            || target.len() > 4096
            || target.chars().any(char::is_control)
        {
            return Err(invalid());
        }
        let (path, heading) = target
            .split_once('#')
            .map(|(p, h)| (p, Some(h)))
            .unwrap_or((target, None));
        let path = decode(path)?;
        if path.starts_with('/') || path.contains(['\\', ':', '?']) {
            return Err(invalid());
        }
        let heading = heading
            .map(decode_markdown_fragment)
            .transpose()?
            .filter(|h| !h.is_empty());
        let resolved = if path.is_empty() {
            from.to_owned()
        } else {
            let mut parts: Vec<&str> = from
                .rsplit_once('/')
                .map(|(p, _)| p.split('/').collect())
                .unwrap_or_default();
            for part in path.split('/') {
                match part {
                    "" | "." => {}
                    ".." => {
                        parts.pop().ok_or_else(invalid)?;
                    }
                    _ => parts.push(part),
                }
            }
            parts.join("/")
        };
        if resolved.is_empty() || resolved.len() > 4096 {
            return Err(invalid());
        }
        let lower = resolved.to_ascii_lowercase();
        let paths = if lower.ends_with(".md") || lower.ends_with(".markdown") {
            vec![resolved]
        } else if Path::new(&resolved).extension().is_none() {
            vec![format!("{resolved}.md"), format!("{resolved}.markdown")]
        } else {
            return Err(invalid());
        };
        if paths.iter().any(|p| p.len() > 4096) {
            return Err(invalid());
        }
        Ok(Self { paths, heading })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relative_markdown_names_decode_once_and_never_supply_authority() {
        let parsed = RelativeMarkdownTarget::parse(
            "docs/nested/from.md",
            "../Other%20%CF%80.markdown#fn%3A1",
        )
        .unwrap();
        assert_eq!(parsed.paths, ["docs/Other π.markdown"]);
        assert_eq!(parsed.heading.as_deref(), Some("fn:1"));
        assert_eq!(
            RelativeMarkdownTarget::parse("docs/from.md", "#Raw%2520ID")
                .unwrap()
                .heading
                .as_deref(),
            Some("Raw%20ID")
        );
        assert_eq!(
            RelativeMarkdownTarget::parse("docs/from.md", "name")
                .unwrap()
                .paths,
            ["docs/name.md", "docs/name.markdown"]
        );
        assert_eq!(
            RelativeMarkdownTarget::parse("docs/from.md", "%252e%252e/next.MD")
                .unwrap()
                .paths,
            ["docs/%2e%2e/next.MD"]
        );
        for target in [
            "../../outside.md",
            "%2foutside.md",
            "//host/a.md",
            "https://example.test/a.md",
            "file:a.md",
            "%43%3a/a.md",
            "..%5coutside.md",
            "next.md?q=x",
            "next.md%00",
            "next.md#bad%0a",
            "%ZZ.md",
            "%.md",
            "%FF.md",
            "image.png",
            "asset.pdf",
        ] {
            assert!(
                RelativeMarkdownTarget::parse("docs/from.md", target).is_err(),
                "accepted {target}"
            );
        }
    }
}
