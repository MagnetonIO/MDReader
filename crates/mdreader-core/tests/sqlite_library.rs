use mdreader_core::{ReaderStore, SourceRoot};
use tempfile::tempdir;

#[test]
fn durable_jobs_recover_and_filtered_search_preserves_rank_evidence() {
    let data = tempdir().unwrap();
    let notes = tempdir().unwrap();
    std::fs::create_dir(notes.path().join("plans")).unwrap();
    std::fs::write(
        notes.path().join("plans/one.md"),
        "---\ntags: [reader, local]\n---\n# One\nDurable offline search\n",
    )
    .unwrap();
    std::fs::write(
        notes.path().join("two.md"),
        "# Two\nDurable offline search\n",
    )
    .unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = store
        .add_local_source(
            "Notes",
            SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    store.queue_index(&source.id).unwrap();
    drop(store);
    let mut store = ReaderStore::open(data.path()).unwrap();
    assert_eq!(store.pending_index_jobs().unwrap(), vec![source.id.clone()]);
    store.index_source(&source.id).unwrap();
    store.record_index_job(&source.id, "done", None).unwrap();
    assert!(store.pending_index_jobs().unwrap().is_empty());
    assert_eq!(store.index_job(&source.id).unwrap().unwrap().documents, 2);
    store.queue_index(&source.id).unwrap();
    assert!(!store
        .record_index_job_if_current(&source.id, 1, "done", None)
        .unwrap());
    assert_eq!(
        store.index_job(&source.id).unwrap().unwrap().phase,
        "queued"
    );
    for query in [
        "offline tag:reader",
        "path:plans/*",
        "after:2020-01-01 tag:local",
    ] {
        let hits = store
            .search(query, std::slice::from_ref(&source.id), 10)
            .unwrap();
        assert_eq!(hits.len(), 1, "{query}");
        assert_eq!(hits[0].path, "plans/one.md");
        assert_eq!(hits[0].lexical_rank, Some(1));
        assert!(hits[0].modified_ms > 0);
    }
    assert!(store
        .search("after:2026-02-30", std::slice::from_ref(&source.id), 10)
        .is_err());
    assert!(store
        .search("path:", std::slice::from_ref(&source.id), 10)
        .is_err());
    assert!(store.search("unknown:value", &[source.id], 10).is_err());
}
#[test]
fn sqlite_reopen_rebuild_and_source_revoke_preserve_user_state() {
    let data = tempdir().unwrap();
    let notes = tempdir().unwrap();
    std::fs::write(
        notes.path().join("plan.md"),
        "# Plan\n\n## Offline reading\nKeep the selected documents on the phone.\n",
    )
    .unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = store
        .add_local_source(
            "Notes",
            SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    assert_eq!(store.index_source(&source.id).unwrap(), 1);
    let hits = store
        .search("selected documents", std::slice::from_ref(&source.id), 10)
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "plan.md");
    assert_eq!(hits[0].heading, "Plan > Offline reading");
    assert_eq!(hits[0].line_start, 3);
    let revision = hits[0].revision.clone();
    store.set_bookmark(&source.id, "plan.md", 4).unwrap();
    drop(store);
    assert!(data.path().join("library.sqlite").is_file());
    assert!(data.path().join("index.sqlite").is_file());
    let mut store = ReaderStore::open(data.path()).unwrap();
    store.rebuild_index().unwrap();
    assert_eq!(store.bookmark(&source.id, "plan.md").unwrap(), Some(4));
    assert_eq!(
        store
            .search("selected documents", std::slice::from_ref(&source.id), 10)
            .unwrap()[0]
            .revision,
        revision
    );
    store.set_source_enabled(&source.id, false).unwrap();
    assert!(store
        .search("selected documents", std::slice::from_ref(&source.id), 10)
        .is_err());
    assert!(store.bookmark(&source.id, "plan.md").is_err());
    store.set_source_enabled(&source.id, true).unwrap();
    assert_eq!(store.bookmark(&source.id, "plan.md").unwrap(), Some(4));
}
#[test]
fn incremental_index_removes_deleted_documents_and_never_searches_another_source() {
    let data = tempdir().unwrap();
    let a = tempdir().unwrap();
    let b = tempdir().unwrap();
    std::fs::write(a.path().join("one.md"), "# First\nUnique shared token").unwrap();
    std::fs::write(b.path().join("two.md"), "# Second\nUnique shared token").unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let sa = store
        .add_local_source(
            "A",
            SourceRoot::capture(a.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    let sb = store
        .add_local_source(
            "B",
            SourceRoot::capture(b.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    store.index_source(&sa.id).unwrap();
    store.index_source(&sb.id).unwrap();
    assert_eq!(
        store
            .search("Unique", std::slice::from_ref(&sa.id), 10)
            .unwrap()
            .len(),
        1
    );
    assert!(store.search("Unique", &[], 10).is_err());
    std::fs::remove_file(a.path().join("one.md")).unwrap();
    store.index_source(&sa.id).unwrap();
    assert!(store.search("Unique", &[sa.id], 10).unwrap().is_empty());
    assert_eq!(store.search("Unique", &[sb.id], 10).unwrap().len(), 1);
}

#[test]
fn vectors_are_revision_bound_scoped_and_disposable() {
    let data = tempdir().unwrap();
    let notes = tempdir().unwrap();
    std::fs::write(notes.path().join("one.md"), "# One\nA quiet animal sleeps.").unwrap();
    std::fs::write(
        notes.path().join("two.md"),
        "# Two\nDatabase storage implementation.",
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
    let mut animal = vec![0.0; 384];
    animal[0] = 1.0;
    let mut database = vec![0.0; 384];
    database[1] = 1.0;
    let (r1, inputs) = store.embedding_inputs(&source.id, "one.md").unwrap();
    assert_eq!(inputs.len(), 1);
    store
        .install_embeddings(&source.id, "one.md", &r1, std::slice::from_ref(&animal))
        .unwrap();
    let (r2, _) = store.embedding_inputs(&source.id, "two.md").unwrap();
    store
        .install_embeddings(&source.id, "two.md", &r2, &[database])
        .unwrap();
    assert_eq!(
        store
            .semantic_search(&animal, std::slice::from_ref(&source.id), 1)
            .unwrap()[0]
            .path,
        "one.md"
    );
    assert_eq!(
        store
            .hybrid_search("animal", &animal, std::slice::from_ref(&source.id), 1)
            .unwrap()[0]
            .path,
        "one.md"
    );
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&source.id))
            .unwrap()
            .embedded,
        2
    );
    std::fs::write(notes.path().join("one.md"), "# One\nChanged body.").unwrap();
    store.index_source(&source.id).unwrap();
    assert!(matches!(
        store.install_embeddings(&source.id, "one.md", &r1, &[animal]),
        Err(mdreader_core::ReaderError::Conflict)
    ));
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&source.id))
            .unwrap()
            .embedded,
        1
    );
    store.set_bookmark(&source.id, "one.md", 2).unwrap();
    drop(store);
    std::fs::remove_file(data.path().join("index.sqlite")).unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    store.rebuild_index().unwrap();
    assert_eq!(store.bookmark(&source.id, "one.md").unwrap(), Some(2));
    assert_eq!(store.index_status(&[source.id]).unwrap().embedded, 0);
}

