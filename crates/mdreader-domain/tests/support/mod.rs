//! A deliberately minimal markdown reader for this crate's fixtures.
//!
//! `mdreader-domain` is parser-free by design: the real parser lives in
//! `mdreader-ingest`, which depends on *this* crate, so it cannot be used
//! here. The extractors consume a
//! [`SourceDocument`](mdreader_domain::extract::SourceDocument) view, and the
//! fixtures under `tests/fixtures/` are written to be readable by the small
//! line-based scanner below.
//!
//! This is a **test fixture producer, not a markdown parser**. It handles
//! exactly what the fixtures use: `---` frontmatter, ATX headings, fenced
//! code blocks (skipped), wikilinks, and inline links. Anything richer is out
//! of scope on purpose — the point of the fixtures is that the extraction
//! rules are pinned against input a human can read and line-count by eye.

#![allow(dead_code)] // each integration test uses a different subset

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use mdreader_domain::extract::{SourceDocument, SourceLink, SourceSection};

/// blake3 hash of a string, in the `blake3:<hex>` form the real parser uses.
pub fn hash(text: &str) -> String {
    format!("blake3:{}", blake3::hash(text.as_bytes()).to_hex())
}

/// Repo-relative paths of every `.md` file under `root`, sorted.
pub fn markdown_paths(root: &Path) -> Vec<String> {
    let mut paths = Vec::new();
    collect(root, root, &mut paths);
    paths.sort();
    paths
}

/// Read every `.md` file under `root`, in sorted repo-relative path order.
pub fn read_corpus(root: &Path) -> Vec<SourceDocument> {
    markdown_paths(root)
        .iter()
        .map(|relative| {
            let text = fs::read_to_string(root.join(relative)).expect("fixture is readable");
            parse(relative, &text)
        })
        .collect()
}

/// Byte-level fingerprint of every file under `root` (not just markdown), so
/// a non-destructive claim covers stray writes of any kind.
pub fn fingerprint(root: &Path) -> BTreeMap<String, String> {
    let mut all = Vec::new();
    collect_any(root, root, &mut all);
    all.into_iter()
        .map(|relative| {
            let bytes = fs::read(root.join(&relative)).expect("file is readable");
            let digest = format!("blake3:{}", blake3::hash(&bytes).to_hex());
            (relative, digest)
        })
        .collect()
}

/// Copy a directory tree into `destination` (used to run extraction against a
/// scratch copy, so the assertion is about behaviour rather than about the
/// repo happening to be read-only).
pub fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create destination");
    for entry in fs::read_dir(source).expect("readable source") {
        let entry = entry.expect("dir entry");
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for path in sorted_entries(dir) {
        if path.is_dir() {
            collect(root, &path, out);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            out.push(relative_of(root, &path));
        }
    }
}

fn collect_any(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for path in sorted_entries(dir) {
        if path.is_dir() {
            collect_any(root, &path, out);
        } else {
            out.push(relative_of(root, &path));
        }
    }
}

fn sorted_entries(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .expect("directory is readable")
        .map(|entry| entry.expect("dir entry").path())
        .collect();
    entries.sort();
    entries
}

fn relative_of(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .expect("under root")
        .to_string_lossy()
        .replace('\\', "/")
}

