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
//! ## Failure atomicity
//!
//! Each file's sync is **one storage transaction**
//! ([`IngestContext::apply_batch`]): the pending-marker document row, every
//! retraction of a replaced fact, the section upserts, the stale-row
//! deletions, the new rows, and the new assertions all commit together or
//! not at all — the operation log and the row store can never desync across
//! a crash, and in particular a replaced chunk's `RetractObject` lands in
//! the same transaction that deletes its row (retract-with-delete, never
//! delete-then-maybe-retract).
//!
//! A document's stored `content_hash` may only advance once its sections,
//! chunks, AND embeddings are durably committed. The mechanism is a per-file
//! finalize step: [`sync_document`]'s batch writes the interim document row
//! with a `pending:`-prefixed hash (which can never equal a file's real
//! blake3) and stages the final row via
//! [`IngestContext::stage_document_finalize`]; the engine commits staged
//! rows only after the embedding flush succeeds. Any failure leaves the
//! pending marker in place, so the next import detects it and re-processes
//! the file in *force* mode (every section's chunks are rebuilt and
//! re-scheduled, repairing missing vectors). Chunk ids are deterministic
//! (`(section, part, content hash)`), so a repair pass overwrites rows
//! instead of duplicating them. Force mode also deletes the old chunk rows
//! a boundary change leaves unmatched (see [`StaleTarget::Chunks`]) — a
//! chunker rotation changes chunk ids without changing any section hash,
//! and those replaced rows must not linger searchable in FTS or the vector
//! tables.
//!
//! Every semantic mutation is mirrored into the operation log as a
//! [`CatOperation`] with real provenance (source path, line range, content
//! hash), using the engine's qualified-name contract (`md.Document`,
//! `md.Section`, `md.Link`, `rx.Chunk`; arrows `md.document`,
//! `md.sourceSection`, `md.sourceDocument`) so the engine's instance-level
//! equation check can verify the md schema's path equations over the
//! imported corpus.
//!
//! ## The KN layer: link morphisms and concepts
//!
//! On top of the md facts, each sync maintains the knowledge (`kn`) layer
//! of the builtin pipeline:
//!
//! * every document is asserted as a `kn.Artifact` (with `doc_ref` back to
//!   the `md.Document`, and its `uri` refreshed on rename);
//! * frontmatter `tags:` become `kn.Concept` objects (identity = the
//!   normalized, lowercased tag — shared across every document carrying
//!   it) joined through `kn.ArtifactConcept` rows with `kn.artifact` /
//!   `kn.concept` morphisms, diffed incrementally (a removed tag retracts
//!   its join row and morphisms; a concept no other artifact references is
//!   left in place — stable identity makes re-tagging idempotent);
//! * resolving a link additionally asserts a `kn.links_to` morphism
//!   between the source and target **documents** — the document graph edge
//!   `relation_search` traverses — retracted whenever the link dies or its
//!   target document is deleted (inbound links are looked up before the
//!   cascade nulls their resolutions).
//!
//! All of it rides the same per-file/per-resolution transactions as the
//! rows it derives from.

use std::collections::{HashMap, HashSet, VecDeque};

use async_trait::async_trait;
use catdb_engine::{
    qualify, EngineResult, IngestContext, IngestRequest, IngestStats, IngestWrite, Ingestor,
};
use catdb_ir::{CatOperation, Provenance};
use catdb_storage::{ChunkRow, DocumentRow, LinkRow, SectionRow, StaleTarget};
use uuid::Uuid;

use crate::parse::{parse_markdown_with_limit, ParsedDocument, CHUNK_TOKEN_LIMIT};
use crate::scan::{scan_root, ScanOptions, ScannedFile};

/// The markdown ingest pipeline. Stateless; safe to share.
#[derive(Debug)]
pub struct MarkdownIngestor {
    /// Token limit at which sections split into multiple chunks.
    chunk_token_limit: u32,
}

impl MarkdownIngestor {
    /// A new markdown ingestor splitting at the default
    /// [`CHUNK_TOKEN_LIMIT`].
    pub fn new() -> Self {
        Self {
            chunk_token_limit: CHUNK_TOKEN_LIMIT,
        }
    }

