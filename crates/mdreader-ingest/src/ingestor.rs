//! [`MarkdownIngestor`]: the incremental markdown pipeline behind
//! `catdb import`, run by the engine through its [`IngestContext`].
//!
//! Incrementality contract (the P2 acceptance bar):
//! * unchanged file (same blake3) → short-circuit, nothing touched;
//! * one edited section → exactly that section's chunks re-embedded, sibling
//!   sections keep their ids, chunks, and vectors (line-number shifts are
//!   row updates with unchanged content hashes, which storage honors by
//!   keeping chunks);
//! * renamed file (same blake3, new path) → path-only update, zero re-embeds;
//! * deleted file → document, sections, chunks, links, and vectors gone, with
//!   matching retraction operations in the log.
//!
//! Every semantic mutation is mirrored into the operation log as a
//! [`CatOperation`] with real provenance (source path, line range, content
//! hash), using the engine's qualified-name contract (`md.Document`,
//! `md.Section`, `md.Link`; arrows `md.document`, `md.sourceSection`,
//! `md.sourceDocument`) so the engine's instance-level equation check can
//! verify the md schema's path equations over the imported corpus.

use std::collections::{HashMap, HashSet, VecDeque};

use async_trait::async_trait;
use catdb_engine::{qualify, EngineResult, IngestContext, IngestRequest, IngestStats, Ingestor};
use catdb_ir::{CatOperation, Provenance};
use catdb_storage::{ChunkRow, DocumentRow, LinkRow, SectionRow};
use uuid::Uuid;

use crate::parse::{parse_markdown, ParsedDocument};
use crate::scan::{scan_root, ScanOptions, ScannedFile};

/// The markdown ingest pipeline. Stateless; safe to share.
#[derive(Debug, Default)]
pub struct MarkdownIngestor;

impl MarkdownIngestor {
    /// A new markdown ingestor.
    pub fn new() -> Self {
        Self
    }
}

fn blake3_file(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

/// Deterministic UUID from labelled parts (blake3-derived). Used for link
/// and morphism identities so re-ingesting is idempotent.
fn stable_uuid(parts: &[&str]) -> Uuid {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
    }
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    Uuid::from_bytes(bytes)
}

fn provenance(source: &str, range: Option<[u32; 2]>, content_hash: &str) -> Provenance {
    Provenance {
        source: Some(source.to_string()),
        range,
        content_hash: Some(content_hash.to_string()),
        schema_version: Some("md/v1".to_string()),
        extractor: Some(concat!("mdreader-ingest@", env!("CARGO_PKG_VERSION")).to_string()),
        confidence: Some(1.0),
        approved_by: None,
    }
}

fn file_stem(rel: &str) -> &str {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name)
}

fn dir_of(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
}