/// Parse one fixture into the extractor's input view.
pub fn parse(path: &str, text: &str) -> SourceDocument {
    let lines: Vec<&str> = text.lines().collect();
    let mut cursor = 0usize;
    let mut frontmatter = Vec::new();

    if lines.first().is_some_and(|line| line.trim() == "---") {
        cursor = 1;
        while cursor < lines.len() && lines[cursor].trim() != "---" {
            if let Some((key, value)) = lines[cursor].split_once(':') {
                frontmatter.push((key.trim().to_string(), value.trim().to_string()));
            }
            cursor += 1;
        }
        cursor += 1; // step past the closing `---`
    }

    // Heading scan; line numbers are 1-based.
    let mut headings: Vec<(u32, u8, String)> = Vec::new();
    let mut in_fence = false;
    for (index, line) in lines.iter().enumerate().skip(cursor) {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && line.chars().nth(hashes) == Some(' ') {
            headings.push((
                index as u32 + 1,
                hashes as u8,
                line[hashes + 1..].trim().to_string(),
            ));
        }
    }

    let mut sections: Vec<SourceSection> = Vec::new();
    let mut breadcrumb: Vec<(u8, String, usize)> = Vec::new(); // (depth, heading, index)
    for (position, (start_line, depth, heading)) in headings.iter().enumerate() {
        // The span runs until the next heading of depth <= this one.
        let end_line = headings[position + 1..]
            .iter()
            .find(|(_, next_depth, _)| next_depth <= depth)
            .map_or(lines.len() as u32, |(next_start, _, _)| next_start - 1);
        // The section's OWN content stops at its first subsection: links
        // below a nested heading belong to that heading (see
        // `SourceSection::links`).
        let own_end_line = headings
            .get(position + 1)
            .map_or(end_line, |(next_start, _, _)| {
                (next_start - 1).min(end_line)
            });
        while breadcrumb.last().is_some_and(|(d, _, _)| d >= depth) {
            breadcrumb.pop();
        }
        let parent = breadcrumb.last().map(|(_, _, index)| *index);
        let mut heading_path: Vec<String> =
            breadcrumb.iter().map(|(_, text, _)| text.clone()).collect();
        heading_path.push(heading.clone());
        let own_text = lines[(*start_line as usize - 1)..(own_end_line as usize)].join("\n");
        sections.push(SourceSection {
            heading_path,
            depth: *depth,
            start_line: *start_line,
            end_line,
            own_end_line,
            parent,
            content_hash: hash(&own_text),
            links: scan_links(&lines, *start_line, own_end_line),
        });
        breadcrumb.push((*depth, heading.clone(), sections.len() - 1));
    }

    let title = headings
        .iter()
        .find(|(_, depth, _)| *depth == 1)
        .map(|(_, _, heading)| heading.clone())
        .unwrap_or_else(|| {
            path.rsplit('/')
                .next()
                .unwrap_or(path)
                .trim_end_matches(".md")
                .to_string()
        });

    SourceDocument {
        path: path.to_string(),
        title,
        // The newline-normalized text, i.e. exactly the lines `span()` names
        // — see `SourceDocument::content_hash`.
        content_hash: hash(&lines.join("\n")),
        line_count: lines.len() as u32,
        frontmatter,
        sections,
    }
}

/// Links written on lines `start..=end` (1-based, inclusive), outside fences.
fn scan_links(lines: &[&str], start: u32, end: u32) -> Vec<SourceLink> {
    let mut links = Vec::new();
    let mut in_fence = false;
    for line_number in start..=end {
        let line_text = lines[line_number as usize - 1];
        let mut rest = line_text;
        if rest.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        while let Some(open) = rest.find('[') {
            if let Some(close) = rest[open..]
                .strip_prefix("[[")
                .and_then(|after| after.find("]]"))
            {
                let raw = &rest[open + 2..open + 2 + close];
                links.push(SourceLink {
                    // `[[target|title]]` keeps only the target.
                    raw_target: raw.split('|').next().unwrap_or(raw).trim().to_string(),
                    kind: "wikilink".into(),
                    line: line_number,
                    line_text: line_text.to_string(),
                });
                rest = &rest[open + 2 + close + 2..];
                continue;
            }
            if let Some(text_end) = rest[open..].find("](") {
                let target_start = open + text_end + 2;
                if let Some(close) = rest[target_start..].find(')') {
                    links.push(SourceLink {
                        raw_target: rest[target_start..target_start + close].trim().to_string(),
                        kind: "markdown".into(),
                        line: line_number,
                        line_text: line_text.to_string(),
                    });
                    rest = &rest[target_start + close + 1..];
                    continue;
                }
            }
            rest = &rest[open + 1..];
        }
    }
    links
}