    /// A markdown ingestor splitting at an explicit token limit. A different
    /// limit produces different chunk *boundaries* for the same source —
    /// that is a chunker change, so the accompanying
    /// [`DomainRegistration`](catdb_engine::DomainRegistration) must carry a
    /// distinct `chunker_version` (the engine rotates the index version and
    /// forces a full re-chunk when it changes).
    pub fn with_chunk_token_limit(chunk_token_limit: u32) -> Self {
        Self { chunk_token_limit }
    }
}

impl Default for MarkdownIngestor {
    fn default() -> Self {
        Self::new()
    }
}

fn blake3_file(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

/// Marker prefix for a document row whose import has not finished (sections/
/// chunks may be committed but embeddings are not). A pending hash can never
/// equal a file's real `blake3:` hash, so the file is always re-processed —
/// and the prefix tells [`sync_document`] to run in force mode (rebuild every
/// section's chunks and re-schedule their embeddings) rather than trust the
/// per-section short-circuits.
const PENDING_PREFIX: &str = "pending:";

fn pending_hash(content_hash: &str) -> String {
    format!("{PENDING_PREFIX}{content_hash}")
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

/// `kn.Artifact` identity for a document (stable per document id).
fn artifact_id(doc_id: Uuid) -> Uuid {
    stable_uuid(&["kn-artifact", &doc_id.to_string()])
}

/// `kn.Concept` identity for a tag: global — every document tagged
/// `retrieval` shares one concept object.
fn concept_id(tag: &str) -> Uuid {
    stable_uuid(&["kn-concept", tag])
}

/// `kn.ArtifactConcept` join identity for (document, tag).
fn artifact_concept_id(doc_id: Uuid, tag: &str) -> Uuid {
    stable_uuid(&["kn-artifact-concept", &doc_id.to_string(), tag])
}

/// `kn.links_to` morphism identity, keyed by the LINK id: two parallel
/// links between the same documents keep independent morphisms, so a
/// link's death retracts exactly its own edge.
fn links_to_morphism_id(link_id: Uuid) -> Uuid {
    stable_uuid(&["m", "kn.links_to", &link_id.to_string()])
}

/// Extract normalized tags from tolerant frontmatter pairs: repeated
/// `(tags, item)` pairs (block lists), inline `[a, b]` arrays, and bare
/// comma strings all work; tags are trimmed, unquoted, `#`-stripped, and
/// lowercased (concept identity is case-insensitive); order preserved,
/// duplicates dropped.
fn frontmatter_tags(frontmatter: &[(String, String)]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for (key, value) in frontmatter {
        if !key.eq_ignore_ascii_case("tags") {
            continue;
        }
        let inner = value.trim();
        let inner = inner
            .strip_prefix('[')
            .and_then(|v| v.strip_suffix(']'))
            .unwrap_or(inner);
        for part in inner.split(',') {
            let tag = part
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .trim_start_matches('#')
                .trim();
            if tag.is_empty() {
                continue;
            }
            let tag = tag.to_lowercase();
            if seen.insert(tag.clone()) {
                out.push(tag);
            }
        }
    }
    out
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
        let files_total = files.len() as u64;

        for file in &files {
            stats.files_scanned += 1;
            let bytes = std::fs::read(&file.abs)?;
            let content_hash = blake3_file(&bytes);
            match stored_by_path.get(&file.rel) {
                Some(doc) if doc.content_hash == content_hash && !req.rebuild => {
                    seen.insert(doc.id);
                    stats.files_skipped_unchanged += 1;
                }
                Some(doc) => {
                    seen.insert(doc.id);
                    // A pending hash means a previous import died before its
                    // embeddings were committed: force-rebuild the file.
                    let force = req.rebuild || doc.content_hash.starts_with(PENDING_PREFIX);
                    let text = String::from_utf8_lossy(&bytes);
                    sync_document(
                        ctx,
                        doc.id,
                        file,
                        &text,
                        &content_hash,
                        force,
                        self.chunk_token_limit,
                        &mut stats,
                    )
                    .await?;
                }
                None => {
                    let renamed = rename_candidates
                        .get_mut(&content_hash)
                        .and_then(|q| q.pop_front());
                    if let Some(old) = renamed {
                        seen.insert(old.id);
                        if req.rebuild {
                            // A rebuild import must re-chunk/re-embed even
                            // renamed-but-unchanged content: run the full
                            // sync (which also updates the stored path)
                            // under the surviving document id.
                            let text = String::from_utf8_lossy(&bytes);
                            sync_document(
                                ctx,
                                old.id,
                                file,
                                &text,
                                &content_hash,
                                true,
                                self.chunk_token_limit,
                                &mut stats,
                            )
                            .await?;
                        } else {
                            rename_document(ctx, &old, file, &content_hash).await?;
                            stats.files_renamed += 1;
                        }
                    } else {
                        let doc_id = Uuid::new_v4();
                        seen.insert(doc_id);
                        let text = String::from_utf8_lossy(&bytes);
                        sync_document(
                            ctx,
                            doc_id,
                            file,
                            &text,
                            &content_hash,
                            false,
                            self.chunk_token_limit,
                            &mut stats,
                        )
                        .await?;
                    }
                }
            }
            // Live, pollable job progress (engine `index_status`); advisory,
            // so it costs one cheap row update per synced file.
            ctx.report_progress(stats.files_scanned, files_total)
                .await?;
        }

        // Deletions are scoped against STORED state under this source root:
        // stored paths are relative to the scanned root (v1's single-source
        // model, see the crate docs), so every stored document this scan did
        // not claim — including the contents of a fully deleted nested repo
        // directory — is gone from disk and must be cleaned up. Multi-source
        // scoping arrives when documents carry a source id.
        for doc in &stored_before {
            if seen.contains(&doc.id) || scan_set.contains(doc.file_path.as_str()) {
                continue;
            }
            delete_document(ctx, doc).await?;
            stats.files_deleted += 1;
        }

        stats.links_resolved = resolve_links(ctx).await?;
        Ok(stats)
    }
}

/// Synchronize one parsed file into storage, incrementally, as **one
/// transaction** (a single [`IngestContext::apply_batch`] call): rows and
/// operation-log facts commit together, so no crash can strand a retraction
/// without its row deletion or vice versa. With `force` (a rebuild import,
/// or a document left `pending:` by a failed import) every section is
/// processed even when its content hash is unchanged, so chunks are rebuilt
/// (idempotently, via deterministic ids), their embeddings re-scheduled, and
/// any old chunk row left unmatched by new boundaries deleted together with
/// the retraction of its facts.
#[allow(clippy::too_many_arguments)]
async fn sync_document(
    ctx: &dyn IngestContext,
    doc_id: Uuid,
    file: &ScannedFile,
    source_text: &str,
    content_hash: &str,
    force: bool,
    chunk_token_limit: u32,
    stats: &mut IngestStats,
) -> EngineResult<()> {
    let parsed: ParsedDocument =
        parse_markdown_with_limit(source_text, file_stem(&file.rel), chunk_token_limit);
    let stored_sections = ctx.sections_for_document(doc_id)?;
    // Prior chunks (read before any write can clear them), grouped by
    // section, so replaced or vanished chunks get retraction operations.
    let mut prior_chunks: HashMap<Uuid, Vec<ChunkRow>> = HashMap::new();
    for chunk in ctx.chunks_for_document(doc_id)? {
        prior_chunks
            .entry(chunk.section_id)
            .or_default()
            .push(chunk);
    }
    let source_lines: Vec<&str> = source_text.lines().collect();

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

    // Plan every section that will be (re)processed — changed content, or
    // `force` — computing its new chunk rows (deterministic ids) and which
    // prior chunk rows those leave unmatched. The plan is what lets the
    // batch below queue every RetractObject in the SAME transaction as the
    // row deletion that makes it true.
    struct SectionSync {
        index: usize,
        section_id: Uuid,
        chunk_rows: Vec<ChunkRow>,
        stale_chunk_ids: Vec<Uuid>,
    }
    let mut plans: Vec<SectionSync> = Vec::new();
    for (i, section) in parsed.sections.iter().enumerate() {
        let (section_id, changed) = claimed[i].expect("claimed above");
        if !changed && !force {
            continue;
        }
        let chunk_rows: Vec<ChunkRow> = section
            .chunks
            .iter()
            .map(|chunk| ChunkRow {
                id: stable_uuid(&[
                    "chunk",
                    &section_id.to_string(),
                    &chunk.part.to_string(),
                    &chunk.content_hash,
                ]),
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
        let new_ids: HashSet<Uuid> = chunk_rows.iter().map(|c| c.id).collect();
        let stale_chunk_ids: Vec<Uuid> = prior_chunks
            .get(&section_id)
            .map(|old| {
                old.iter()
                    .filter(|c| !new_ids.contains(&c.id))
                    .map(|c| c.id)
                    .collect()
            })
            .unwrap_or_default();
        plans.push(SectionSync {
            index: i,
            section_id,
            chunk_rows,
            stale_chunk_ids,
        });
    }

    // The file's entire sync is ONE batch = one storage transaction.
    let mut writes: Vec<IngestWrite> = Vec::new();
    let mut embeds: Vec<(Uuid, String)> = Vec::new();

    // Document row + operation first (sections reference it). The interim
    // row carries a pending hash; the real hash is staged and committed by
    // the engine only after every scheduled embedding is durably stored, so
    // a failed import leaves this file re-processable (see the module docs).
    writes.push(IngestWrite::UpsertDocuments(vec![DocumentRow {
        id: doc_id,
        file_path: file.rel.clone(),
        repo: Some(file.repo.clone()),
        content_hash: pending_hash(content_hash),
    }]));
    writes.push(IngestWrite::Emit(CatOperation::AssertObject {
        entity: qualify("md", "Document"),
        object_id: doc_id,
        value: serde_json::json!({
            "file_path": file.rel,
            "title": parsed.title,
            "repo": file.repo,
        }),
        provenance: provenance(&file.rel, None, content_hash),
    }));

    // KN layer: the document as a kn.Artifact, and its frontmatter tags as
    // kn.Concept / kn.ArtifactConcept facts, diffed against the previously
    // committed join morphisms — all in this same transaction.
    let artifact = artifact_id(doc_id);
    let kn_prov = provenance(&file.rel, None, content_hash);
    writes.push(IngestWrite::Emit(CatOperation::AssertObject {
        entity: qualify("kn", "Artifact"),
        object_id: artifact,
        value: serde_json::json!({
            "uri": file.rel,
            "artifact_kind": "markdown",
            "artifact_content_hash": content_hash,
            "doc_ref": doc_id,
        }),
        provenance: kn_prov.clone(),
    }));
    let tags = frontmatter_tags(&parsed.frontmatter);
    let new_acs: HashSet<Uuid> = tags
        .iter()
        .map(|tag| artifact_concept_id(doc_id, tag))
        .collect();
    let prior_acs = artifact_concepts(ctx, artifact)?;
    push_artifact_concept_retractions(
        &mut writes,
        prior_acs.into_iter().filter(|ac| !new_acs.contains(ac)),
    );
    for tag in &tags {
        let concept = concept_id(tag);
        let ac = artifact_concept_id(doc_id, tag);
        writes.push(IngestWrite::Emit(CatOperation::AssertObject {
            entity: qualify("kn", "Concept"),
            object_id: concept,
            value: serde_json::json!({ "concept_name": tag }),
            provenance: kn_prov.clone(),
        }));
        writes.push(IngestWrite::Emit(CatOperation::AssertObject {
            entity: qualify("kn", "ArtifactConcept"),
            object_id: ac,
            value: serde_json::json!({
                "artifact_ref": artifact,
                "concept_ref": concept,
            }),
            provenance: kn_prov.clone(),
        }));
        writes.push(IngestWrite::Emit(CatOperation::AssertMorphism {
            arrow: qualify("kn", "artifact"),
            morphism_id: stable_uuid(&["m", "kn.artifact", &ac.to_string()]),
            source: ac,
            target: artifact,
            provenance: kn_prov.clone(),
        }));
        writes.push(IngestWrite::Emit(CatOperation::AssertMorphism {
            arrow: qualify("kn", "concept"),
            morphism_id: stable_uuid(&["m", "kn.concept", &ac.to_string()]),
            source: ac,
            target: concept,
            provenance: kn_prov.clone(),
        }));
    }

    // Retractions for every fact this sync replaces or removes — queued
    // before (and committed with) the row writes that make them true:
    // (a) stale sections' chunks, links, and section facts;
    // (b) processed sections' replaced chunks (boundary changes included).
    let stale_ids: Vec<Uuid> = stale.iter().map(|row| row.id).collect();
    for stale_id in &stale_ids {
        if let Some(chunks) = prior_chunks.get(stale_id) {
            for chunk in chunks {
                writes.push(IngestWrite::Emit(CatOperation::RetractObject {
                    object_id: chunk.id,
                }));
            }
        }
    }
    push_section_retractions(ctx, &mut writes, &stale_ids)?;
    for plan in &plans {
        for stale_chunk in &plan.stale_chunk_ids {
            writes.push(IngestWrite::Emit(CatOperation::RetractObject {
                object_id: *stale_chunk,
            }));
        }
    }

    // Section rows in document order (parents resolve through the claim
    // map). Upserted BEFORE the stale-section deletion so claimed children
    // have already been re-parented and cannot be swept by the cascade.
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
        writes.push(IngestWrite::UpsertSections(section_rows));
    }
    if !stale_ids.is_empty() {
        writes.push(IngestWrite::DeleteStale(StaleTarget::Sections(stale_ids)));
    }

    // Changed (or new) sections: rebuild chunks, links, and facts. Unchanged
    // sections keep their chunks and links (storage keeps chunks when the
    // upserted hash is unchanged) — unless `force` demands a full rebuild.
    for plan in &plans {
        let section = &parsed.sections[plan.index];
        let section_id = plan.section_id;
        stats.sections_upserted += 1;
        let range = Some([section.start_line, section.end_line]);

        writes.push(IngestWrite::Emit(CatOperation::AssertObject {
            entity: qualify("md", "Section"),
            object_id: section_id,
            value: serde_json::json!({
                "heading_path": section.heading_path,
                "start_line": section.start_line,
                "end_line": section.end_line,
            }),
            provenance: provenance(&file.rel, range, &section.content_hash),
        }));
        writes.push(IngestWrite::Emit(CatOperation::AssertMorphism {
            arrow: qualify("md", "document"),
            morphism_id: stable_uuid(&["m", "md.document", &section_id.to_string()]),
            source: section_id,
            target: doc_id,
            provenance: provenance(&file.rel, range, &section.content_hash),
        }));

        // Replaced chunk ROWS go with their (already queued) retractions in
        // this same transaction. A changed section's rows were cleared by
        // the section upsert above (section-scoped replacement in storage);
        // this covers the force-rechunk case, where new boundaries change
        // chunk ids without any section-hash change — those old rows (and
        // their FTS entries and vectors) must not linger searchable.
        if !plan.stale_chunk_ids.is_empty() {
            writes.push(IngestWrite::DeleteStale(StaleTarget::Chunks(
                plan.stale_chunk_ids.clone(),
            )));
        }
        // Chunk ids are deterministic — (section, part, content hash) — so a
        // repair or rebuild pass overwrites the same rows instead of
        // duplicating them.
        for chunk in &plan.chunk_rows {
            embeds.push((chunk.id, chunk.embedding_text.clone()));
            writes.push(IngestWrite::Emit(CatOperation::AssertObject {
                entity: qualify("rx", "Chunk"),
                object_id: chunk.id,
                value: serde_json::json!({
                    "file_path": file.rel,
                    "section_ref": section_id,
                    "part": chunk.part,
                    "heading_path": section.heading_path,
                    "token_count": chunk.token_count,
                }),
                provenance: provenance(&file.rel, range, &chunk.content_hash),
            }));
        }
        if !plan.chunk_rows.is_empty() {
            writes.push(IngestWrite::UpsertChunks(plan.chunk_rows.clone()));
        }

        // Links: deterministic ids keyed by (section, ordinal, target), so
        // re-ingest is idempotent and vanished links are diffed away. Each
        // link fact carries real provenance: the line it sits on and the
        // blake3 of that line's text (falling back to the section hash if
        // the reported line is out of range).
        let mut new_links = Vec::new();
        for (ordinal, link) in section.links.iter().enumerate() {
            let link_id = stable_uuid(&[
                "link",
                &section_id.to_string(),
                &ordinal.to_string(),
                &link.kind,
                &link.raw_target,
            ]);
            let link_hash = source_lines
                .get(link.line.saturating_sub(1) as usize)
                .map(|line| blake3_file(line.as_bytes()))
                .unwrap_or_else(|| section.content_hash.clone());
            let link_provenance = provenance(&file.rel, Some([link.line, link.line]), &link_hash);
            new_links.push(LinkRow {
                id: link_id,
                source_section: section_id,
                raw_target: link.raw_target.clone(),
                kind: link.kind.clone(),
                resolved_document: None,
            });
            writes.push(IngestWrite::Emit(CatOperation::AssertObject {
                entity: qualify("md", "Link"),
                object_id: link_id,
                value: serde_json::json!({
                    "raw_target": link.raw_target,
                    "kind": link.kind,
                }),
                provenance: link_provenance.clone(),
            }));
            writes.push(IngestWrite::Emit(CatOperation::AssertMorphism {
                arrow: qualify("md", "sourceSection"),
                morphism_id: stable_uuid(&["m", "md.sourceSection", &link_id.to_string()]),
                source: link_id,
                target: section_id,
                provenance: link_provenance.clone(),
            }));
            writes.push(IngestWrite::Emit(CatOperation::AssertMorphism {
                arrow: qualify("md", "sourceDocument"),
                morphism_id: stable_uuid(&["m", "md.sourceDocument", &link_id.to_string()]),
                source: link_id,
                target: doc_id,
                provenance: link_provenance,
            }));
        }
        let new_ids: HashSet<Uuid> = new_links.iter().map(|l| l.id).collect();
        let existing = ctx.links_for_sections(&[section_id])?;
        let stale_links: Vec<Uuid> = existing
            .iter()
            .filter(|l| !new_ids.contains(&l.id))
            .map(|l| l.id)
            .collect();
        if !stale_links.is_empty() {
            push_link_retractions(&mut writes, &stale_links);
            writes.push(IngestWrite::DeleteLinks(stale_links));
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
            writes.push(IngestWrite::UpsertLinks(new_links));
        }
    }

    // Commit the whole file sync atomically, then hand the engine the
    // pieces that must wait for the embedding flush: the finalize row and
    // the embedding schedule.
    ctx.apply_batch(writes).await?;
    ctx.stage_document_finalize(DocumentRow {
        id: doc_id,
        file_path: file.rel.clone(),
        repo: Some(file.repo.clone()),
        content_hash: content_hash.to_string(),
    });
    for (chunk_id, embedding_text) in embeds {
        ctx.schedule_embedding(chunk_id, embedding_text);
    }
    Ok(())
}

/// Rename: same content, new path — a path-only update, zero re-embeds,
/// committed as one transaction. Writing the final content hash directly is
/// safe here: the surviving document's chunks and vectors are already
/// durably committed (its hash matched the file's), so the
/// failure-atomicity invariant holds.
async fn rename_document(
    ctx: &dyn IngestContext,
    old: &DocumentRow,
    file: &ScannedFile,
    content_hash: &str,
) -> EngineResult<()> {
    let mut writes: Vec<IngestWrite> = Vec::new();
    writes.push(IngestWrite::UpsertDocuments(vec![DocumentRow {
        id: old.id,
        file_path: file.rel.clone(),
        repo: Some(file.repo.clone()),
        content_hash: content_hash.to_string(),
    }]));
    // Chunks denormalize the path for display/filtering: rewrite in place.
    // Ids are unchanged, so stored vectors stay valid — no re-embeds. The
    // path change is a mutation, so each chunk fact is re-asserted with its
    // new path (section line ranges provide the provenance ranges).
    let sections = ctx.sections_for_document(old.id)?;
    let section_ranges: HashMap<Uuid, [u32; 2]> = sections
        .iter()
        .map(|s| (s.id, [s.start_line, s.end_line]))
        .collect();
    let mut chunks = ctx.chunks_for_document(old.id)?;
    for chunk in &mut chunks {
        chunk.file_path = file.rel.clone();
    }
    if !chunks.is_empty() {
        for chunk in &chunks {
            writes.push(IngestWrite::Emit(CatOperation::AssertObject {
                entity: qualify("rx", "Chunk"),
                object_id: chunk.id,
                value: serde_json::json!({
                    "file_path": chunk.file_path,
                    "section_ref": chunk.section_id,
                    "part": chunk.part,
                    "heading_path": chunk.heading_path,
                    "token_count": chunk.token_count,
                }),
                provenance: provenance(
                    &file.rel,
                    section_ranges.get(&chunk.section_id).copied(),
                    &chunk.content_hash,
                ),
            }));
        }
        writes.push(IngestWrite::UpsertChunks(chunks));
    }
    writes.push(IngestWrite::Emit(CatOperation::AssertObject {
        entity: qualify("md", "Document"),
        object_id: old.id,
        value: serde_json::json!({
            "file_path": file.rel,
            "repo": file.repo,
            "renamed_from": old.file_path,
        }),
        provenance: provenance(&file.rel, None, content_hash),
    }));
    // The kn.Artifact mirrors the document: refresh its uri. Tags are
    // content, and content is unchanged — the concept joins stand.
    writes.push(IngestWrite::Emit(CatOperation::AssertObject {
        entity: qualify("kn", "Artifact"),
        object_id: artifact_id(old.id),
        value: serde_json::json!({
            "uri": file.rel,
            "artifact_kind": "markdown",
            "artifact_content_hash": content_hash,
            "doc_ref": old.id,
        }),
        provenance: provenance(&file.rel, None, content_hash),
    }));
    ctx.apply_batch(writes).await
}

/// Delete a vanished document as one transaction: retract every fact
/// (chunks, KN artifact/concept joins, and the document's `links_to`
/// edges — inbound ones included, looked up before the FK cascade nulls
/// their resolutions) together with the cascade-delete of its rows.
async fn delete_document(ctx: &dyn IngestContext, doc: &DocumentRow) -> EngineResult<()> {
    let mut writes: Vec<IngestWrite> = Vec::new();
    for chunk in ctx.chunks_for_document(doc.id)? {
        writes.push(IngestWrite::Emit(CatOperation::RetractObject {
            object_id: chunk.id,
        }));
    }
    let sections = ctx.sections_for_document(doc.id)?;
    let section_ids: Vec<Uuid> = sections.iter().map(|s| s.id).collect();
    push_section_retractions(ctx, &mut writes, &section_ids)?;
    // Inbound links (other documents linking here): their rows survive with
    // `resolved_document` nulled by the FK cascade — re-resolvable later —
    // but the derived document edge must go NOW. (This document's own
    // outbound `links_to` edges were retracted with its links above.)
    for inbound in ctx.links_resolved_to(doc.id)? {
        writes.push(IngestWrite::Emit(CatOperation::RetractMorphism {
            morphism_id: links_to_morphism_id(inbound.id),
        }));
    }
    // KN teardown: the artifact's concept joins, then the artifact itself.
    let artifact = artifact_id(doc.id);
    push_artifact_concept_retractions(&mut writes, artifact_concepts(ctx, artifact)?);
    writes.push(IngestWrite::Emit(CatOperation::RetractObject {
        object_id: artifact,
    }));
    writes.push(IngestWrite::Emit(CatOperation::RetractObject {
        object_id: doc.id,
    }));
    writes.push(IngestWrite::DeleteStale(StaleTarget::Documents(vec![
        doc.id,
    ])));
    ctx.apply_batch(writes).await
}

/// Queue retractions of the operation-log facts for sections (their links
/// included). The caller commits them in the same batch as the row deletes
/// they mirror.
fn push_section_retractions(
    ctx: &dyn IngestContext,
    writes: &mut Vec<IngestWrite>,
    section_ids: &[Uuid],
) -> EngineResult<()> {
    if section_ids.is_empty() {
        return Ok(());
    }
    let links = ctx.links_for_sections(section_ids)?;
    let link_ids: Vec<Uuid> = links.iter().map(|l| l.id).collect();
    push_link_retractions(writes, &link_ids);
    for section_id in section_ids {
        writes.push(IngestWrite::Emit(CatOperation::RetractMorphism {
            morphism_id: stable_uuid(&["m", "md.document", &section_id.to_string()]),
        }));
        writes.push(IngestWrite::Emit(CatOperation::RetractObject {
            object_id: *section_id,
        }));
    }
    Ok(())
}

/// Queue retractions of link facts (object + both md morphisms + the
/// derived `kn.links_to` document edge — a no-op when the link never
/// resolved).
fn push_link_retractions(writes: &mut Vec<IngestWrite>, link_ids: &[Uuid]) {
    for link_id in link_ids {
        writes.push(IngestWrite::Emit(CatOperation::RetractMorphism {
            morphism_id: stable_uuid(&["m", "md.sourceSection", &link_id.to_string()]),
        }));
        writes.push(IngestWrite::Emit(CatOperation::RetractMorphism {
            morphism_id: stable_uuid(&["m", "md.sourceDocument", &link_id.to_string()]),
        }));
        writes.push(IngestWrite::Emit(CatOperation::RetractMorphism {
            morphism_id: links_to_morphism_id(*link_id),
        }));
        writes.push(IngestWrite::Emit(CatOperation::RetractObject {
            object_id: *link_id,
        }));
    }
}

/// The `kn.ArtifactConcept` ids currently joined to `artifact`, read from
/// the committed `kn.artifact` morphisms (source = join row, target =
/// artifact).
fn artifact_concepts(ctx: &dyn IngestContext, artifact: Uuid) -> EngineResult<Vec<Uuid>> {
    let arrow = qualify("kn", "artifact");
    Ok(ctx
        .morphisms_with_arrow_prefix(&arrow)?
        .into_iter()
        .filter(|edge| edge.arrow == arrow && edge.target == artifact)
        .map(|edge| edge.source)
        .collect())
}

/// Queue retractions for `kn.ArtifactConcept` join rows: both projection
/// morphisms, then the join object. Concept objects themselves are kept —
/// their identity is the tag, so an orphaned concept is harmless and
/// re-tagging finds it again.
fn push_artifact_concept_retractions(
    writes: &mut Vec<IngestWrite>,
    ac_ids: impl IntoIterator<Item = Uuid>,
) {
    for ac in ac_ids {
        writes.push(IngestWrite::Emit(CatOperation::RetractMorphism {
            morphism_id: stable_uuid(&["m", "kn.artifact", &ac.to_string()]),
        }));
        writes.push(IngestWrite::Emit(CatOperation::RetractMorphism {
            morphism_id: stable_uuid(&["m", "kn.concept", &ac.to_string()]),
        }));
        writes.push(IngestWrite::Emit(CatOperation::RetractObject {
            object_id: ac,
        }));
    }
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
        // Row update, re-asserted link fact, and the derived document-level
        // `kn.links_to` morphism (source document -> target document, the
        // edge relation_search traverses) commit together; the resolution
        // provenance is real: the source section's line range and content
        // hash (the span the link was written in).
        let resolution_provenance = provenance(
            &entry.source_path,
            Some(entry.section_range),
            &entry.section_content_hash,
        );
        ctx.apply_batch(vec![
            IngestWrite::UpsertLinks(vec![link]),
            IngestWrite::Emit(CatOperation::AssertObject {
                entity: qualify("md", "Link"),
                object_id: entry.link.id,
                value: serde_json::json!({
                    "raw_target": entry.link.raw_target,
                    "kind": entry.link.kind,
                    "target_ref": target_id,
                }),
                provenance: resolution_provenance.clone(),
            }),
            IngestWrite::Emit(CatOperation::AssertMorphism {
                arrow: qualify("kn", "links_to"),
                morphism_id: links_to_morphism_id(entry.link.id),
                source: entry.source_document,
                target: target_id,
                provenance: resolution_provenance,
            }),
        ])
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