#[test]
fn source_replacement_traversal_and_cancelled_index_fail_closed() {
    let data = tempdir().unwrap();
    let parent = tempdir().unwrap();
    let notes = parent.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    std::fs::write(notes.join("one.md"), "# Original\nUnique body.").unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = store
        .add_local_source(
            "Notes",
            SourceRoot::capture(notes.canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    store.index_source(&source.id).unwrap();
    let old = store.read_document(&source.id, "one.md", None).unwrap();
    assert!(store
        .read_document(&source.id, "../outside.md", None)
        .is_err());
    std::fs::write(notes.join("one.md"), "# Updated").unwrap();
    assert!(matches!(
        store.read_document(&source.id, "one.md", Some(&old.revision)),
        Err(mdreader_core::ReaderError::Conflict)
    ));
    let cancel = mdreader_core::Cancellation::default();
    cancel.cancel();
    assert!(matches!(
        store.index_source_cancelled(&source.id, &cancel),
        Err(mdreader_core::ReaderError::Cancelled)
    ));
    assert_eq!(
        store
            .search("Unique", std::slice::from_ref(&source.id), 10)
            .unwrap()
            .len(),
        1
    );
    std::fs::rename(&notes, parent.path().join("original")).unwrap();
    std::fs::create_dir(&notes).unwrap();
    assert!(store
        .search("Unique", std::slice::from_ref(&source.id), 10)
        .is_err());
    assert!(store.read_document(&source.id, "one.md", None).is_err());
}

#[test]
fn scanner_bounded_read_rechecks_growth_after_discovery() {
    let notes = tempdir().unwrap();
    let path = notes.path().join("one.md");
    std::fs::write(&path, "# Original").unwrap();
    let root = SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap();
    let files = mdreader_core::scan_source_root(
        &root,
        &mdreader_core::ScanOptions {
            include_mdx: false,
            max_file_size: 32,
            respect_gitignore: true,
        },
    )
    .unwrap();
    std::fs::write(path, vec![b'x'; 1024]).unwrap();
    assert_eq!(
        files[0].read_bounded(32).unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
}

#[test]
fn references_resolve_relative_paths_duplicate_names_and_heading_lines() {
    let data = tempdir().unwrap();
    let notes = tempdir().unwrap();
    std::fs::create_dir(notes.path().join("docs")).unwrap();
    std::fs::write(
        notes.path().join("docs/from.md"),
        "# From\n[Target](../target.md#second-heading)",
    )
    .unwrap();
    std::fs::write(
        notes.path().join("target.md"),
        "# Target\n\n## Second heading\nEvidence here.\n\n## Second heading\nAgain.",
    )
    .unwrap();
    std::fs::write(notes.path().join("docs/target.md"), "# Another target").unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let source = store
        .add_local_source(
            "Notes",
            SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    store.index_source(&source.id).unwrap();
    let reference = store
        .resolve_reference(&source.id, "docs/from.md", "../target.md#second-heading")
        .unwrap();
    assert_eq!(reference.len(), 1);
    assert_eq!(reference[0].path, "target.md");
    assert_eq!(reference[0].line, 3);
    assert_eq!(
        store
            .resolve_reference(&source.id, "docs/from.md", "../target.md#second-heading-1")
            .unwrap()[0]
            .line,
        6
    );
    assert!(store
        .resolve_reference(&source.id, "docs/from.md", "../target.md#missing")
        .unwrap()
        .is_empty());
    assert!(store
        .resolve_reference(&source.id, "docs/from.md", "../../outside.md")
        .is_err());
    let backlinks = store.related(&source.id, "target.md", 10).unwrap();
    assert_eq!(backlinks[0].path, "docs/from.md");
}

#[test]
fn bookmarks_require_a_current_source_grant_for_reads_and_writes() {
    let data = tempfile::tempdir().unwrap();
    let notes = tempfile::tempdir().unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let root = SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap();
    let source = store.add_local_source("Notes", root).unwrap();
    store.set_bookmark(&source.id, "note.md", 2).unwrap();
    store.set_source_enabled(&source.id, false).unwrap();
    assert!(store.bookmark(&source.id, "note.md").is_err());
    assert!(store.set_bookmark(&source.id, "note.md", 3).is_err());
    assert!(store.bookmark("ungranted", "note.md").is_err());
}

#[test]
fn repeated_admission_cannot_revive_a_revoked_source_and_lookup_is_read_only() {
    let data = tempfile::tempdir().unwrap();
    let notes = tempfile::tempdir().unwrap();
    let root = SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    assert!(store
        .find_local_source(&root.path, &root.identity)
        .unwrap()
        .is_none());
    assert!(store.list_sources().unwrap().is_empty());
    let source = store.add_local_source("Notes", root.clone()).unwrap();
    store.set_source_enabled(&source.id, false).unwrap();
    let late_admission = store.add_local_source("Late import", root.clone()).unwrap();
    assert_eq!(late_admission.id, source.id);
    assert!(!late_admission.enabled);
    assert!(store
        .search("anything", std::slice::from_ref(&source.id), 10)
        .is_err());
    assert!(store
        .find_local_source(data.path(), &root.identity)
        .unwrap()
        .is_none());
    store.set_source_enabled(&source.id, true).unwrap();
    assert!(
        store
            .find_local_source(&root.path, &root.identity)
            .unwrap()
            .unwrap()
            .enabled
    );
}

#[test]
#[cfg(unix)]
fn indexing_more_files_than_descriptor_budget_does_not_exhaust_handles() {
    let status = std::process::Command::new("/bin/sh")
        .args(["-c", "ulimit -n 128\nexec \"$@\"", "mdreader-low-fd"])
        .arg(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "low_descriptor_index_process"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "indexing must work under a normal descriptor budget"
    );
}
#[test]
#[ignore = "synthetic subprocess fixture"]
#[cfg(unix)]
fn low_descriptor_index_process() {
    let data = tempfile::tempdir().unwrap();
    let notes = tempfile::tempdir().unwrap();
    for index in 0..500 {
        std::fs::write(
            notes.path().join(format!("{index}.md")),
            format!("# Note {index}\nBounded handle indexing"),
        )
        .unwrap();
    }
    let mut store = ReaderStore::open(data.path()).unwrap();
    let root = SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap();
    let source = store.add_local_source("Synthetic", root).unwrap();
    assert_eq!(store.index_source(&source.id).unwrap(), 500);
}

fn seeded_vector(seed: &mut u64) -> Vec<f32> {
    (0..384)
        .map(|_| {
            *seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((*seed >> 33) as f32 / (1u64 << 31) as f32) - 0.5
        })
        .collect()
}

#[test]
fn partitioned_knn_matches_the_exhaustive_scan() {
    let data = tempdir().unwrap();
    let mut store = ReaderStore::open(data.path()).unwrap();
    let mut seed = 7u64;
    let tied = seeded_vector(&mut seed);
    let mut sources = Vec::new();
    let mut folders = Vec::new();
    for s in 0..3 {
        let notes = tempdir().unwrap();
        std::fs::create_dir(notes.path().join("sub")).unwrap();
        for d in 0..30 {
            let tags = if d % 3 == 0 { "alpha" } else { "beta" };
            let dir = if d % 4 == 0 { "sub/" } else { "" };
            std::fs::write(
                notes.path().join(format!("{dir}doc{d:02}.md")),
                format!(
                    "---\ntags: [{tags}]\n---\n# Doc {d}\nFirst part.\n\n## Second\nSecond part.\n"
                ),
            )
            .unwrap();
        }
        let source = store
            .add_local_source(
                &format!("Notes {s}"),
                SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
            )
            .unwrap()
            .id;
        store.index_source(&source).unwrap();
        for document in store.list_documents(&source).unwrap() {
            let (revision, inputs) = store.embedding_inputs(&source, &document.path).unwrap();
            // Every fifth document repeats one vector: distance ties must
            // still break by path and chunk exactly as the full scan does.
            let vectors: Vec<_> = inputs
                .iter()
                .map(|_| {
                    if document.path.ends_with("0.md") || document.path.ends_with("5.md") {
                        tied.clone()
                    } else {
                        seeded_vector(&mut seed)
                    }
                })
                .collect();
            store
                .install_embeddings(&source, &document.path, &revision, &vectors)
                .unwrap();
        }
        sources.push(source);
        folders.push(notes);
    }
    let index = rusqlite::Connection::open(data.path().join("index.sqlite")).unwrap();
    let exhaustive = |vector: &[f32], scope: &[String], filter: &str, limit: usize| {
        let bytes: Vec<u8> = vector.iter().flat_map(|x| x.to_le_bytes()).collect();
        let sql = format!("SELECT d.source_id,d.path,c.line_start,1.0-vec_distance_cosine(v.embedding,?1) AS score FROM vectors v JOIN chunks c ON c.id=v.rowid JOIN documents d ON d.id=c.document_id WHERE d.source_id IN(SELECT value FROM json_each(?2)) {filter} ORDER BY score DESC,d.path,c.id LIMIT ?3");
        index
            .prepare(&sql)
            .unwrap()
            .query_map(
                rusqlite::params![bytes, serde_json::to_string(scope).unwrap(), limit as i64],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, u32>(2)?,
                        r.get::<_, f64>(3)?,
                    ))
                },
            )
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let mut queries = vec![tied.clone()];
    for _ in 0..4 {
        queries.push(seeded_vector(&mut seed));
    }
    let scopes = [sources[..1].to_vec(), sources.clone()];
    let filters = [
        (None, ""),
        (
            Some("tag:alpha"),
            "AND EXISTS(SELECT 1 FROM document_tags t WHERE t.document_id=d.id AND t.tag='alpha')",
        ),
        (Some("path:sub"), "AND d.path GLOB '*sub*'"),
    ];
    for vector in &queries {
        for scope in &scopes {
            for (query, filter) in filters {
                for limit in [1, 3, 10, 25, 100] {
                    let hits = match query {
                        Some(query) => store.semantic_search_query(query, vector, scope, limit),
                        None => store.semantic_search(vector, scope, limit),
                    }
                    .unwrap();
                    let actual: Vec<_> = hits
                        .iter()
                        .map(|h| (h.source_id.clone(), h.path.clone(), h.line_start, h.score))
                        .collect();
                    assert_eq!(
                        actual,
                        exhaustive(vector, scope, filter, limit),
                        "{query:?} {limit}"
                    );
                    assert!(hits
                        .iter()
                        .enumerate()
                        .all(|(rank, hit)| hit.semantic_rank == Some(rank as u32 + 1)));
                }
            }
        }
    }
}

#[test]
fn v6_vectors_migrate_to_source_partitions_and_search_after_reembedding() {
    let data = tempdir().unwrap();
    let notes = tempdir().unwrap();
    std::fs::write(notes.path().join("one.md"), "# One\nA quiet animal sleeps.").unwrap();
    std::fs::write(notes.path().join("two.md"), "# Two\nDatabase storage.").unwrap();
    let mut animal = vec![0.0f32; 384];
    animal[0] = 1.0;
    let mut database = vec![0.0f32; 384];
    database[1] = 1.0;
    let source;
    {
        let mut store = ReaderStore::open(data.path()).unwrap();
        source = store
            .add_local_source(
                "Notes",
                SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
            )
            .unwrap()
            .id;
        store.index_source(&source).unwrap();
        store.record_index_job(&source, "done", None).unwrap();
    }
    // A genuine v6 index: one unpartitioned vec0 table, populated memberships
    // and parser v2 rows.
    let old = rusqlite::Connection::open(data.path().join("index.sqlite")).unwrap();
    old.execute_batch(
        "DROP TABLE vectors;
         CREATE VIRTUAL TABLE vectors USING vec0(embedding float[384] distance_metric=cosine);
         UPDATE documents SET parser_version=2;
         PRAGMA user_version=6;",
    )
    .unwrap();
    for (path, vector) in [("one.md", &animal), ("two.md", &database)] {
        let bytes: Vec<u8> = vector.iter().flat_map(|x| x.to_le_bytes()).collect();
        old.execute("INSERT INTO vectors(rowid,embedding) SELECT c.id,?2 FROM chunks c JOIN documents d ON d.id=c.document_id WHERE d.path=?1", rusqlite::params![path, bytes]).unwrap();
        old.execute("INSERT INTO embedding_memberships(chunk_id,source_id) SELECT c.id,d.source_id FROM chunks c JOIN documents d ON d.id=c.document_id WHERE d.path=?1", [path]).unwrap();
    }
    drop(old);
    assert!(ReaderStore::open_readonly(data.path()).is_err());
    let mut store = ReaderStore::open(data.path()).unwrap();
    let index = rusqlite::Connection::open(data.path().join("index.sqlite")).unwrap();
    assert_eq!(
        index
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        7
    );
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&source))
            .unwrap()
            .embedded,
        0
    );
    assert_eq!(
        store.pending_embedding_documents(&source).unwrap(),
        vec!["one.md", "two.md"]
    );
    assert_eq!(store.pending_index_jobs().unwrap(), vec![source.clone()]);
    assert!(store
        .semantic_search(&animal, std::slice::from_ref(&source), 5)
        .unwrap()
        .is_empty());
    store.index_source(&source).unwrap();
    for path in store.pending_embedding_documents(&source).unwrap() {
        let (revision, inputs) = store.embedding_inputs(&source, &path).unwrap();
        let vector = if path == "one.md" { &animal } else { &database };
        store
            .install_embeddings(
                &source,
                &path,
                &revision,
                &vec![vector.clone(); inputs.len()],
            )
            .unwrap();
    }
    let hits = store
        .semantic_search(&animal, std::slice::from_ref(&source), 5)
        .unwrap();
    assert_eq!(hits[0].path, "one.md");
    assert_eq!(hits[0].semantic_rank, Some(1));
    // Chunk deletion still removes the partitioned vector row.
    std::fs::remove_file(notes.path().join("one.md")).unwrap();
    store.index_source(&source).unwrap();
    let vectors: i64 = index
        .query_row("SELECT count(*) FROM vectors", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        vectors as u64,
        store.index_status(&[source]).unwrap().embedded
    );
    assert_eq!(vectors, 1);
}
