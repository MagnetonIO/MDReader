use mdreader_core::{EMBEDDING_DIM, ReaderStore, RelatedVectorState, SourceRoot};
use std::fs;
use tempfile::tempdir;

fn vector(x: f32, y: f32) -> Vec<f32> {
    let mut values = vec![0.0; EMBEDDING_DIM];
    values[0] = x;
    values[1] = y;
    values
}
fn embed(store: &mut ReaderStore, source: &str, path: &str, values: &[f32]) {
    let (revision, chunks) = store.embedding_inputs(source, path).unwrap();
    store
        .install_embeddings(
            source,
            path,
            &revision,
            &vec![values.to_vec(); chunks.len()],
        )
        .unwrap();
}

#[test]
fn ranked_related_preserves_two_hop_graph_and_vector_only_candidates() {
    let data = tempdir().unwrap();
    let notes = tempdir().unwrap();
    for (path, body) in [
        (
            "anchor.md",
            "---\ntags: [reader, local]\n---\n# Anchor\n[One](direct.md#details)\n",
        ),
        (
            "direct.md",
            "---\ntags: [reader]\n---\n# Direct\n[Two](two-hop.md)\n\n## Details\nEvidence.\n[Cycle](anchor.md)\n",
        ),
        ("two-hop.md", "# Two hop\n[Three](third-hop.md)\n"),
        ("third-hop.md", "# Third hop\nBeyond the depth budget.\n"),
        ("vector-only.md", "# Vector only\nNo links or tags.\n"),
        (
            "tag-only.md",
            "---\ntags: [local]\n---\n# Tag only\nNo links or vectors.\n",
        ),
        ("orthogonal.md", "# Orthogonal\nNo positive relation.\n"),
    ] {
        fs::write(notes.path().join(path), body).unwrap();
    }
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = store
        .add_local_source(
            "Synthetic",
            SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    store.index_source(&source.id).unwrap();
    embed(&mut store, &source.id, "anchor.md", &vector(1.0, 0.0));
    embed(&mut store, &source.id, "vector-only.md", &vector(1.0, 0.0));
    embed(&mut store, &source.id, "orthogonal.md", &vector(0.0, 1.0));
    let outcome = store.related_ranked(&source.id, "anchor.md", 20).unwrap();
    let paths = outcome
        .hits
        .iter()
        .map(|hit| hit.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        ["direct.md", "vector-only.md", "tag-only.md", "two-hop.md"]
    );
    assert_eq!(outcome.availability.vector, RelatedVectorState::Available);
    let direct = &outcome.hits[0];
    assert_eq!(direct.graph_depth, Some(1));
    assert_eq!(direct.line, 7);
    assert_eq!(direct.components.graph, 0.5);
    assert_eq!(direct.components.shared_tags, 0.25);
    assert_eq!(direct.shared_tag_count, 1);
    assert_eq!(direct.tag_union_count, 2);
    assert_eq!(
        direct.components.vector, None,
        "unembedded target must not claim a measured zero similarity"
    );
    let semantic = &outcome.hits[1];
    assert_eq!(semantic.components.vector, Some(0.4));
    assert_eq!(semantic.vector_similarity, Some(1.0));
    assert_eq!(semantic.components.graph, 0.0);
    let two = outcome
        .hits
        .iter()
        .find(|hit| hit.path == "two-hop.md")
        .unwrap();
    assert_eq!(two.graph_depth, Some(2));
    assert_eq!(two.components.graph, 0.25);
    let filtered = store
        .related_ranked_query(&source.id, "anchor.md", "path:two-hop.md", 1)
        .unwrap();
    assert_eq!(
        filtered.hits[0].graph_depth,
        Some(2),
        "filtered intermediate documents still participate in the two-hop walk"
    );
    for query in ["linked-to:two-hop.md", "Evidence"] {
        let filtered = store
            .related_ranked_query(&source.id, "anchor.md", query, 1)
            .unwrap();
        assert_eq!(filtered.hits[0].path, "direct.md", "{query}");
    }
    for hit in &outcome.hits {
        assert_eq!(hit.source_id, source.id);
        assert_eq!(
            hit.score,
            hit.components.graph
                + hit.components.shared_tags
                + hit.components.vector.unwrap_or(0.0)
        );
        assert!(!hit.revision.is_empty());
        assert!(hit.line > 0);
    }
    assert_eq!(
        store.related(&source.id, "anchor.md", 20).unwrap().len(),
        1,
        "legacy readonly link API remains one-hop compatible"
    );
}

#[test]
fn backlinks_work_without_vectors_and_follow_current_source_revisions() {
    let data = tempdir().unwrap();
    let notes = tempdir().unwrap();
    fs::write(
        notes.path().join("first.md"),
        "# First\n[Second](second.md)\n",
    )
    .unwrap();
    fs::write(
        notes.path().join("second.md"),
        "# Second\nNo outgoing links.\n",
    )
    .unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = store
        .add_local_source(
            "Notes",
            SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    store.index_source(&source.id).unwrap();
    let related = store.related_ranked(&source.id, "second.md", 12).unwrap();
    assert_eq!(
        related.availability.vector,
        RelatedVectorState::NoAnchorVectors
    );
    assert_eq!(related.hits.len(), 1);
    assert_eq!(related.hits[0].path, "first.md");
    assert_eq!(related.hits[0].graph_direction.as_deref(), Some("incoming"));
    assert_eq!(related.hits[0].line, 2);
    assert_eq!(related.hits[0].score, 0.5);
    fs::write(notes.path().join("first.md"), "# First\nLink removed.\n").unwrap();
    store.index_source(&source.id).unwrap();
    assert!(
        store
            .related_ranked(&source.id, "second.md", 12)
            .unwrap()
            .hits
            .is_empty()
    );
}

#[test]
fn centroid_means_anchor_vectors_and_uses_the_best_target_chunk_with_line_evidence() {
    let data = tempdir().unwrap();
    let notes = tempdir().unwrap();
    fs::write(
        notes.path().join("anchor.md"),
        "# Anchor\nFirst.\n\n## Other\nSecond.\n",
    )
    .unwrap();
    fs::write(
        notes.path().join("target.md"),
        "# Target\nFirst.\n\n## Best\nSecond.\n",
    )
    .unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = store
        .add_local_source(
            "Synthetic",
            SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    store.index_source(&source.id).unwrap();
    for (path, values) in [
        ("anchor.md", vec![vector(0.6, 0.8), vector(0.6, -0.8)]),
        ("target.md", vec![vector(0.0, 1.0), vector(1.0, 0.0)]),
    ] {
        let (revision, chunks) = store.embedding_inputs(&source.id, path).unwrap();
        assert_eq!(chunks.len(), 2);
        store
            .install_embeddings(&source.id, path, &revision, &values)
            .unwrap();
    }
    let outcome = store.related_ranked(&source.id, "anchor.md", 10).unwrap();
    assert_eq!(outcome.availability.anchor_vectors, 2);
    assert_eq!(outcome.availability.evaluated_vector_chunks, 2);
    assert_eq!(outcome.hits.len(), 1);
    assert_eq!(outcome.hits[0].vector_similarity, Some(1.0));
    assert_eq!(outcome.hits[0].components.vector, Some(0.4));
    assert_eq!(
        outcome.hits[0].line, 4,
        "best chunk heading, not the target document centroid or first chunk"
    );
}

#[test]
fn filters_scope_and_chunk_heavy_documents_cannot_starve_vector_candidates() {
    let data = tempdir().unwrap();
    let notes = tempdir().unwrap();
    let hidden = tempdir().unwrap();
    let mut heavy = String::from("# Heavy\n");
    for index in 0..4200 {
        heavy.push_str(&format!("\n## Part {index}\nCrowder body.\n"));
    }
    fs::write(notes.path().join("anchor.md"), &heavy).unwrap();
    fs::write(notes.path().join("crowder.md"), &heavy).unwrap();
    for (path, seconds) in [("small-a.md", 1_735_689_600), ("small-b.md", 1_798_761_600)] {
        fs::write(
            notes.path().join(path),
            "---\ntags: [keep]\n---\n# Small\nUnique matching text.\n",
        )
        .unwrap();
        fs::File::open(notes.path().join(path))
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
            .unwrap();
    }
    fs::write(
        hidden.path().join("hidden.md"),
        "# Hidden\nUnique matching text.\n",
    )
    .unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = store
        .add_local_source(
            "Selected",
            SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    let other = store
        .add_local_source(
            "Hidden",
            SourceRoot::capture(hidden.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    store.index_source(&source.id).unwrap();
    store.index_source(&other.id).unwrap();
    embed(&mut store, &source.id, "anchor.md", &vector(1.0, 0.0));
    embed(&mut store, &source.id, "crowder.md", &vector(1.0, 0.0));
    embed(&mut store, &source.id, "small-a.md", &vector(0.6, 0.8));
    embed(&mut store, &source.id, "small-b.md", &vector(0.6, 0.8));
    embed(&mut store, &other.id, "hidden.md", &vector(1.0, 0.0));
    let anchor_count = store
        .embedding_inputs(&source.id, "anchor.md")
        .unwrap()
        .1
        .len();
    let target_count = store
        .embedding_inputs(&source.id, "crowder.md")
        .unwrap()
        .1
        .len();
    assert!(anchor_count > 4096 && target_count > 4096);
    let outcome = store.related_ranked(&source.id, "anchor.md", 3).unwrap();
    assert_eq!(
        outcome
            .hits
            .iter()
            .map(|h| h.path.as_str())
            .collect::<Vec<_>>(),
        ["crowder.md", "small-a.md", "small-b.md"]
    );
    assert_eq!(
        outcome.availability.evaluated_vector_chunks,
        target_count as u64 + 2,
        "exclude all anchor and hidden-source chunks before scanning candidates"
    );
    let filtered = store
        .related_ranked_query(
            &source.id,
            "anchor.md",
            "Unique tag:keep path:small after:2026-01-01",
            1,
        )
        .unwrap();
    assert_eq!(filtered.hits.len(), 1);
    assert_eq!(filtered.hits[0].path, "small-b.md");
    assert_eq!(
        filtered.availability.evaluated_vector_chunks, 1,
        "filters precede the vector scan bound and result limit"
    );
    assert!((filtered.hits[0].components.vector.unwrap() - 0.24).abs() < 1e-6);
    let readonly = ReaderStore::open_readonly(data.path()).unwrap();
    assert_eq!(
        readonly
            .related_ranked(&source.id, "anchor.md", 3)
            .unwrap()
            .hits
            .len(),
        3
    );
    store.set_source_enabled(&source.id, false).unwrap();
    assert!(readonly.related_ranked(&source.id, "anchor.md", 3).is_err());
}

#[test]
fn unavailable_vectors_and_measured_zero_are_distinct_and_reindex_invalidates_evidence() {
    use mdreader_core::{Cancellation, RelatedLegacyState};
    let data = tempdir().unwrap();
    let notes = tempdir().unwrap();
    fs::write(
        notes.path().join("anchor.md"),
        "# Anchor\n[Next](target.md)\n\n## Other\nSecond.\n",
    )
    .unwrap();
    fs::write(notes.path().join("target.md"), "# Target\nNeighbor.\n").unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = store
        .add_local_source(
            "Synthetic",
            SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    store.index_source(&source.id).unwrap();
    let missing = store.related_ranked(&source.id, "unknown.md", 10).unwrap();
    assert!(missing.hits.is_empty());
    assert!(missing.anchor.is_none());
    assert!(!missing.availability.graph);
    assert_eq!(
        missing.availability.vector,
        RelatedVectorState::AnchorMissing
    );
    let lexical = store.related_ranked(&source.id, "anchor.md", 10).unwrap();
    assert_eq!(
        lexical.availability.vector,
        RelatedVectorState::NoAnchorVectors
    );
    assert_eq!(lexical.hits[0].components.vector, None);
    assert_eq!(
        lexical.availability.legacy_concepts,
        RelatedLegacyState::NotMigrated
    );
    assert_eq!(
        lexical.availability.legacy_graph_entities,
        RelatedLegacyState::NotMigrated
    );
    let (revision, chunks) = store.embedding_inputs(&source.id, "anchor.md").unwrap();
    assert_eq!(chunks.len(), 2);
    store
        .install_embeddings(
            &source.id,
            "anchor.md",
            &revision,
            &[vector(1.0, 0.0), vector(-1.0, 0.0)],
        )
        .unwrap();
    let zero = store.related_ranked(&source.id, "anchor.md", 10).unwrap();
    assert_eq!(zero.availability.vector, RelatedVectorState::ZeroCentroid);
    assert_eq!(zero.hits[0].components.vector, None);
    embed(&mut store, &source.id, "anchor.md", &vector(1.0, 0.0));
    embed(&mut store, &source.id, "target.md", &vector(-1.0, 0.0));
    let measured = store.related_ranked(&source.id, "anchor.md", 10).unwrap();
    assert_eq!(measured.availability.vector, RelatedVectorState::Available);
    assert_eq!(measured.hits[0].components.vector, Some(0.0));
    assert_eq!(measured.hits[0].vector_similarity, Some(-1.0));
    let fixture = rusqlite::Connection::open(data.path().join("index.sqlite")).unwrap();
    fixture.execute("DELETE FROM vectors WHERE rowid=(SELECT MIN(c.id) FROM chunks c JOIN documents d ON d.id=c.document_id WHERE d.source_id=?1 AND d.path='anchor.md')",[&source.id]).unwrap();
    drop(fixture);
    let partial = store.related_ranked(&source.id, "anchor.md", 10).unwrap();
    assert_eq!(partial.availability.anchor_chunks, 2);
    assert_eq!(partial.availability.anchor_vectors, 1);
    assert_eq!(
        partial.availability.vector,
        RelatedVectorState::Available,
        "stored-vector coverage is reported rather than inventing an unavailable model"
    );
    let old = measured.hits[0].revision.clone();
    fs::write(notes.path().join("target.md"), "# Target\nNew revision.\n").unwrap();
    store.index_source(&source.id).unwrap();
    let changed = store.related_ranked(&source.id, "anchor.md", 10).unwrap();
    assert_ne!(changed.hits[0].revision, old);
    assert_eq!(changed.hits[0].components.vector, None);
    fs::remove_file(notes.path().join("target.md")).unwrap();
    store.index_source(&source.id).unwrap();
    assert!(
        store
            .related_ranked(&source.id, "anchor.md", 10)
            .unwrap()
            .hits
            .is_empty()
    );
    for limit in [0, 101] {
        assert!(
            store
                .related_ranked(&source.id, "anchor.md", limit)
                .is_err()
        );
    }
    assert!(
        store
            .related_ranked(&source.id, "../anchor.md", 10)
            .is_err()
    );
    assert!(
        store
            .related_ranked_query(&source.id, "anchor.md", "invalid:filter", 10)
            .is_err()
    );
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(matches!(
        store.related_ranked_cancelled(&source.id, "anchor.md", None, 10, &cancel),
        Err(mdreader_core::ReaderError::Cancelled)
    ));
    drop(store);
    assert!(
        ReaderStore::open_readonly(data.path())
            .unwrap()
            .related_ranked(&source.id, "anchor.md", 10)
            .unwrap()
            .hits
            .is_empty()
    );
}

/// Explicit manual measurement, excluded from routine regression runs. Every
/// corpus, vector and database is synthetic, with no model or user configuration.
#[test]
#[ignore = "manual synthetic 1k/10k/50k performance measurement"]
fn ranked_related_synthetic_performance() {
    use serde_json::json;
    use std::time::Instant;
    fn measure(mut operation: impl FnMut() -> mdreader_core::RelatedRanked) -> serde_json::Value {
        for _ in 0..5 {
            operation();
        }
        let mut samples = Vec::new();
        for _ in 0..20 {
            let start = Instant::now();
            operation();
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        let mut sorted = samples.clone();
        sorted.sort_by(f64::total_cmp);
        json!({"samples_ms":samples,"p50_ms":sorted[9],"p95_ms":sorted[18],"max_ms":sorted[19]})
    }
    let mut results = Vec::new();
    for count in [1000, 10000, 50000] {
        eprintln!("ranked related synthetic corpus: {count}");
        let data = tempdir().unwrap();
        let notes = tempdir().unwrap();
        for index in 0..count {
            fs::write(notes.path().join(format!("note{index:06}.md")),format!("---\ntags: [benchmark, topic{:03}]\n---\n# Document {index}\nSynthetic reading evidence.\n\n## Details\n[Next](note{:06}.md#details)\n",index%97,(index+1)%count)).unwrap();
        }
        let mut store = ReaderStore::open(data.path()).unwrap();
        let source = store
            .add_local_source(
                "Synthetic",
                SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
            )
            .unwrap();
        let start = Instant::now();
        store.index_source(&source.id).unwrap();
        let index_ms = start.elapsed().as_secs_f64() * 1000.0;
        let graph_tags = measure(|| {
            let outcome = store
                .related_ranked(&source.id, "note000000.md", 20)
                .unwrap();
            assert_eq!(
                outcome.availability.vector,
                RelatedVectorState::NoAnchorVectors
            );
            assert!(outcome.hits.iter().any(|hit| hit.components.graph > 0.0));
            outcome
        });
        let start = Instant::now();
        // Bulk fixture installation uses the real vec0 table and current chunk
        // IDs in one transaction, avoiding per-document fsync from dominating
        // setup. Production installs continue through revision-guarded core API.
        let mut fixture = rusqlite::Connection::open(data.path().join("index.sqlite")).unwrap();
        let tx = fixture.transaction().unwrap();
        {
            let mut rows_statement=tx.prepare("SELECT c.id,d.path FROM documents d JOIN chunks c ON c.document_id=d.id WHERE d.source_id=?1 ORDER BY c.id").unwrap();
            let mut rows = rows_statement.query([&source.id]).unwrap();
            let mut insert = tx
                .prepare("INSERT INTO vectors(rowid,embedding) VALUES(?1,?2)")
                .unwrap();
            while let Some(row) = rows.next().unwrap() {
                let id: i64 = row.get(0).unwrap();
                let path: String = row.get(1).unwrap();
                let number: usize = path[4..10].parse().unwrap();
                let x = if number == 0 {
                    1.0
                } else {
                    0.2 + (number % 97) as f32 * 0.008
                };
                let bytes = vector(x, (1.0 - x * x).sqrt())
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
                    .collect::<Vec<_>>();
                insert.execute(rusqlite::params![id, bytes]).unwrap();
            }
        }
        tx.commit().unwrap();
        drop(fixture);
        let install_ms = start.elapsed().as_secs_f64() * 1000.0;
        let complete = measure(|| {
            let outcome = store
                .related_ranked(&source.id, "note000000.md", 20)
                .unwrap();
            assert_eq!(outcome.availability.vector, RelatedVectorState::Available);
            assert_eq!(outcome.availability.anchor_vectors, 2);
            assert_eq!(
                outcome.availability.evaluated_vector_chunks,
                2 * count as u64 - 2
            );
            assert_eq!(
                outcome.availability.evaluated_vector_documents,
                count as u64 - 1
            );
            assert!(
                outcome
                    .hits
                    .iter()
                    .all(|hit| hit.components.vector.is_some())
            );
            outcome
        });
        let filtered = measure(|| {
            let outcome = store
                .related_ranked_query(&source.id, "note000000.md", "path:note000002.md", 20)
                .unwrap();
            assert_eq!(outcome.hits.len(), 1);
            assert_eq!(outcome.hits[0].graph_depth, Some(2));
            assert_eq!(outcome.availability.evaluated_vector_chunks, 2);
            outcome
        });
        results.push(json!({"documents":count,"chunks":2*count,"index_ms":index_ms,"synthetic_vector_install_ms":install_ms,"graph_tags":graph_tags,"graph_tags_vectors":complete,"filtered_vectors":filtered}));
    }
    println!(
        "RELATED_PERF_JSON:{}",
        json!({"benchmark":"ranked-related-native-synthetic-v1","profile":if cfg!(debug_assertions){"debug"}else{"release"},"samples":20,"warmups":5,"percentiles":"nearest-rank","model_enabled":false,"store_source_blake3":blake3::hash(include_str!("../src/store.rs").as_bytes()).to_hex().to_string(),"related_source_blake3":blake3::hash(include_str!("../src/related.rs").as_bytes()).to_hex().to_string(),"results":results})
    );
}