/// Normalize a relative reference against a base directory: resolves `.`
/// and `..`, strips a leading `/` (treated as root-relative), and
/// percent-decodes spaces.
fn normalize_target(base_dir: &str, target: &str) -> Option<String> {
    let decoded = target.replace("%20", " ");
    let (start, path) = if let Some(stripped) = decoded.strip_prefix('/') {
        ("", stripped.to_string())
    } else {
        (base_dir, decoded)
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
                // A pop past the top escapes the root: unresolvable.
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

#[async_trait]
impl Ingestor for MarkdownIngestor {
    fn name(&self) -> &str {
        "markdown"
    }

    async fn ingest(
        &self,
        ctx: &dyn IngestContext,
        req: &IngestRequest,
    ) -> EngineResult<IngestStats> {
        let scan_options = ScanOptions {
            include_mdx: req.options.include_mdx,
            max_file_size: req.source.max_file_size,
            respect_gitignore: req.source.respect_gitignore,
        };
        let files = scan_root(&req.root, &scan_options)?;

        let stored_before = ctx.list_documents()?;
        let stored_by_path: HashMap<String, DocumentRow> = stored_before
            .iter()
            .map(|d| (d.file_path.clone(), d.clone()))
            .collect();
        let scan_set: HashSet<&str> = files.iter().map(|f| f.rel.as_str()).collect();

        // Rename candidates: stored documents whose path vanished from the
        // scan, keyed by content hash (deterministic order by path).
        let mut rename_candidates: HashMap<String, VecDeque<DocumentRow>> = HashMap::new();
        for doc in &stored_before {
            if !scan_set.contains(doc.file_path.as_str()) {
                rename_candidates
                    .entry(doc.content_hash.clone())
                    .or_default()
                    .push_back(doc.clone());
            }
        }

        let mut stats = IngestStats::default();
        let mut seen: HashSet<Uuid> = HashSet::new();

        for file in &files {
            stats.files_scanned += 1;
            let bytes = std::fs::read(&file.abs)?;
            let content_hash = blake3_file(&bytes);
            match stored_by_path.get(&file.rel) {
                Some(doc) if doc.content_hash == content_hash => {
                    seen.insert(doc.id);
                    stats.files_skipped_unchanged += 1;
                }
                Some(doc) => {
                    seen.insert(doc.id);
                    let text = String::from_utf8_lossy(&bytes);
                    sync_document(ctx, doc.id, file, &text, &content_hash, &mut stats).await?;
                }
                None => {
                    let renamed = rename_candidates
                        .get_mut(&content_hash)
                        .and_then(|q| q.pop_front());
                    if let Some(old) = renamed {
                        seen.insert(old.id);
                        rename_document(ctx, &old, file, &content_hash).await?;
                        stats.files_renamed += 1;
                    } else {
                        let doc_id = Uuid::new_v4();
                        seen.insert(doc_id);
                        let text = String::from_utf8_lossy(&bytes);
                        sync_document(ctx, doc_id, file, &text, &content_hash, &mut stats).await?;
                    }
                }
            }
        }

        // Deletions: stored documents that no scanned file claimed, scoped to
        // repo identities this root can produce (a guard for shared
        // databases; see the crate docs for the v1 single-source caveat).
        let synthetic = req
            .root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "root".to_string());
        let repos_seen: HashSet<&str> = files.iter().map(|f| f.repo.as_str()).collect();
        for doc in &stored_before {
            if seen.contains(&doc.id) || scan_set.contains(doc.file_path.as_str()) {
                continue;
            }
            let in_scope = match &doc.repo {
                None => true,
                Some(repo) => repo == &synthetic || repos_seen.contains(repo.as_str()),
            };
            if !in_scope {
                continue;
            }
            delete_document(ctx, doc).await?;
            stats.files_deleted += 1;
        }

        stats.links_resolved = resolve_links(ctx).await?;
        Ok(stats)
    }
}

/// Synchronize one parsed file into storage, incrementally.
async fn sync_document(
    ctx: &dyn IngestContext,
    doc_id: Uuid,
    file: &ScannedFile,
    source_text: &str,
    content_hash: &str,
    stats: &mut IngestStats,
) -> EngineResult<()> {
    let parsed: ParsedDocument = parse_markdown(source_text, file_stem(&file.rel));
    let stored_sections = ctx.sections_for_document(doc_id)?;

    // Pass 1 — claim stored sections by content hash (unchanged content;
    // possibly shifted lines). Pass 2 — claim by heading path (edited in
    // place). Leftover parsed sections are new; leftover stored are stale.
    let mut by_hash: HashMap<&str, VecDeque<&SectionRow>> = HashMap::new();
    for row in &stored_sections {
        by_hash
            .entry(row.content_hash.as_str())
            .or_default()
            .push_back(row);
    }
    let mut claimed: Vec<Option<(Uuid, bool)>> = vec![None; parsed.sections.len()]; // (id, changed)
    let mut taken: HashSet<Uuid> = HashSet::new();
    for (i, section) in parsed.sections.iter().enumerate() {
        if let Some(row) = by_hash
            .get_mut(section.content_hash.as_str())
            .and_then(|q| q.pop_front())
        {
            claimed[i] = Some((row.id, false));
            taken.insert(row.id);
        }
    }
    let mut by_heading: HashMap<&[String], VecDeque<&SectionRow>> = HashMap::new();
    for row in &stored_sections {
        if !taken.contains(&row.id) {
            by_heading
                .entry(row.heading_path.as_slice())
                .or_default()
                .push_back(row);
        }
    }
    for (i, section) in parsed.sections.iter().enumerate() {
        if claimed[i].is_none() {
            if let Some(row) = by_heading
                .get_mut(section.heading_path.as_slice())
                .and_then(|q| q.pop_front())
            {
                claimed[i] = Some((row.id, true));
                taken.insert(row.id);
            }
        }
    }
    for slot in claimed.iter_mut() {
        if slot.is_none() {
            *slot = Some((Uuid::new_v4(), true));
        }
    }
    let stale: Vec<&SectionRow> = stored_sections
        .iter()
        .filter(|row| !taken.contains(&row.id))
        .collect();

    // Document row + operation first (sections reference it).
    ctx.upsert_documents(vec![DocumentRow {
        id: doc_id,
        file_path: file.rel.clone(),
        repo: Some(file.repo.clone()),
        content_hash: content_hash.to_string(),
    }])
    .await?;
    ctx.emit(CatOperation::AssertObject {
        entity: qualify("md", "Document"),
        object_id: doc_id,
        value: serde_json::json!({
            "file_path": file.rel,
            "title": parsed.title,
            "repo": file.repo,
        }),
        provenance: provenance(&file.rel, None, content_hash),
    })
    .await?;

    // Section rows in document order (parents resolve through the claim map).
    let section_rows: Vec<SectionRow> = parsed
        .sections
        .iter()
        .enumerate()
        .map(|(i, section)| SectionRow {
            id: claimed[i].expect("claimed above").0,
            document_id: doc_id,
            heading_path: section.heading_path.clone(),
            start_line: section.start_line,
            end_line: section.end_line,
            content_hash: section.content_hash.clone(),
            parent_id: section
                .parent
                .and_then(|p| claimed.get(p).copied().flatten())
                .map(|(id, _)| id),
        })
        .collect();
    if !section_rows.is_empty() {
        ctx.upsert_sections(section_rows.clone()).await?;
    }

    // Stale sections: retract their facts, then cascade-delete. Deletion runs
    // AFTER the upsert so claimed children have already been re-parented and
    // cannot be swept by the cascade.
    if !stale.is_empty() {
        let stale_ids: Vec<Uuid> = stale.iter().map(|row| row.id).collect();
        retract_sections(ctx, &file.rel, &stale_ids).await?;
        ctx.delete_stale_sections(stale_ids).await?;
    }

    // Changed (or new) sections: rebuild chunks, links, and facts. Unchanged
    // sections keep their chunks and links (storage keeps chunks when the
    // upserted hash is unchanged).
    for (i, section) in parsed.sections.iter().enumerate() {
        let (section_id, changed) = claimed[i].expect("claimed above");
        if !changed {
            continue;
        }
        stats.sections_upserted += 1;
        let range = Some([section.start_line, section.end_line]);

        ctx.emit(CatOperation::AssertObject {
            entity: qualify("md", "Section"),
            object_id: section_id,
            value: serde_json::json!({
                "heading_path": section.heading_path,
                "start_line": section.start_line,
                "end_line": section.end_line,
            }),
            provenance: provenance(&file.rel, range, &section.content_hash),
        })
        .await?;
        ctx.emit(CatOperation::AssertMorphism {
            arrow: qualify("md", "document"),
            morphism_id: stable_uuid(&["m", "md.document", &section_id.to_string()]),
            source: section_id,
            target: doc_id,
            provenance: provenance(&file.rel, range, &section.content_hash),
        })
        .await?;

        let chunk_rows: Vec<ChunkRow> = section
            .chunks
            .iter()
            .map(|chunk| ChunkRow {
                id: Uuid::new_v4(),
                section_id,
                document_id: doc_id,
                file_path: file.rel.clone(),
                heading_path: section.heading_path.clone(),
                part: chunk.part,
                text: chunk.text.clone(),
                embedding_text: chunk.embedding_text.clone(),
                content_hash: chunk.content_hash.clone(),
                token_count: chunk.token_count,
            })
            .collect();
        for chunk in &chunk_rows {
            ctx.schedule_embedding(chunk.id, chunk.embedding_text.clone());
        }
        if !chunk_rows.is_empty() {
            ctx.upsert_chunks(chunk_rows).await?;
        }

        // Links: deterministic ids keyed by (section, ordinal, target), so
        // re-ingest is idempotent and vanished links are diffed away.
        let mut new_links = Vec::new();
        for (ordinal, link) in section.links.iter().enumerate() {
            let link_id = stable_uuid(&[
                "link",
                &section_id.to_string(),
                &ordinal.to_string(),
                &link.kind,
                &link.raw_target,
            ]);
            new_links.push(LinkRow {
                id: link_id,
                source_section: section_id,
                raw_target: link.raw_target.clone(),
                kind: link.kind.clone(),
                resolved_document: None,
            });
            ctx.emit(CatOperation::AssertObject {
                entity: qualify("md", "Link"),
                object_id: link_id,
                value: serde_json::json!({
                    "raw_target": link.raw_target,
                    "kind": link.kind,
                }),
                provenance: provenance(&file.rel, Some([link.line, link.line]), ""),
            })
            .await?;
            ctx.emit(CatOperation::AssertMorphism {
                arrow: qualify("md", "sourceSection"),
                morphism_id: stable_uuid(&["m", "md.sourceSection", &link_id.to_string()]),
                source: link_id,
                target: section_id,
                provenance: provenance(&file.rel, Some([link.line, link.line]), ""),
            })
            .await?;
            ctx.emit(CatOperation::AssertMorphism {
                arrow: qualify("md", "sourceDocument"),
                morphism_id: stable_uuid(&["m", "md.sourceDocument", &link_id.to_string()]),
                source: link_id,
                target: doc_id,
                provenance: provenance(&file.rel, Some([link.line, link.line]), ""),
            })
            .await?;
        }
        let new_ids: HashSet<Uuid> = new_links.iter().map(|l| l.id).collect();
        let existing = ctx.links_for_sections(&[section_id])?;
        let stale_links: Vec<Uuid> = existing
            .iter()
            .filter(|l| !new_ids.contains(&l.id))
            .map(|l| l.id)
            .collect();
        if !stale_links.is_empty() {
            retract_links(ctx, &stale_links).await?;
            ctx.delete_links(&stale_links)?;
        }
        // Keep already-resolved targets for links that survived unchanged.
        let existing_resolved: HashMap<Uuid, Option<Uuid>> = existing
            .iter()
            .map(|l| (l.id, l.resolved_document))
            .collect();
        for link in &mut new_links {
            if let Some(resolved) = existing_resolved.get(&link.id) {
                link.resolved_document = *resolved;
            }
        }
        if !new_links.is_empty() {
            ctx.upsert_links(new_links).await?;
        }
    }
    Ok(())
}

/// Rename: same content, new path — a path-only update, zero re-embeds.
async fn rename_document(
    ctx: &dyn IngestContext,
    old: &DocumentRow,
    file: &ScannedFile,
    content_hash: &str,
) -> EngineResult<()> {
    ctx.upsert_documents(vec![DocumentRow {
        id: old.id,
        file_path: file.rel.clone(),
        repo: Some(file.repo.clone()),
        content_hash: content_hash.to_string(),
    }])
    .await?;
    // Chunks denormalize the path for display/filtering: rewrite in place.
    // Ids are unchanged, so stored vectors stay valid — no re-embeds.
    let mut chunks = ctx.chunks_for_document(old.id)?;
    for chunk in &mut chunks {
        chunk.file_path = file.rel.clone();
    }
    if !chunks.is_empty() {
        ctx.upsert_chunks(chunks).await?;
    }
    ctx.emit(CatOperation::AssertObject {
        entity: qualify("md", "Document"),
        object_id: old.id,
        value: serde_json::json!({
            "file_path": file.rel,
            "repo": file.repo,
            "renamed_from": old.file_path,
        }),
        provenance: provenance(&file.rel, None, content_hash),
    })
    .await?;
    Ok(())
}

/// Delete a vanished document: retract every fact, then cascade-delete.
async fn delete_document(ctx: &dyn IngestContext, doc: &DocumentRow) -> EngineResult<()> {
    let sections = ctx.sections_for_document(doc.id)?;
    let section_ids: Vec<Uuid> = sections.iter().map(|s| s.id).collect();
    retract_sections(ctx, &doc.file_path, &section_ids).await?;
    ctx.emit(CatOperation::RetractObject { object_id: doc.id })
        .await?;
    ctx.delete_stale_documents(vec![doc.id]).await?;
    Ok(())
}

/// Retract the operation-log facts for sections (their links included).
async fn retract_sections(
    ctx: &dyn IngestContext,
    _source: &str,
    section_ids: &[Uuid],
) -> EngineResult<()> {
    if section_ids.is_empty() {
        return Ok(());
    }
    let links = ctx.links_for_sections(section_ids)?;
    let link_ids: Vec<Uuid> = links.iter().map(|l| l.id).collect();
    retract_links(ctx, &link_ids).await?;
    for section_id in section_ids {
        ctx.emit(CatOperation::RetractMorphism {
            morphism_id: stable_uuid(&["m", "md.document", &section_id.to_string()]),
        })
        .await?;
        ctx.emit(CatOperation::RetractObject {
            object_id: *section_id,
        })
        .await?;
    }
    Ok(())
}

async fn retract_links(ctx: &dyn IngestContext, link_ids: &[Uuid]) -> EngineResult<()> {
    for link_id in link_ids {
        ctx.emit(CatOperation::RetractMorphism {
            morphism_id: stable_uuid(&["m", "md.sourceSection", &link_id.to_string()]),
        })
        .await?;
        ctx.emit(CatOperation::RetractMorphism {
            morphism_id: stable_uuid(&["m", "md.sourceDocument", &link_id.to_string()]),
        })
        .await?;
        ctx.emit(CatOperation::RetractObject {
            object_id: *link_id,
        })
        .await?;
    }
    Ok(())
}

/// Resolve every unresolved link against the current corpus: relative paths
/// (with `.md`/`.markdown` completion) for markdown/image links, stem lookup
/// for wikilinks. Returns how many links were resolved in this pass.
async fn resolve_links(ctx: &dyn IngestContext) -> EngineResult<u64> {
    let docs = ctx.list_documents()?;
    let path_map: HashMap<&str, Uuid> = docs.iter().map(|d| (d.file_path.as_str(), d.id)).collect();
    let mut stem_map: HashMap<String, Vec<(&str, Uuid)>> = HashMap::new();
    for doc in &docs {
        stem_map
            .entry(file_stem(&doc.file_path).to_lowercase())
            .or_default()
            .push((doc.file_path.as_str(), doc.id));
    }
    for candidates in stem_map.values_mut() {
        candidates.sort();
    }

    let mut resolved_count = 0u64;
    for entry in ctx.unresolved_links()? {
        let target = resolve_target(
            &entry.link.kind,
            &entry.link.raw_target,
            &entry.source_path,
            &path_map,
            &stem_map,
        );
        let Some(target_id) = target else { continue };
        let mut link = entry.link.clone();
        link.resolved_document = Some(target_id);
        ctx.upsert_links(vec![link]).await?;
        ctx.emit(CatOperation::AssertObject {
            entity: qualify("md", "Link"),
            object_id: entry.link.id,
            value: serde_json::json!({
                "raw_target": entry.link.raw_target,
                "kind": entry.link.kind,
                "target_ref": target_id,
            }),
            provenance: provenance(&entry.source_path, None, ""),
        })
        .await?;
        resolved_count += 1;
    }
    Ok(resolved_count)
}

fn resolve_target(
    kind: &str,
    raw_target: &str,
    source_path: &str,
    path_map: &HashMap<&str, Uuid>,
    stem_map: &HashMap<String, Vec<(&str, Uuid)>>,
) -> Option<Uuid> {
    let target = raw_target.split(['#', '?']).next().unwrap_or("").trim();
    if target.is_empty() {
        return None; // pure anchor (or empty) target
    }
    if target.contains("://") || target.starts_with("mailto:") {
        return None; // external
    }
    if kind == "wikilink" {
        // Vault-wide stem lookup; a path-ish wikilink also tries the path map.
        if target.contains('/') {
            for candidate in [
                target.to_string(),
                format!("{target}.md"),
                format!("{target}.markdown"),
            ] {
                if let Some(id) = path_map.get(candidate.as_str()) {
                    return Some(*id);
                }
            }
        }
        return stem_map
            .get(&target.to_lowercase())
            .and_then(|candidates| candidates.first())
            .map(|(_, id)| *id);
    }
    // markdown / image: resolve relative to the source file's directory.
    let normalized = normalize_target(dir_of(source_path), target)?;
    if let Some(id) = path_map.get(normalized.as_str()) {
        return Some(*id);
    }
    let has_extension = normalized
        .rsplit('/')
        .next()
        .is_some_and(|n| n.contains('.'));
    if !has_extension {
        for candidate in [format!("{normalized}.md"), format!("{normalized}.markdown")] {
            if let Some(id) = path_map.get(candidate.as_str()) {
                return Some(*id);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_uuid_is_deterministic_and_labelled() {
        let a = stable_uuid(&["link", "s", "0", "markdown", "x.md"]);
        let b = stable_uuid(&["link", "s", "0", "markdown", "x.md"]);
        let c = stable_uuid(&["link", "s", "1", "markdown", "x.md"]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn normalize_target_resolves_dots_and_root() {
        assert_eq!(
            normalize_target("notes/daily", "../projects/x.md"),
            Some("notes/projects/x.md".to_string())
        );
        assert_eq!(normalize_target("", "./a/b.md"), Some("a/b.md".to_string()));
        assert_eq!(
            normalize_target("docs", "/index.md"),
            Some("index.md".to_string())
        );
        assert_eq!(normalize_target("", "../escapes.md"), None);
    }

    #[test]
    fn resolve_target_handles_kinds_anchors_and_externals() {
        let d1 = Uuid::from_u128(1);
        let d2 = Uuid::from_u128(2);
        let mut path_map = HashMap::new();
        path_map.insert("notes/zettel/node-storage.md", d1);
        path_map.insert("index.md", d2);
        let mut stem_map: HashMap<String, Vec<(&str, Uuid)>> = HashMap::new();
        stem_map.insert(
            "node-storage".to_string(),
            vec![("notes/zettel/node-storage.md", d1)],
        );

        // Wikilink by stem, case-insensitive, fragment stripped.
        assert_eq!(
            resolve_target(
                "wikilink",
                "Node-Storage#Section",
                "notes/zettel/other.md",
                &path_map,
                &stem_map
            ),
            Some(d1)
        );
        // Relative markdown link with extension completion.
        assert_eq!(
            resolve_target(
                "markdown",
                "node-storage",
                "notes/zettel/wikilinks.md",
                &path_map,
                &stem_map
            ),
            Some(d1), // "notes/zettel/node-storage" completes to ".md"
        );
        assert_eq!(
            resolve_target(
                "markdown",
                "node-storage.md",
                "notes/zettel/wikilinks.md",
                &path_map,
                &stem_map
            ),
            Some(d1)
        );
        // Root-escaping and external targets stay unresolved.
        assert_eq!(
            resolve_target(
                "markdown",
                "https://example.com/x.md",
                "a.md",
                &path_map,
                &stem_map
            ),
            None
        );
        assert_eq!(
            resolve_target("markdown", "#anchor-only", "index.md", &path_map, &stem_map),
            None
        );
    }
}
