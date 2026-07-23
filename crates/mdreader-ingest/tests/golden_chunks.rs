//! Golden chunking tests: fixture markdown files → expected section/chunk
//! structure, snapshotted with insta. Any change to section boundaries,
//! heading paths, hashes, chunk splits, or link extraction shows up as a
//! snapshot diff (and, when intended, a `CHUNKER_VERSION` bump).

use mdreader_ingest::{parse_markdown, ParsedDocument};

fn corpus_file(rel: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../fixtures/corpus")
        .join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn stem(rel: &str) -> &str {
    let name = rel.rsplit('/').next().unwrap();
    name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name)
}

/// Render the parsed structure compactly and deterministically.
fn render(doc: &ParsedDocument) -> String {
    let mut out = String::new();
    out.push_str(&format!("title: {}\n", doc.title));
    if !doc.frontmatter.is_empty() {
        out.push_str("frontmatter:\n");
        for (k, v) in &doc.frontmatter {
            // An empty value renders as `key:` (no trailing whitespace).
            if v.is_empty() {
                out.push_str(&format!("  {k}:\n"));
            } else {
                out.push_str(&format!("  {k}: {v}\n"));
            }
        }
    }
    for (i, s) in doc.sections.iter().enumerate() {
        out.push_str(&format!(
            "section[{i}] depth={} lines={}..{} own_end={} parent={} hash={} path=[{}]\n",
            s.depth,
            s.start_line,
            s.end_line,
            s.own_end_line,
            s.parent.map(|p| p.to_string()).unwrap_or("-".to_string()),
            &s.content_hash["blake3:".len()..][..12],
            s.heading_path.join(" > "),
        ));
        for b in &s.blocks {
            out.push_str(&format!(
                "  block {} {}..{}{}\n",
                b.kind,
                b.start_line,
                b.end_line,
                b.lang
                    .as_deref()
                    .map(|l| format!(" lang={l}"))
                    .unwrap_or_default(),
            ));
        }
        for l in &s.links {
            out.push_str(&format!(
                "  link {} -> {} @{}\n",
                l.kind, l.raw_target, l.line
            ));
        }
        for c in &s.chunks {
            let head: String = c
                .embedding_text
                .chars()
                .take(60)
                .collect::<String>()
                .replace('\n', "\\n");
            out.push_str(&format!(
                "  chunk part={} tokens={} hash={} text={head:?}\n",
                c.part,
                c.token_count,
                &c.content_hash["blake3:".len()..][..12],
            ));
        }
    }
    out
}

fn golden(rel: &str) -> String {
    let source = corpus_file(rel);
    render(&parse_markdown(&source, stem(rel)))
}

#[test]
fn golden_index() {
    insta::assert_snapshot!(golden("index.md"));
}

#[test]
fn golden_wikilinks() {
    insta::assert_snapshot!(golden("notes/zettel/wikilinks.md"));
}

#[test]
fn golden_frontmatter_typical() {
    insta::assert_snapshot!(golden("frontmatter/typical.md"));
}

#[test]
fn golden_headings_deep() {
    insta::assert_snapshot!(golden("docs/reference/headings-deep.md"));
}

#[test]
fn golden_code_fences() {
    insta::assert_snapshot!(golden("docs/guides/code-fences.md"));
}

#[test]
fn golden_unicode_emoji_cjk() {
    insta::assert_snapshot!(golden("edge-cases/unicode-emoji-cjk.md"));
}

#[test]
fn golden_no_headings() {
    insta::assert_snapshot!(golden("docs/reference/no-headings.md"));
}

#[test]
fn golden_huge_paragraph_splits() {
    insta::assert_snapshot!(golden("edge-cases/huge-paragraph.md"));
}
