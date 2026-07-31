//! Relevance smoke suite: the YAML cases in `fixtures/relevance.yaml` run
//! against `fixtures/corpus`, indexed by a minimal, test-only markdown
//! ingestor (the platform cannot depend on the application's ingest crate —
//! the boundary rule — so this file carries just enough parsing to exercise
//! search: sections, one chunk per section, frontmatter tags as concept
//! joins, and wikilink/markdown links resolved into `kn.ArtifactLink`
//! spans).
//!
//! The default run uses the deterministic [`HashEmbedder`], making the suite
//! CI-stable: lexical-dominant cases assert strictly, `semantic: true` cases
//! assert loose top-k membership. The ignored real-model variant re-runs
//! every case with a real embedding model:
//!
//! ```text
//! cargo test -p catdb-engine --features real-embed --test relevance -- --ignored
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use catdb_sdk::embed::{Embedder, HashEmbedder};
use catdb_sdk::engine::{
    qualify, DomainRegistration, EmbeddedCatDb, EngineConfig, EngineError, ImportOptions,
    IngestContext, IngestRequest, IngestStats, Ingestor, SearchOptions,
};
use catdb_sdk::ir::{CatOperation, Provenance};
use catdb_sdk::storage::{ChunkRow, DocumentRow, LinkRow, SectionRow};
use serde::Deserialize;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Suite definition
// ---------------------------------------------------------------------------

fn default_top_k() -> usize {
    5
}

