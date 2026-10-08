//! Presentation/index interpretation only. Capture admission independently
//! verifies body hashes and provenance; recognizing YAML grants no authority.
use comrak::nodes::{AstNode, NodeValue};

pub(crate) fn metadata_line(source: &str) -> Option<&str> {
    let mut lines = source.split('\n').map(|line| line.trim_end_matches('\r'));
    if lines.next()? != "<!-- mdreader-capture:v1 -->" || lines.next()? != "---" {
        return None;
    }
    let metadata_line = lines.next()?;
    let json = metadata_line.strip_prefix("mdreader_capture: ")?;
    if lines.next()? != "---" || lines.next().is_some_and(|line| !line.is_empty()) {
        return None;
    }
    let metadata: serde_json::Value = serde_json::from_str(json).ok()?;
    if metadata.get("schema_version").and_then(|v| v.as_u64()) != Some(1)
        || metadata
            .get("capture_is_not_approval")
            .and_then(|v| v.as_bool())
            != Some(true)
        || metadata
            .get("occurrences")
            .and_then(|v| v.as_array())
            .is_none_or(|v| v.is_empty())
    {
        return None;
    }
    Some(metadata_line)
}

pub(super) fn extract<'a>(root: &'a AstNode<'a>, source: &str) -> Option<String> {
    let metadata_line = metadata_line(source)?;
    let prefix: Vec<_> = root.children().take(3).collect();
    if prefix.len() != 3
        || !matches!(prefix[0].data.borrow().value, NodeValue::HtmlBlock(_))
        || !matches!(prefix[1].data.borrow().value, NodeValue::ThematicBreak)
        || !matches!(&prefix[2].data.borrow().value, NodeValue::Heading(h) if h.level == 2)
        || prefix[1].data.borrow().sourcepos.start.line != 2
        || prefix[2].data.borrow().sourcepos.start.line != 3
        || prefix[2].data.borrow().sourcepos.end.line != 4
    {
        return None;
    }
    for node in prefix {
        node.detach();
    }
    Some(metadata_line.into())
}

#[cfg(test)]
mod tests {
    use super::super::parse_markdown;

    #[test]
    fn capture_metadata_is_not_a_chunk_and_raw_body_line_positions_survive() {
        for ending in ["\n", "\r\n"] {
            let source = "<!-- mdreader-capture:v1 -->\n---\nmdreader_capture: {\"schema_version\":1,\"capture_is_not_approval\":true,\"occurrences\":[{\"session_id\":\"metadataonlytoken\"}]}\n---\n\n# Visible plan\n\nCobalt lighthouse body.\n".replace('\n', ending);
            let parsed = parse_markdown(&source, "capture");
            assert_eq!(parsed.title, "Visible plan");
            assert_eq!(parsed.sections.len(), 1);
            assert_eq!(parsed.sections[0].start_line, 6);
            assert_eq!(parsed.sections[0].blocks[0].start_line, 8);
            assert!(parsed.sections[0]
                .chunks
                .iter()
                .all(|chunk| !chunk.embedding_text.contains("metadataonlytoken")));
            assert_eq!(parsed.frontmatter[0].0, "mdreader_capture");
        }
    }

    #[test]
    fn malformed_capture_envelope_is_not_silently_removed() {
        let source =
            "<!-- mdreader-capture:v1 -->\n---\nmdreader_capture: invalid\n---\n\n# Body\n";
        assert!(parse_markdown(source, "capture").sections.iter().any(|s| s
            .heading_path
            .iter()
            .any(|h| h.contains("mdreader_capture"))));
    }
}
