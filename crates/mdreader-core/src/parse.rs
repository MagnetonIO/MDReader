//! Markdown parser and chunker: comrak AST (with source positions) to a
//! structural [`ParsedDocument`] — heading-tree sections, blocks, links, and
//! embedding-ready chunks.
//!
//! The parser is deliberately **pure**: no ids, no storage, no I/O. The
//! ingestor layer decides identities and persistence; golden tests snapshot
//! this layer's output directly.

use comrak::nodes::NodeValue;
use comrak::{parse_document, Arena, Options};
use serde::Serialize;
pub(crate) mod capture_frontmatter;

/// Version tag of this parser/chunker, recorded on index versions: bump it
/// whenever section or chunk boundaries change so re-imports re-embed.
pub const CHUNKER_VERSION: &str = "mdreader-chunker@3";
pub(crate) const PARSER_VERSION: i64 = 3;

/// Sections longer than this (estimated tokens) are split at paragraph
/// boundaries into multiple chunks (the default for [`parse_markdown`] and
/// the default parser).
pub const CHUNK_TOKEN_LIMIT: u32 = 400;

/// Estimated tokens: ~4 characters per token, the standard rough cut for
/// English prose and code alike.
fn estimate_tokens(text: &str) -> u32 {
    (text.chars().count() as u32).div_ceil(4)
}

/// A parsed markdown document.
#[derive(Debug, Clone, Serialize)]
pub struct ParsedDocument {
    /// Title: the first level-1 heading, or the file stem.
    pub title: String,
    /// Tolerantly parsed `key: value` frontmatter pairs (never fails; lines
    /// that are not simple scalars are skipped).
    pub frontmatter: Vec<(String, String)>,
    /// Heading-delimited sections in document order (a preamble section with
    /// an empty heading path first, when content precedes the first heading).
    pub sections: Vec<ParsedSection>,
}

/// One heading-delimited section.
#[derive(Debug, Clone, Serialize)]
pub struct ParsedSection {
    /// Heading breadcrumb from the document root down to (and including)
    /// this section's own heading. Empty for the preamble.
    pub heading_path: Vec<String>,
    /// Heading level (1–6); 0 for the preamble.
    pub depth: u8,
    /// First source line of the section span (1-based, inclusive).
    pub start_line: u32,
    /// Last source line of the span: content runs until the next heading of
    /// depth ≤ this section's (so the span includes subsections).
    pub end_line: u32,
    /// Last line of the section's *own* content (before the first subsection).
    pub own_end_line: u32,
    /// Index (into [`ParsedDocument::sections`]) of the parent section.
    pub parent: Option<usize>,
    /// blake3 hash of `(title, heading_path, own text)` — heading renames
    /// re-hash descendants because breadcrumbs feed the embedding text.
    pub content_hash: String,
    /// The section's own raw text (heading line + content, subsections
    /// excluded), newline-normalized.
    pub text: String,
    /// Top-level blocks within the section's own content.
    pub blocks: Vec<ParsedBlock>,
    /// Links written in the section's own content.
    pub links: Vec<ParsedLink>,
    /// Retrieval chunks derived from the section (parts of a split share the
    /// heading path).
    pub chunks: Vec<ParsedChunk>,
}

/// A top-level block within a section.
#[derive(Debug, Clone, Serialize)]
pub struct ParsedBlock {
    /// Block kind (`paragraph`, `code`, `list`, `table`, `quote`, `html`,
    /// `rule`, `footnote`, `heading`...).
    pub kind: String,
    /// Code-fence language, when present.
    pub lang: Option<String>,
    /// First source line (1-based, inclusive).
    pub start_line: u32,
    /// Last source line (1-based, inclusive).
    pub end_line: u32,
}

