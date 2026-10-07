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
    assert!(store.find_local_source(&root.path, &root.identity).unwrap().is_none());
    assert!(store.list_sources().unwrap().is_empty());
    let source = store.add_local_source("Notes", root.clone()).unwrap();
    store.set_source_enabled(&source.id, false).unwrap();
    let late_admission = store.add_local_source("Late import", root.clone()).unwrap();
    assert_eq!(late_admission.id, source.id);
    assert!(!late_admission.enabled);
    assert!(store.search("anything", std::slice::from_ref(&source.id), 10).is_err());
    assert!(store.find_local_source(data.path(), &root.identity).unwrap().is_none());
    store.set_source_enabled(&source.id, true).unwrap();
    assert!(store.find_local_source(&root.path, &root.identity).unwrap().unwrap().enabled);
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
