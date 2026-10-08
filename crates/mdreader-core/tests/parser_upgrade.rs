use mdreader_core::{ReaderStore, SourceRoot};
use rusqlite::{params, Connection};

#[test]
fn v5_capture_reparse_preserves_originals_bookmarks_admissions_and_ordinary_vectors() {
    let data = tempfile::tempdir().unwrap();
    let notes = tempfile::tempdir().unwrap();
    let capture = "<!-- mdreader-capture:v1 -->\r\n---\r\nmdreader_capture: {\"schema_version\":1,\"capture_is_not_approval\":true,\"occurrences\":[{\"session_id\":\"metadataonlytoken\"}]}\r\n---\r\n\r\n# Visible plan\r\n\r\nCobalt lighthouse body.\r\n";
    std::fs::write(notes.path().join("capture.md"), capture).unwrap();
    std::fs::write(
        notes.path().join("ordinary.md"),
        "# Ordinary\nUseful body.\n",
    )
    .unwrap();
    let source;
    let capture_revision;
    {
        let mut store = ReaderStore::open(data.path()).unwrap();
        source = store
            .add_local_source(
                "Selected",
                SourceRoot::capture(notes.path().canonicalize().unwrap()).unwrap(),
            )
            .unwrap()
            .id;
        store.index_source(&source).unwrap();
        capture_revision = store
            .read_document(&source, "capture.md", None)
            .unwrap()
            .revision;
        store
            .commit_capture(
                &"a".repeat(64),
                &source,
                "capture.md",
                "synthetic admission",
            )
            .unwrap();
        store.set_bookmark(&source, "capture.md", 8).unwrap();
        let (revision, inputs) = store.embedding_inputs(&source, "ordinary.md").unwrap();
        let mut vector = vec![0.0; 384];
        vector[0] = 1.0;
        store
            .install_embeddings(
                &source,
                "ordinary.md",
                &revision,
                &vec![vector; inputs.len()],
            )
            .unwrap();
        store.record_index_job(&source, "done", None).unwrap();
    }
    // A genuine v5 derived fixture has no parser-version column and includes
    // the metadata heading emitted by the previous parser. Durable records are
    // untouched, and the ordinary document has a real sqlite-vec membership.
    let old = Connection::open(data.path().join("index.sqlite")).unwrap();
    old.execute("INSERT INTO chunks(document_id,heading,line_start,line_end,text,embedding_text) SELECT id,'metadataonlytoken',3,4,'metadataonlytoken','metadataonlytoken' FROM documents WHERE source_id=?1 AND path='capture.md'", [&source]).unwrap();
    old.execute_batch("DROP INDEX documents_parser_version; ALTER TABLE documents DROP COLUMN parser_version; PRAGMA user_version=5;").unwrap();
    drop(old);
    assert!(ReaderStore::open_readonly(data.path()).is_err());
    let generation;
    {
        let store = ReaderStore::open(data.path()).unwrap();
        let job = store.index_job(&source).unwrap().unwrap();
        assert_eq!(job.phase, "queued");
        generation = job.generation;
        assert_eq!(store.pending_index_jobs().unwrap(), vec![source.clone()]);
        assert_eq!(
            store
                .search("metadataonlytoken", std::slice::from_ref(&source), 10)
                .unwrap()
                .len(),
            1
        );
    }
    let mut store = ReaderStore::open(data.path()).unwrap();
    assert_eq!(
        store.index_job(&source).unwrap().unwrap().generation,
        generation,
        "reopen must not invalidate a queued parser-upgrade job"
    );
    store.index_source(&source).unwrap();
    store.record_index_job(&source, "done", None).unwrap();
    assert!(store
        .search("metadataonlytoken", std::slice::from_ref(&source), 10)
        .unwrap()
        .is_empty());
    let hits = store
        .search("cobalt", std::slice::from_ref(&source), 10)
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "Visible plan");
    assert_eq!((hits[0].line_start, hits[0].line_end), (6, 8));
    assert_eq!(hits[0].revision, capture_revision);
    assert_eq!(store.bookmark(&source, "capture.md").unwrap(), Some(8));
    assert_eq!(store.capture_count().unwrap(), 1);
    assert_eq!(
        store
            .read_document(&source, "capture.md", Some(&capture_revision))
            .unwrap()
            .markdown,
        capture
    );
    assert_eq!(
        std::fs::read(notes.path().join("capture.md")).unwrap(),
        capture.as_bytes()
    );
    assert_eq!(
        store
            .index_status(std::slice::from_ref(&source))
            .unwrap()
            .embedded,
        1,
        "ordinary unchanged Markdown must not require re-embedding"
    );
    let index = Connection::open(data.path().join("index.sqlite")).unwrap();
    assert_eq!(
        index
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        6
    );
    assert_eq!(
        index
            .query_row(
                "SELECT count(*) FROM documents WHERE source_id=?1 AND parser_version=2",
                params![source],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    drop(index);
    store.index_source(&source).unwrap();
    drop(store);
    let reopened = ReaderStore::open(data.path()).unwrap();
    assert!(reopened.pending_index_jobs().unwrap().is_empty());
    assert_eq!(
        reopened.index_job(&source).unwrap().unwrap().generation,
        generation
    );
    assert_eq!(reopened.bookmark(&source, "capture.md").unwrap(), Some(8));
}