/// A link discovered in a section.
#[derive(Debug, Clone, Serialize)]
pub struct ParsedLink {
    /// The raw target exactly as written (fragment included).
    pub raw_target: String,
    /// `markdown`, `wikilink`, or `image`.
    pub kind: String,
    /// Source line the link starts on.
    pub line: u32,
}

/// One embedding-ready chunk.
#[derive(Debug, Clone, Serialize)]
pub struct ParsedChunk {
    /// Part ordinal within the section (0-based).
    pub part: u32,
    /// Human-facing chunk text (raw markdown slice).
    pub text: String,
    /// Text fed to the embedding model:
    /// `"{title} > {h1} > {h2}\n\n{text}"` (breadcrumb prefix).
    pub embedding_text: String,
    /// blake3 hash of `embedding_text`.
    pub content_hash: String,
    /// Estimated token count of `embedding_text`.
    pub token_count: u32,
}

fn blake3_hex(input: &str) -> String {
    format!("blake3:{}", blake3::hash(input.as_bytes()).to_hex())
}

fn comrak_options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.front_matter_delimiter = Some("---".to_string());
    options.extension.wikilinks_title_after_pipe = true;
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.tasklist = true;
    options.extension.autolink = true;
    options.extension.footnotes = true;
    options
}

/// Collect the plain text of a heading node (concatenated literals).
fn heading_text<'a>(node: &'a comrak::nodes::AstNode<'a>) -> String {
    let mut out = String::new();
    for child in node.descendants().skip(1) {
        match &child.data.borrow().value {
            NodeValue::Text(t) => out.push_str(t),
            NodeValue::Code(code) => out.push_str(&code.literal),
            NodeValue::SoftBreak | NodeValue::LineBreak => out.push(' '),
            _ => {}
        }
    }
    out.trim().to_string()
}

fn block_kind(value: &NodeValue) -> (&'static str, Option<String>) {
    match value {
        NodeValue::Paragraph => ("paragraph", None),
        NodeValue::CodeBlock(cb) => {
            let lang = cb.info.split_whitespace().next().unwrap_or("");
            (
                "code",
                if lang.is_empty() {
                    None
                } else {
                    Some(lang.to_string())
                },
            )
        }
        NodeValue::List(_) => ("list", None),
        NodeValue::Table(_) => ("table", None),
        NodeValue::BlockQuote => ("quote", None),
        NodeValue::HtmlBlock(_) => ("html", None),
        NodeValue::ThematicBreak => ("rule", None),
        NodeValue::FootnoteDefinition(_) => ("footnote", None),
        NodeValue::Heading(_) => ("heading", None),
        _ => ("other", None),
    }
}

/// Tolerant frontmatter parsing: `key: value` lines between the delimiters;
/// block-list items (`- item`) following a bare `key:` line are recorded as
/// repeated `(key, item)` pairs (so `tags:` lists survive extraction);
/// anything else (deeper nesting, malformed lines) is skipped, never an
/// error.
fn parse_frontmatter(raw: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut list_key: Option<String> = None;
    for line in raw.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim() == "---" || trimmed.trim().is_empty() {
            continue;
        }
        // Block-list items attach to the preceding bare key as repeated
        // (key, item) pairs; a dash with no open list is skipped.
        if let Some(item) = trimmed.trim_start().strip_prefix('-') {
            if let Some(key) = &list_key {
                let value = item.trim().trim_matches('"').trim_matches('\'');
                if !value.is_empty() {
                    out.push((key.clone(), value.to_string()));
                }
            }
            continue;
        }
        // Other indented lines belong to nested structures; skip tolerantly.
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            let key = key.trim();
            if key.is_empty() || key.contains(' ') {
                list_key = None;
                continue;
            }
            let value = value.trim().trim_matches('"').to_string();
            list_key = value.is_empty().then(|| key.to_string());
            out.push((key.to_string(), value));
        } else {
            list_key = None;
        }
    }
    out
}