#[derive(Debug, Deserialize)]
struct Suite {
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    query: String,
    expect_path: String,
    #[serde(default = "default_top_k")]
    top_k: usize,
    #[serde(default)]
    semantic: bool,
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn load_suite() -> Suite {
    let text = std::fs::read_to_string(fixtures_root().join("relevance.yaml"))
        .expect("fixtures/relevance.yaml");
    serde_yaml::from_str(&text).expect("relevance.yaml parses")
}

// ---------------------------------------------------------------------------
// Minimal markdown ingestor (test-only; see the module docs)
// ---------------------------------------------------------------------------

/// Deterministic test uuid from labelled parts (stable across runs:
/// `DefaultHasher::new` is keyed with constants).
fn tid(parts: &[&str]) -> Uuid {
    use std::hash::{Hash, Hasher};
    let mut h1 = std::collections::hash_map::DefaultHasher::new();
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    "left".hash(&mut h1);
    "right".hash(&mut h2);
    for p in parts {
        p.hash(&mut h1);
        p.hash(&mut h2);
    }
    Uuid::from_u64_pair(h1.finish(), h2.finish())
}

struct ParsedSection {
    heading: Option<String>,
    start_line: u32,
    end_line: u32,
    text: String,
    links: Vec<(String, String)>, // (kind, raw target)
}

struct ParsedFile {
    rel: String,
    tags: Vec<String>,
    sections: Vec<ParsedSection>,
}

fn parse_frontmatter_tags(lines: &[&str]) -> (usize, Vec<String>) {
    if lines.first().map(|l| l.trim()) != Some("---") {
        return (0, Vec::new());
    }
    let Some(end) = lines[1..].iter().position(|l| l.trim() == "---") else {
        return (0, Vec::new());
    };
    let body = &lines[1..=end];
    let mut tags = Vec::new();
    let mut i = 0;
    while i < body.len() {
        let line = body[i].trim();
        if let Some(rest) = line.strip_prefix("tags:") {
            let rest = rest.trim();
            if let Some(inner) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                for part in inner.split(',') {
                    let tag = part.trim().trim_matches('"').trim_matches('\'');
                    if !tag.is_empty() {
                        tags.push(tag.to_lowercase());
                    }
                }
            } else if rest.is_empty() {
                // Block list on the following lines.
                let mut j = i + 1;
                while j < body.len() {
                    let item = body[j].trim();
                    let Some(tag) = item.strip_prefix("- ") else {
                        break;
                    };
                    let tag = tag.trim().trim_matches('"').trim_matches('\'');
                    if !tag.is_empty() {
                        tags.push(tag.to_lowercase());
                    }
                    j += 1;
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    (end + 2, tags)
}

/// Extract wikilink (`[[target]]`) and markdown (`[text](target)`) targets.
fn extract_links(text: &str) -> Vec<(String, String)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            if let Some(end) = text[i + 2..].find("]]") {
                let target = text[i + 2..i + 2 + end].split('|').next().unwrap_or("");
                if !target.is_empty() {
                    out.push(("wikilink".to_string(), target.trim().to_string()));
                }
                i += 2 + end + 2;
                continue;
            }
        }
        if bytes[i] == b'[' {
            if let Some(close) = text[i..].find("](") {
                let after = i + close + 2;
                if let Some(end) = text[after..].find(')') {
                    let target = text[after..after + end].trim();
                    if !target.is_empty() {
                        out.push(("markdown".to_string(), target.to_string()));
                    }
                    i = after + end + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

fn parse_file(rel: &str, content: &str) -> ParsedFile {
    let content = content.replace("\r\n", "\n");
    let lines: Vec<&str> = content.lines().collect();
    let (body_start, tags) = parse_frontmatter_tags(&lines);
    let mut sections: Vec<ParsedSection> = Vec::new();
    let mut current: Option<ParsedSection> = None;
    for (idx, line) in lines.iter().enumerate().skip(body_start) {
        let line_no = idx as u32 + 1;
        let trimmed = line.trim_start();
        let is_heading =
            trimmed.starts_with('#') && trimmed.trim_start_matches('#').starts_with([' ', '\t']);
        if is_heading {
            if let Some(mut section) = current.take() {
                section.end_line = line_no.saturating_sub(1);
                sections.push(section);
            }
            current = Some(ParsedSection {
                heading: Some(trimmed.trim_start_matches('#').trim().to_string()),
                start_line: line_no,
                end_line: line_no,
                text: String::new(),
                links: Vec::new(),
            });
            continue;
        }
        let section = current.get_or_insert_with(|| ParsedSection {
            heading: None,
            start_line: line_no,
            end_line: line_no,
            text: String::new(),
            links: Vec::new(),
        });
        section.text.push_str(line);
        section.text.push('\n');
        section.end_line = line_no;
    }
    if let Some(section) = current.take() {
        sections.push(section);
    }
    let mut sections: Vec<ParsedSection> = sections
        .into_iter()
        .filter(|s| s.heading.is_some() || !s.text.trim().is_empty())
        .collect();
    for section in &mut sections {
        section.links = extract_links(&section.text);
    }
    ParsedFile {
        rel: rel.to_string(),
        tags,
        sections,
    }
}

fn scan_markdown(root: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("md") | Some("markdown")
            ) {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if let Ok(content) = std::fs::read_to_string(&path) {
                    files.push((rel, content));
                }
            }
        }
    }
    files.sort();
    files
}

fn normalize_relative(base_dir: &str, target: &str) -> Option<String> {
    let decoded = target.replace("%20", " ");
    let (start, path) = match decoded.strip_prefix('/') {
        Some(stripped) => ("", stripped.to_string()),
        None => (base_dir, decoded),
    };
    let mut parts: Vec<&str> = if start.is_empty() {
        Vec::new()
    } else {
        start.split('/').collect()
    };
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

struct MiniMarkdownIngestor;

#[async_trait]
impl Ingestor for MiniMarkdownIngestor {
    fn name(&self) -> &str {
        "mini-markdown"
    }

    async fn ingest(
        &self,
        ctx: &dyn IngestContext,
        req: &IngestRequest,
    ) -> Result<IngestStats, EngineError> {
        let files = scan_markdown(&req.root);
        let parsed: Vec<ParsedFile> = files
            .iter()
            .map(|(rel, content)| parse_file(rel, content))
            .collect();

        let doc_id = |rel: &str| tid(&["doc", rel]);
        let path_map: HashMap<String, Uuid> = parsed
            .iter()
            .map(|f| (f.rel.clone(), doc_id(&f.rel)))
            .collect();
        let mut stem_map: HashMap<String, Uuid> = HashMap::new();
        for f in &parsed {
            let stem = f
                .rel
                .rsplit('/')
                .next()
                .unwrap_or(&f.rel)
                .trim_end_matches(".markdown")
                .trim_end_matches(".md")
                .to_lowercase();
            stem_map.entry(stem).or_insert_with(|| doc_id(&f.rel));
        }

        let mut stats = IngestStats::default();
        // Phase 1: every document row first, so cross-document link
        // resolutions in phase 2 never hit a missing FK target.
        ctx.upsert_documents(
            parsed
                .iter()
                .map(|file| DocumentRow {
                    id: doc_id(&file.rel),
                    source_id: Some(req.source.source_id),
                    file_path: file.rel.clone(),
                    repo: Some("corpus".to_string()),
                    content_hash: format!("mini:{}", file.rel),
                })
                .collect(),
        )
        .await?;
        for file in &parsed {
            let doc = doc_id(&file.rel);
            let artifact = tid(&["artifact", &file.rel]);
            ctx.emit(CatOperation::AssertObject {
                entity: qualify("kn", "Artifact"),
                object_id: artifact,
                value: serde_json::json!({
                    "uri": file.rel,
                    "artifact_kind": "markdown",
                    "artifact_content_hash": format!("mini:{}", file.rel),
                    "doc_ref": doc,
                }),
                provenance: Provenance::default(),
            })
            .await?;
            for tag in &file.tags {
                let concept = tid(&["concept", tag]);
                let ac = tid(&["ac", &file.rel, tag]);
                ctx.emit(CatOperation::AssertObject {
                    entity: qualify("kn", "Concept"),
                    object_id: concept,
                    value: serde_json::json!({ "concept_name": tag }),
                    provenance: Provenance::default(),
                })
                .await?;
                ctx.emit(CatOperation::AssertMorphism {
                    arrow: qualify("kn", "artifact"),
                    morphism_id: tid(&["m-artifact", &file.rel, tag]),
                    source: ac,
                    target: artifact,
                    provenance: Provenance::default(),
                })
                .await?;
                ctx.emit(CatOperation::AssertMorphism {
                    arrow: qualify("kn", "concept"),
                    morphism_id: tid(&["m-concept", &file.rel, tag]),
                    source: ac,
                    target: concept,
                    provenance: Provenance::default(),
                })
                .await?;
            }

            let dir = file.rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            for (si, section) in file.sections.iter().enumerate() {
                let section_id = tid(&["section", &file.rel, &si.to_string()]);
                ctx.upsert_sections(vec![SectionRow {
                    id: section_id,
                    document_id: doc,
                    heading_path: section.heading.iter().cloned().collect(),
                    start_line: section.start_line,
                    end_line: section.end_line,
                    content_hash: format!("mini-sec:{}:{si}", file.rel),
                    parent_id: None,
                }])
                .await?;
                stats.sections_upserted += 1;

                let mut text: String = section.text.chars().take(4000).collect();
                if let Some(h) = &section.heading {
                    text = format!("# {h}\n{text}");
                }
                let chunk_id = tid(&["chunk", &file.rel, &si.to_string()]);
                let embedding_text: String = text.chars().take(1000).collect();
                ctx.schedule_embedding(chunk_id, embedding_text.clone());
                ctx.upsert_chunks(vec![ChunkRow {
                    id: chunk_id,
                    section_id,
                    document_id: doc,
                    file_path: file.rel.clone(),
                    heading_path: section.heading.iter().cloned().collect(),
                    part: 0,
                    text: text.clone(),
                    embedding_text,
                    content_hash: format!("mini-chunk:{}:{si}", file.rel),
                    token_count: text.split_whitespace().count() as u32,
                }])
                .await?;

                // Resolve this section's links and assert the spans.
                for (li, (kind, raw)) in section.links.iter().enumerate() {
                    let target = raw.split(['#', '?']).next().unwrap_or("").trim();
                    if target.is_empty() || target.contains("://") || target.starts_with("mailto:")
                    {
                        continue;
                    }
                    let resolved: Option<Uuid> = if kind == "wikilink" {
                        stem_map.get(&target.to_lowercase()).copied()
                    } else {
                        normalize_relative(dir, target).and_then(|n| {
                            path_map.get(&n).copied().or_else(|| {
                                path_map
                                    .get(&format!("{n}.md"))
                                    .or_else(|| path_map.get(&format!("{n}.markdown")))
                                    .copied()
                            })
                        })
                    };
                    let Some(target_doc) = resolved else {
                        continue;
                    };
                    let link_id = tid(&["link", &file.rel, &si.to_string(), &li.to_string()]);
                    ctx.upsert_links(vec![LinkRow {
                        id: link_id,
                        source_section: section_id,
                        raw_target: raw.clone(),
                        kind: kind.clone(),
                        resolved_document: Some(target_doc),
                    }])
                    .await?;
                    stats.links_resolved += 1;
                    let span = tid(&["span", &link_id.to_string()]);
                    let target_rel = parsed
                        .iter()
                        .find(|f| doc_id(&f.rel) == target_doc)
                        .map(|f| f.rel.clone())
                        .unwrap_or_default();
                    let target_artifact = tid(&["artifact", &target_rel]);
                    ctx.emit(CatOperation::AssertObject {
                        entity: qualify("kn", "ArtifactLink"),
                        object_id: span,
                        value: serde_json::json!({
                            "link_ref": link_id,
                            "from_ref": artifact,
                            "to_ref": target_artifact,
                        }),
                        provenance: Provenance::default(),
                    })
                    .await?;
                    ctx.emit(CatOperation::AssertMorphism {
                        arrow: qualify("kn", "from"),
                        morphism_id: tid(&["m-from", &span.to_string()]),
                        source: span,
                        target: artifact,
                        provenance: Provenance::default(),
                    })
                    .await?;
                    ctx.emit(CatOperation::AssertMorphism {
                        arrow: qualify("kn", "to"),
                        morphism_id: tid(&["m-to", &span.to_string()]),
                        source: span,
                        target: target_artifact,
                        provenance: Provenance::default(),
                    })
                    .await?;
                }
            }
            stats.files_scanned += 1;
        }
        Ok(stats)
    }
}

fn corpus_domain() -> DomainRegistration {
    use catdb_sdk::core::builtin;
    DomainRegistration {
        name: "relevance-corpus".to_string(),
        schemas: vec![
            builtin::fs_schema(),
            builtin::md_schema(),
            builtin::kn_schema(),
            builtin::rx_schema(),
        ],
        approval_required_schemas: Vec::new(),
        mappings: Vec::new(),
        ingestors: vec![Arc::new(MiniMarkdownIngestor)],
        chunker_version: "mini-markdown@1".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Runners
// ---------------------------------------------------------------------------

async fn run_suite(embedder: Arc<dyn Embedder>, include_semantic: bool) {
    let engine =
        EmbeddedCatDb::open(EngineConfig::in_memory(embedder).with_domain(corpus_domain()))
            .unwrap();
    engine
        .import(fixtures_root().join("corpus"), ImportOptions::default())
        .await
        .unwrap();

    let suite = load_suite();
    assert!(suite.cases.len() >= 12, "the suite must stay ~12 cases");
    let mut failures: Vec<String> = Vec::new();
    for case in &suite.cases {
        if case.semantic && !include_semantic {
            continue;
        }
        let hits = match engine
            .search(
                &case.query,
                SearchOptions {
                    limit: case.top_k as u32,
                    ..SearchOptions::default()
                },
            )
            .await
        {
            Ok(hits) => hits,
            Err(err) => {
                failures.push(format!("{}: search failed: {err}", case.name));
                continue;
            }
        };
        let found = hits.iter().any(|h| h.file_path == case.expect_path);
        if !found {
            let got: Vec<&str> = hits.iter().map(|h| h.file_path.as_str()).collect();
            failures.push(format!(
                "{}: expected {} in top {}, got {:?}",
                case.name, case.expect_path, case.top_k, got
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "relevance failures:\n{}",
        failures.join("\n")
    );
}

/// The CI suite: deterministic HashEmbedder; lexical-dominant cases assert
/// strictly and `semantic: true` cases assert loose top-k membership (the
/// hash embedder is not a semantic model, but the loose bound plus the
/// lexical modality keeps them stable).
#[tokio::test]
async fn relevance_suite_hash_embedder() {
    run_suite(Arc::new(HashEmbedder::new()), true).await;
}

/// The real-model variant (ignored in CI: downloads/loads an ONNX model).
/// Run with:
/// `cargo test -p catdb-engine --features real-embed --test relevance -- --ignored`
#[cfg(feature = "real-embed")]
#[tokio::test]
#[ignore = "requires the local fastembed model"]
async fn relevance_suite_real_model() {
    let embedder = catdb_sdk::embed::FastembedEmbedder::load(None)
        .await
        .expect("fastembed model loads");
    run_suite(Arc::new(embedder), true).await;
}
