//! Immutable demo deployment identity.
//!
//! The fingerprint intentionally binds three independent inputs: the deployed
//! source bytes, the authoritative catalog revision, and the build commit. It
//! is not a substitute for verifying that revision against the opened catalog;
//! the demo server remains latched off until Task 3 supplies that authority.

use std::collections::HashMap;

use catdb_engine::{EmbeddedCatDb, EngineResult};

/// Compute the immutable workspace fingerprint.
///
/// `files` are sorted by source-relative path before hashing, so filesystem
/// traversal order cannot change the identity.
pub fn workspace_fingerprint(
    build_commit: &str,
    catalog_revision: &str,
    files: &[(String, Vec<u8>)],
) -> String {
    let mut ordered = files.to_vec();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"catdb-demo-workspace/v1\0");
    hash_part(&mut hasher, build_commit.as_bytes());
    hash_part(&mut hasher, catalog_revision.as_bytes());
    for (path, bytes) in ordered {
        hash_part(&mut hasher, path.as_bytes());
        hash_part(&mut hasher, &bytes);
    }
    hasher.finalize().to_hex().to_string()
}

fn hash_part(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value);
}

/// Hash the normalized, query-serving state of a built workspace.
///
/// Database bytes are recorded separately by the image build. This digest
/// deliberately excludes generated UUIDs, timestamps, source root paths, and
/// SQLite page layout so two independent imports of the same corpus can prove
/// that they produced the same document/section/chunk/link semantics.
pub async fn logical_workspace_fingerprint(engine: &EmbeddedCatDb) -> EngineResult<String> {
    let status = engine.status().await?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"catdb-demo-logical-workspace/v1\0");

    hash_part(&mut hasher, &status.schema_version.to_le_bytes());
    if let Some(index) = status.active_index {
        hash_part(&mut hasher, index.embedding_model.as_bytes());
        hash_part(&mut hasher, &index.dim.to_le_bytes());
        hash_part(&mut hasher, index.chunker_version.as_bytes());
        hash_part(&mut hasher, b"active");
    } else {
        hash_part(&mut hasher, b"no-active-index");
    }

    let mut sources = status.sources;
    sources.sort_by(|left, right| {
        left.include_globs
            .cmp(&right.include_globs)
            .then_with(|| left.exclude_globs.cmp(&right.exclude_globs))
            .then_with(|| left.max_file_size.cmp(&right.max_file_size))
    });
    for source in sources {
        hash_part(
            &mut hasher,
            &serde_json::to_vec(&(
                source.enabled,
                source.watch,
                source.include_globs,
                source.exclude_globs,
                source.max_file_size,
                source.respect_gitignore,
                source.embed_policy,
            ))
            .expect("source deployment state serializes"),
        );
    }

    let mut documents = engine.store().list_documents()?;
    documents.sort_by(|left, right| left.file_path.cmp(&right.file_path));
    let document_paths = documents
        .iter()
        .map(|document| (document.id, document.file_path.clone()))
        .collect::<HashMap<_, _>>();

    for document in documents {
        hash_part(&mut hasher, document.file_path.as_bytes());
        hash_part(
            &mut hasher,
            document.repo.as_deref().unwrap_or("").as_bytes(),
        );
        hash_part(&mut hasher, document.content_hash.as_bytes());

        let sections = engine.store().sections_for_document(document.id)?;
        let section_keys = sections
            .iter()
            .map(|section| {
                (
                    section.id,
                    format!(
                        "{}:{}-{}",
                        section.heading_path.join("\u{1f}"),
                        section.start_line,
                        section.end_line
                    ),
                )
            })
            .collect::<HashMap<_, _>>();
        for section in &sections {
            hash_part(
                &mut hasher,
                section_keys
                    .get(&section.id)
                    .expect("section key was built")
                    .as_bytes(),
            );
            hash_part(&mut hasher, section.content_hash.as_bytes());
            hash_part(
                &mut hasher,
                section
                    .parent_id
                    .and_then(|id| section_keys.get(&id))
                    .map_or(b"" as &[u8], |key| key.as_bytes()),
            );
        }

        let mut chunks = engine.store().chunks_for_document(document.id)?;
        chunks.sort_by(|left, right| {
            left.heading_path
                .cmp(&right.heading_path)
                .then_with(|| left.part.cmp(&right.part))
                .then_with(|| left.content_hash.cmp(&right.content_hash))
        });
        for chunk in chunks {
            hash_part(&mut hasher, chunk.heading_path.join("\u{1f}").as_bytes());
            hash_part(&mut hasher, &chunk.part.to_le_bytes());
            hash_part(&mut hasher, chunk.text.as_bytes());
            hash_part(&mut hasher, chunk.embedding_text.as_bytes());
            hash_part(&mut hasher, chunk.content_hash.as_bytes());
            hash_part(&mut hasher, &chunk.token_count.to_le_bytes());
        }

        let section_ids = sections
            .iter()
            .map(|section| section.id)
            .collect::<Vec<_>>();
        let mut links = engine.store().links_for_sections(&section_ids)?;
        links.sort_by(|left, right| {
            section_keys
                .get(&left.source_section)
                .cmp(&section_keys.get(&right.source_section))
                .then_with(|| left.raw_target.cmp(&right.raw_target))
                .then_with(|| left.kind.cmp(&right.kind))
        });
        for link in links {
            hash_part(
                &mut hasher,
                section_keys
                    .get(&link.source_section)
                    .expect("link source section belongs to document")
                    .as_bytes(),
            );
            hash_part(&mut hasher, link.raw_target.as_bytes());
            hash_part(&mut hasher, link.kind.as_bytes());
            hash_part(
                &mut hasher,
                link.resolved_document
                    .and_then(|id| document_paths.get(&id))
                    .map_or(b"" as &[u8], |path| path.as_bytes()),
            );
        }
    }

    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use catdb_embed::HashEmbedder;
    use catdb_engine::{EngineConfig, ImportOptions};
    use tempfile::tempdir;

    use super::*;

    fn files() -> Vec<(String, Vec<u8>)> {
        vec![
            ("notes/b.md".into(), b"# B\n".to_vec()),
            ("notes/a.md".into(), b"# A\n".to_vec()),
        ]
    }

    #[test]
    fn fingerprint_is_order_independent_but_binds_every_pinned_input() {
        let baseline = workspace_fingerprint("a".repeat(40).as_str(), "catalog-1", &files());
        let mut reversed = files();
        reversed.reverse();
        assert_eq!(
            baseline,
            workspace_fingerprint("a".repeat(40).as_str(), "catalog-1", &reversed)
        );

        let mut changed_bytes = files();
        changed_bytes[0].1.push(b'!');
        assert_ne!(
            baseline,
            workspace_fingerprint("a".repeat(40).as_str(), "catalog-1", &changed_bytes)
        );
        assert_ne!(
            baseline,
            workspace_fingerprint("a".repeat(40).as_str(), "catalog-2", &files())
        );
        assert_ne!(
            baseline,
            workspace_fingerprint("b".repeat(40).as_str(), "catalog-1", &files())
        );
    }

    #[tokio::test]
    async fn independent_builds_have_identical_logical_state_and_content_changes_break_it() {
        let temp = tempdir().unwrap();
        let corpus = temp.path().join("corpus");
        fs::create_dir_all(corpus.join("notes")).unwrap();
        fs::write(
            corpus.join("README.md"),
            "# Demo\n\nRead [the note](notes/provenance.md).\n",
        )
        .unwrap();
        fs::write(
            corpus.join("notes/provenance.md"),
            "# Provenance\n\nEvery result retains its source.\n",
        )
        .unwrap();

        let build = |name: &str| {
            EmbeddedCatDb::open(
                EngineConfig::new(temp.path().join(name), Arc::new(HashEmbedder::new()))
                    .with_domain(crate::registration()),
            )
            .unwrap()
        };
        let first = build("first.sqlite");
        first
            .import(&corpus, ImportOptions::default())
            .await
            .unwrap();
        let second = build("second.sqlite");
        second
            .import(&corpus, ImportOptions::default())
            .await
            .unwrap();

        let first_fingerprint = logical_workspace_fingerprint(&first).await.unwrap();
        assert_eq!(
            first_fingerprint,
            logical_workspace_fingerprint(&second).await.unwrap()
        );

        fs::write(
            corpus.join("notes/provenance.md"),
            "# Provenance\n\nA materially changed result retains its source.\n",
        )
        .unwrap();
        let changed = build("changed.sqlite");
        changed
            .import(&corpus, ImportOptions::default())
            .await
            .unwrap();
        assert_ne!(
            first_fingerprint,
            logical_workspace_fingerprint(&changed).await.unwrap()
        );
    }
}