/// Split text into paragraph-ish segments at blank lines, preserving content.
fn split_paragraphs(text: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                segments.push(current.join("\n"));
                current.clear();
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        segments.push(current.join("\n"));
    }
    segments
}

/// Breadcrumb prefix for embedding text: title plus heading path, with a
/// consecutive duplicate collapsed (the title usually *is* the first h1).
fn breadcrumb(title: &str, heading_path: &[String]) -> String {
    let mut parts: Vec<&str> = vec![title];
    for part in heading_path {
        if parts.last() != Some(&part.as_str()) {
            parts.push(part);
        }
    }
    parts.join(" > ")
}

fn build_chunks(
    title: &str,
    heading_path: &[String],
    text: &str,
    token_limit: u32,
) -> Vec<ParsedChunk> {
    let prefix = breadcrumb(title, heading_path);
    let make = |part: u32, chunk_text: &str| {
        let embedding_text = format!("{prefix}\n\n{chunk_text}");
        ParsedChunk {
            part,
            text: chunk_text.to_string(),
            content_hash: blake3_hex(&embedding_text),
            token_count: estimate_tokens(&embedding_text),
            embedding_text,
        }
    };
    if estimate_tokens(text) <= token_limit {
        if text.trim().is_empty() {
            return Vec::new();
        }
        return vec![make(0, text)];
    }
    // Oversized section: split at paragraph boundaries; parts share the
    // heading path. A single paragraph larger than the limit stays whole
    // (the embedding model truncates; splitting mid-paragraph would corrupt
    // the text more than truncation does).
    let mut chunks = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut current_tokens = 0u32;
    for segment in split_paragraphs(text) {
        let segment_tokens = estimate_tokens(&segment);
        if !current.is_empty() && current_tokens + segment_tokens > token_limit {
            chunks.push(current.join("\n\n"));
            current.clear();
            current_tokens = 0;
        }
        current_tokens += segment_tokens;
        current.push(segment);
    }
    if !current.is_empty() {
        chunks.push(current.join("\n\n"));
    }
    chunks
        .iter()
        .enumerate()
        .map(|(i, c)| make(i as u32, c))
        .collect()
}

/// Parse a markdown source into its structural form. Never fails: malformed
/// input degrades to whatever CommonMark makes of it. Chunks split at the
/// default [`CHUNK_TOKEN_LIMIT`].
pub fn parse_markdown(source: &str, fallback_title: &str) -> ParsedDocument {
    parse_markdown_with_limit(source, fallback_title, CHUNK_TOKEN_LIMIT)
}

/// [`parse_markdown`] with an explicit chunk token limit. A different limit
/// produces different chunk *boundaries* for the same source — which is
/// exactly a chunker change, so any caller deviating from the default must
/// also carry a distinct chunker version (see
/// the configured chunk limit).
pub fn parse_markdown_with_limit(
    source: &str,
    fallback_title: &str,
    chunk_token_limit: u32,
) -> ParsedDocument {
    let arena = Arena::new();
    let options = comrak_options();
    let root = parse_document(&arena, source, &options);
    let capture_metadata = capture_frontmatter::extract(root, source);

    let lines: Vec<&str> = source.lines().collect();
    let last_line = lines.len() as u32;

    // Top-level pass: frontmatter, headings, and content blocks.
    struct HeadingItem {
        level: u8,
        text: String,
        start_line: u32,
    }
    enum Item {
        Heading(HeadingItem),
        Block(ParsedBlock),
    }
    let mut frontmatter = capture_metadata
        .as_deref()
        .map(parse_frontmatter)
        .unwrap_or_default();
    let mut items: Vec<Item> = Vec::new();
    for child in root.children() {
        let data = child.data.borrow();
        let sp = data.sourcepos;
        match &data.value {
            NodeValue::FrontMatter(raw) => frontmatter = parse_frontmatter(raw),
            NodeValue::Heading(h) => items.push(Item::Heading(HeadingItem {
                level: h.level,
                text: heading_text(child),
                start_line: sp.start.line as u32,
            })),
            value => {
                let (kind, lang) = block_kind(value);
                items.push(Item::Block(ParsedBlock {
                    kind: kind.to_string(),
                    lang,
                    start_line: sp.start.line as u32,
                    end_line: sp.end.line as u32,
                }));
            }
        }
    }

    // Title: first h1 anywhere at the top level, else the file stem.
    let title = items
        .iter()
        .find_map(|item| match item {
            Item::Heading(h) if h.level == 1 && !h.text.is_empty() => Some(h.text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| fallback_title.to_string());

    // All links in the document, assigned to sections by line afterwards.
    let mut all_links: Vec<ParsedLink> = Vec::new();
    for node in root.descendants() {
        let data = node.data.borrow();
        let line = data.sourcepos.start.line as u32;
        match &data.value {
            NodeValue::Link(link) => all_links.push(ParsedLink {
                raw_target: link.url.clone(),
                kind: "markdown".to_string(),
                line,
            }),
            NodeValue::Image(image) => all_links.push(ParsedLink {
                raw_target: image.url.clone(),
                kind: "image".to_string(),
                line,
            }),
            NodeValue::WikiLink(wiki) => all_links.push(ParsedLink {
                raw_target: wiki.url.clone(),
                kind: "wikilink".to_string(),
                line,
            }),
            _ => {}
        }
    }

    // Section boundaries.
    struct Bound {
        heading_path: Vec<String>,
        depth: u8,
        start_line: u32,
        end_line: u32,
        own_end_line: u32,
        parent: Option<usize>,
        blocks: Vec<ParsedBlock>,
    }
    let heading_positions: Vec<(usize, u8, u32)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| match item {
            Item::Heading(h) => Some((i, h.level, h.start_line)),
            Item::Block(_) => None,
        })
        .collect();

    let mut bounds: Vec<Bound> = Vec::new();
    // Preamble: content blocks before the first heading.
    let first_heading_item = heading_positions.first().map(|(i, _, _)| *i);
    let preamble_blocks: Vec<ParsedBlock> = items
        .iter()
        .take(first_heading_item.unwrap_or(items.len()))
        .filter_map(|item| match item {
            Item::Block(b) => Some(b.clone()),
            Item::Heading(_) => None,
        })
        .collect();
    if !preamble_blocks.is_empty() {
        let start = preamble_blocks.first().map(|b| b.start_line).unwrap_or(1);
        let end = preamble_blocks.last().map(|b| b.end_line).unwrap_or(start);
        bounds.push(Bound {
            heading_path: Vec::new(),
            depth: 0,
            start_line: start,
            end_line: end,
            own_end_line: end,
            parent: None,
            blocks: preamble_blocks,
        });
    }

    let mut stack: Vec<String> = Vec::new();
    for (pos_idx, &(item_idx, level, start_line)) in heading_positions.iter().enumerate() {
        // Own content: until the next heading of any level.
        let own_end_line = heading_positions
            .get(pos_idx + 1)
            .map(|&(_, _, next_start)| next_start.saturating_sub(1))
            .unwrap_or(last_line)
            .max(start_line);
        // Span: until the next heading of depth <= level.
        let end_line = heading_positions[pos_idx + 1..]
            .iter()
            .find(|&&(_, l, _)| l <= level)
            .map(|&(_, _, s)| s.saturating_sub(1))
            .unwrap_or(last_line)
            .max(start_line);

        let text = match &items[item_idx] {
            Item::Heading(h) => h.text.clone(),
            Item::Block(_) => unreachable!("heading_positions only indexes headings"),
        };
        stack.truncate((level as usize).saturating_sub(1).min(stack.len()));
        stack.push(text);
        let heading_path = stack.clone();

        // Parent: nearest previous heading section with a smaller depth.
        let parent = bounds
            .iter()
            .rposition(|b| b.depth >= 1 && b.depth < level)
            .or(None);

        // Blocks in own content (top-level items between this heading and the
        // next one).
        let next_item = heading_positions
            .get(pos_idx + 1)
            .map(|&(i, _, _)| i)
            .unwrap_or(items.len());
        let blocks: Vec<ParsedBlock> = items[item_idx + 1..next_item]
            .iter()
            .filter_map(|item| match item {
                Item::Block(b) => Some(b.clone()),
                Item::Heading(_) => None,
            })
            .collect();

        bounds.push(Bound {
            heading_path,
            depth: level,
            start_line,
            end_line,
            own_end_line,
            parent,
            blocks,
        });
    }

    // Assemble sections: slice text, hash, chunk, and assign links.
    let mut sections = Vec::with_capacity(bounds.len());
    for bound in bounds {
        let start = bound.start_line.max(1) as usize;
        let end = (bound.own_end_line as usize).min(lines.len());
        let text = if start <= end {
            lines[start - 1..end].join("\n")
        } else {
            String::new()
        };
        let hash_input = format!(
            "{title}\u{1f}{}\u{1e}{text}",
            bound.heading_path.join("\u{1f}")
        );
        let links: Vec<ParsedLink> = all_links
            .iter()
            .filter(|l| l.line >= bound.start_line && l.line <= bound.own_end_line)
            .cloned()
            .collect();
        sections.push(ParsedSection {
            chunks: build_chunks(&title, &bound.heading_path, &text, chunk_token_limit),
            content_hash: blake3_hex(&hash_input),
            text,
            heading_path: bound.heading_path,
            depth: bound.depth,
            start_line: bound.start_line,
            end_line: bound.end_line,
            own_end_line: bound.own_end_line,
            parent: bound.parent,
            blocks: bound.blocks,
            links,
        });
    }

    ParsedDocument {
        title,
        frontmatter,
        sections,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_comes_from_first_h1_or_fallback() {
        let doc = parse_markdown("# Hello\n\nBody.\n", "fallback");
        assert_eq!(doc.title, "Hello");
        let doc = parse_markdown("## Only h2\n", "fallback");
        assert_eq!(doc.title, "fallback");
        let doc = parse_markdown("", "empty");
        assert_eq!(doc.title, "empty");
        assert!(doc.sections.is_empty());
    }

    #[test]
    fn heading_tree_builds_paths_spans_and_parents() {
        let src = "\
intro before any heading

# A

a-body

## B

b-body

### C

c-body

## D

d-body

# E

e-body
";
        let doc = parse_markdown(src, "t");
        let paths: Vec<(Vec<String>, u8, Option<usize>)> = doc
            .sections
            .iter()
            .map(|s| (s.heading_path.clone(), s.depth, s.parent))
            .collect();
        assert_eq!(
            paths,
            vec![
                (vec![], 0, None),
                (vec!["A".into()], 1, None),
                (vec!["A".into(), "B".into()], 2, Some(1)),
                (vec!["A".into(), "B".into(), "C".into()], 3, Some(2)),
                (vec!["A".into(), "D".into()], 2, Some(1)),
                (vec!["E".into()], 1, None),
            ]
        );
        // A's span covers B/C/D; its own content stops before B.
        let a = &doc.sections[1];
        assert_eq!(a.start_line, 3);
        assert_eq!(a.own_end_line, 6);
        assert_eq!(a.end_line, 18);
        // E runs to EOF.
        let e = &doc.sections[5];
        assert_eq!(e.end_line, 21);
    }

    #[test]
    fn section_hash_is_stable_under_line_shifts_but_not_heading_renames() {
        let a = parse_markdown("# T\n\n## S\n\nbody\n", "t");
        let b = parse_markdown("# T\n\nmore preamble here\n\n## S\n\nbody\n", "t");
        let hash = |d: &ParsedDocument| {
            d.sections
                .iter()
                .find(|s| s.depth == 2)
                .unwrap()
                .content_hash
                .clone()
        };
        assert_eq!(hash(&a), hash(&b), "line shifts must not change the hash");
        let c = parse_markdown("# T\n\n## Renamed\n\nbody\n", "t");
        assert_ne!(hash(&a), hash(&c), "heading renames must re-hash");
    }

    #[test]
    fn links_are_extracted_with_kinds_and_assigned_to_sections() {
        let src = "\
# Top

See [other](notes/other.md) and ![img](assets/a.png).

## Wiki

A [[target|label]] and [[plain]].
";
        let doc = parse_markdown(src, "t");
        let top = &doc.sections[0];
        let kinds: Vec<(&str, &str)> = top
            .links
            .iter()
            .map(|l| (l.kind.as_str(), l.raw_target.as_str()))
            .collect();
        assert_eq!(
            kinds,
            vec![("markdown", "notes/other.md"), ("image", "assets/a.png")]
        );
        let wiki = &doc.sections[1];
        let kinds: Vec<(&str, &str)> = wiki
            .links
            .iter()
            .map(|l| (l.kind.as_str(), l.raw_target.as_str()))
            .collect();
        assert_eq!(kinds, vec![("wikilink", "target"), ("wikilink", "plain")]);
    }

    #[test]
    fn frontmatter_parses_tolerantly() {
        let src = "---\ntitle: My Note\ntags:\n  - a\n  - b\nbroken line without colon\nrating: 4.5\n---\n\n# H\n";
        let doc = parse_markdown(src, "t");
        assert_eq!(
            doc.frontmatter,
            vec![
                ("title".to_string(), "My Note".to_string()),
                ("tags".to_string(), String::new()),
                ("tags".to_string(), "a".to_string()),
                ("tags".to_string(), "b".to_string()),
                ("rating".to_string(), "4.5".to_string()),
            ]
        );
        // Title still comes from the h1, not the frontmatter.
        assert_eq!(doc.title, "H");
    }

    #[test]
    fn frontmatter_block_lists_end_at_the_next_key_or_junk() {
        let src = "---\naliases:\n  - \"one\"\n- two\nother: x\n- stray\nplain junk\n- more stray\n---\n\nbody\n";
        let doc = parse_markdown(src, "t");
        assert_eq!(
            doc.frontmatter,
            vec![
                ("aliases".to_string(), String::new()),
                ("aliases".to_string(), "one".to_string()),
                ("aliases".to_string(), "two".to_string()),
                ("other".to_string(), "x".to_string()),
            ]
        );
    }

    #[test]
    fn embedding_text_carries_the_breadcrumb() {
        let doc = parse_markdown("# Guide\n\n## Search\n\nhybrid retrieval\n", "t");
        let search = &doc.sections[1];
        assert_eq!(search.chunks.len(), 1);
        assert!(search.chunks[0]
            .embedding_text
            .starts_with("Guide > Search\n\n"));
    }

    #[test]
    fn oversized_sections_split_at_paragraph_boundaries() {
        let paragraph = "word ".repeat(1200); // ~1500 tokens
        let src = format!("# Big\n\n{p}\n\n{p}\n\n{p}\n", p = paragraph.trim());
        let doc = parse_markdown(&src, "t");
        let big = &doc.sections[0];
        assert!(big.chunks.len() >= 2, "must split: {}", big.chunks.len());
        for (i, chunk) in big.chunks.iter().enumerate() {
            assert_eq!(chunk.part, i as u32);
            assert!(chunk.embedding_text.starts_with("Big\n\n"));
        }
    }

    #[test]
    fn crlf_input_is_newline_normalized() {
        let doc = parse_markdown("# A\r\n\r\nbody line\r\n", "t");
        assert_eq!(doc.sections[0].text, "# A\n\nbody line");
    }
}
