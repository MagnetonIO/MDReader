//! The app-on-platform integration proof: **MDReader really runs on CatDB.**
//!
//! Every other test in this crate reaches for a library type. This one
//! deliberately does not: it spawns the REAL shipping `catdb` binary — the
//! composition root at `src/bin/catdb.rs`, which is nothing but
//! `catdb_sdk::cli::main_with_domains(vec![mdreader_ingest::registration()])` —
//! through `env!("CARGO_BIN_EXE_catdb")`, so cargo builds and hands us the
//! exact artifact a user invokes. Calling `EmbeddedCatDb` directly would
//! prove the engine works; only spawning the binary proves the *seam* works:
//! the app's domain registering into the platform's CLI, the platform's
//! engine reading back what the app's markdown ingestor wrote.
//!
//! The chain asserted here, in order:
//!
//! 1. `catdb import` — the APP's markdown ingestor driving the PLATFORM engine.
//! 2. `catdb status` — the platform reading back what was written.
//! 3. `catdb search --explain` — hybrid retrieval (both arms), RRF + boost
//!    decomposition, and the full `chunk > section > document > file @ source`
//!    provenance chain. This is the load-bearing step.
//! 4. The database file itself is a real CatDB workspace — the platform's
//!    `operation_log` / `objects` / `morphisms` tables and the execution
//!    indexes — not an app-private format.
//! 5. `catdb validate` — the registered domain schemas pass the kernel prover.
//!
//! Hermetic and offline: the deterministic `HashEmbedder` is selected
//! explicitly with `--embedder hash`, so no model is ever downloaded or
//! loaded and the ranking is reproducible in CI.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use catdb_sdk::embed::HashEmbedder;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Driving the real binary
// ---------------------------------------------------------------------------

/// Spawn the composed `catdb` binary and return its raw exit status + stdout.
fn catdb_raw(args: &[&str]) -> (std::process::ExitStatus, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_catdb"))
        .args(args)
        .output()
        .expect("the composed `catdb` binary must be runnable");
    (
        out.status,
        String::from_utf8(out.stdout).expect("stdout is utf-8"),
    )
}

/// Run the composed `catdb` binary, asserting it exited 0, and return stdout.
#[track_caller]
fn catdb(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_catdb"))
        .args(args)
        .output()
        .expect("the composed `catdb` binary must be runnable");
    assert!(
        out.status.success(),
        "`catdb {}` exited {:?}\n--- stdout ---\n{}--- stderr ---\n{}",
        args.join(" "),
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8(out.stdout).expect("stdout is utf-8")
}

/// Run the composed `catdb` binary expecting FAILURE, and return stderr.
#[track_caller]
fn catdb_failing(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_catdb"))
        .args(args)
        .output()
        .expect("the composed `catdb` binary must be runnable");
    assert!(
        !out.status.success(),
        "`catdb {}` unexpectedly succeeded\n--- stdout ---\n{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stdout),
    );
    String::from_utf8(out.stderr).expect("stderr is utf-8")
}

// ---------------------------------------------------------------------------
// The corpus
// ---------------------------------------------------------------------------

/// The marker term the lexical arm must discriminate on: it appears in
/// `guide.md` and `notes/graph.md`, and in no other file.
const MARKER: &str = "zebrafish";

/// Files that contain [`MARKER`] — the exact set the lexical (FTS) arm may
/// return for a `MARKER` query.
const MARKER_FILES: [&str; 2] = ["guide.md", "notes/graph.md"];

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// A small deterministic vault: 3 files, 6 sections, 2 resolvable links
/// (one markdown link, one wikilink).
fn corpus(root: &Path) {
    write(
        root,
        "guide.md",
        "\
# Search Guide

Introductory prose about hybrid retrieval over a markdown corpus.

## Lexical

BM25 full-text search over chunk text finds the zebrafish marker.

## Semantic

Vector similarity over embedded chunks.

## Fusion

Reciprocal rank fusion combines both result lists into one ranking.
",
    );
    write(
        root,
        "notes/links.md",
        "# Links\n\nSee the [guide](../guide.md) and the [[graph]] note.\n",
    );
    write(
        root,
        "notes/graph.md",
        "# Graph\n\nThe wikilink graph connects zettel notes; the zebrafish marker recurs here.\n",
    );
}

/// A fresh CLI process must register sqlite-vec before Auto inspects a
/// workspace that already contains a real `vec0` table. The import, search,
/// and query calls below are three separate child processes; replacing either
/// Auto call with an explicit hash embedder would stop exercising the
/// production metadata-inspection boundary this regression protects.
#[test]
fn default_auto_reopens_a_real_vector_workspace_in_new_processes() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    corpus(&vault);
    let db = dir.path().join("catdb.db");
    let vault_arg = vault.to_str().unwrap();
    let db_arg = db.to_str().unwrap();

    catdb(&["import", vault_arg, "--db", db_arg, "--embedder", "hash"]);

    let search = catdb(&["search", MARKER, "--db", db_arg, "--json"]);
    let search_rows: serde_json::Value =
        serde_json::from_str(&search).expect("default-Auto search returns JSON");
    assert!(
        search_rows.as_array().is_some_and(|rows| !rows.is_empty()),
        "default-Auto search must serve the imported vector workspace"
    );

    let query = catdb(&["query", MARKER, "--db", db_arg, "--json"]);
    let query_rows: serde_json::Value =
        serde_json::from_str(&query).expect("default-Auto query returns JSON");
    assert!(
        query_rows.as_array().is_some_and(|rows| !rows.is_empty()),
        "default-Auto query must serve the imported vector workspace"
    );
}

// ---------------------------------------------------------------------------
// Parsing the human-readable CLI output
// ---------------------------------------------------------------------------

/// The integer immediately *preceding* `marker`, e.g. `3` in
/// `imported 3 file(s)` for the marker `" file(s)"`.
#[track_caller]
fn count_before(text: &str, marker: &str) -> u64 {
    let idx = text
        .find(marker)
        .unwrap_or_else(|| panic!("no {marker:?} in output:\n{text}"));
    let head = text[..idx].trim_end();
    let start = head
        .rfind(|c: char| !c.is_ascii_digit())
        .map_or(0, |i| i + 1);
    head[start..]
        .parse()
        .unwrap_or_else(|_| panic!("no count before {marker:?} in output:\n{text}"))
}

/// The integer immediately *following* `label`, e.g. `3` in `documents: 3`
/// for the label `"documents:"`.
#[track_caller]
fn count_after(text: &str, label: &str) -> u64 {
    let idx = text
        .find(label)
        .unwrap_or_else(|| panic!("no {label:?} in output:\n{text}"));
    text[idx + label.len()..]
        .split_whitespace()
        .next()
        .and_then(|t| t.parse().ok())
        .unwrap_or_else(|| panic!("no count after {label:?} in output:\n{text}"))
}

/// The float immediately following `label` on one line.
#[track_caller]
fn float_after(line: &str, label: &str) -> f64 {
    let idx = line
        .find(label)
        .unwrap_or_else(|| panic!("no {label:?} in line:\n{line}"));
    line[idx + label.len()..]
        .split_whitespace()
        .next()
        .and_then(|t| t.parse().ok())
        .unwrap_or_else(|| panic!("no float after {label:?} in line:\n{line}"))
}

/// A UUID that must parse and must not be nil — provenance identities are
/// real object ids, never placeholders.
#[track_caller]
fn real_uuid(token: &str, what: &str) -> Uuid {
    let id =
        Uuid::parse_str(token).unwrap_or_else(|e| panic!("{what} id {token:?} is not a uuid: {e}"));
    assert_ne!(id, Uuid::nil(), "{what} id must not be the nil uuid");
    id
}

/// One parsed `catdb search --explain` hit.
struct Hit {
    file_path: String,
    start_line: u32,
    end_line: u32,
    score: f64,
    lex_rank: Option<u32>,
    sem_rank: Option<u32>,
    explain: String,
    index: String,
    provenance: String,
}

/// `" 1. guide.md:5-8  Search Guide > Lexical"` starts a hit; the four
/// following lines are indented four spaces and never do.
fn is_hit_header(line: &str) -> bool {
    if line.starts_with("    ") {
        return false;
    }
    let trimmed = line.trim_start();
    match trimmed.find(". ") {
        Some(i) if i > 0 => trimmed[..i].chars().all(|c| c.is_ascii_digit()),
        _ => false,
    }
}

/// The one line in `block` starting with `prefix` (after indentation).
#[track_caller]
fn field(block: &[&str], prefix: &str) -> String {
    block
        .iter()
        .find(|l| l.trim_start().starts_with(prefix))
        .unwrap_or_else(|| panic!("no {prefix:?} line in hit block:\n{}", block.join("\n")))
        .trim_start()
        .to_string()
}

/// Parse every hit out of a `catdb search --explain` run.
#[track_caller]
fn parse_hits(out: &str) -> Vec<Hit> {
    let lines: Vec<&str> = out.lines().collect();
    let starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| is_hit_header(l))
        .map(|(i, _)| i)
        .collect();
    starts
        .iter()
        .enumerate()
        .map(|(n, &start)| {
            let end = starts.get(n + 1).copied().unwrap_or(lines.len());
            let block = &lines[start..end];
            // Header: `{rank}. {path}:{start}-{end}  {breadcrumb}`
            let location = lines[start]
                .split_whitespace()
                .nth(1)
                .unwrap_or_else(|| panic!("no location in hit header:\n{}", lines[start]));
            let (file_path, range) = location
                .rsplit_once(':')
                .unwrap_or_else(|| panic!("no line range in location {location:?}"));
            let (from, to) = range
                .split_once('-')
                .unwrap_or_else(|| panic!("no line range in location {location:?}"));
            let score_line = field(block, "score ");
            Hit {
                file_path: file_path.to_string(),
                start_line: from.parse().expect("start line is a number"),
                end_line: to.parse().expect("end line is a number"),
                score: float_after(&score_line, "score "),
                lex_rank: score_line
                    .find("lex#")
                    .and_then(|i| score_line[i + 4..].split_whitespace().next()?.parse().ok()),
                sem_rank: score_line
                    .find("sem#")
                    .and_then(|i| score_line[i + 4..].split_whitespace().next()?.parse().ok()),
                explain: field(block, "explain: "),
                index: field(block, "index: "),
                provenance: field(block, "provenance: "),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The proof
// ---------------------------------------------------------------------------

#[test]
fn mdreader_corpus_round_trips_through_the_composed_catdb_binary() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    corpus(&vault);
    let db = dir.path().join("catdb.db");
    // The engine canonicalizes a source root before storing it, so that is
    // the form `status` and the provenance chain must report.
    let vault_arg = vault.to_str().unwrap().to_string();
    let vault_root = vault.canonicalize().unwrap();
    let vault_root = vault_root.to_str().unwrap().to_string();
    let db_arg = db.to_str().unwrap().to_string();

    // -- 1. import: the app's markdown ingestor driving the platform engine --
    let out = catdb(&["import", &vault_arg, "--db", &db_arg, "--embedder", "hash"]);
    assert_eq!(count_before(&out, " file(s)"), 3, "output:\n{out}");
    assert_eq!(
        count_before(&out, " section(s) upserted"),
        6,
        "output:\n{out}"
    );
    assert_eq!(
        count_before(&out, " chunk(s) embedded"),
        6,
        "output:\n{out}"
    );
    assert_eq!(
        count_before(&out, " link(s) resolved"),
        2,
        "both the markdown link and the wikilink resolve; output:\n{out}"
    );
    assert!(db.is_file(), "import must create the workspace at --db");

    // -- 2. status: the platform reads back what the app wrote --------------
    let out = catdb(&["status", "--db", &db_arg]);
    assert_eq!(count_after(&out, "documents:"), 3, "output:\n{out}");
    assert_eq!(count_after(&out, "sections:"), 6, "output:\n{out}");
    assert_eq!(count_after(&out, "chunks:"), 6, "output:\n{out}");
    assert_eq!(count_after(&out, "links:"), 2, "output:\n{out}");
    assert!(
        out.contains(&format!("active index: {}", HashEmbedder::MODEL_NAME)),
        "status must name the active index's embedding model; output:\n{out}"
    );
    assert!(
        out.contains(mdreader_ingest::CHUNKER_VERSION),
        "status must name the app's chunker version ({}); output:\n{out}",
        mdreader_ingest::CHUNKER_VERSION
    );
    assert!(
        out.contains(&vault_root),
        "status must report the imported source root {vault_root}; output:\n{out}"
    );

    // -- 3. search --explain: hybrid retrieval with provenance --------------
    // THE load-bearing step. `MARKER` is in exactly two of the three files,
    // so the lexical arm has something to discriminate on.
    let out = catdb(&[
        "search",
        MARKER,
        "--db",
        &db_arg,
        "--embedder",
        "hash",
        "--explain",
    ]);
    let hits = parse_hits(&out);
    assert!(!hits.is_empty(), "the query must match; output:\n{out}");

    // BOTH retrieval arms contribute, and they fuse: at least one hit was
    // found by lexical AND semantic retrieval. A silently dropped arm — the
    // classic hybrid-retrieval regression — fails right here.
    assert!(
        hits.iter().any(|h| h.lex_rank.is_some()),
        "the lexical (FTS) arm must contribute; output:\n{out}"
    );
    assert!(
        hits.iter().any(|h| h.sem_rank.is_some()),
        "the semantic (vector) arm must contribute; output:\n{out}"
    );
    assert!(
        hits.iter()
            .any(|h| h.lex_rank.is_some() && h.sem_rank.is_some()),
        "reciprocal-rank fusion must merge a hit found by BOTH arms; output:\n{out}"
    );
    assert_eq!(
        hits.len(),
        6,
        "every chunk in the corpus is retrievable; output:\n{out}"
    );

    for hit in &hits {
        // (a) The hit carries a file path that resolves to a real file in
        //     the corpus, and a sane 1-based line range.
        assert!(
            vault.join(&hit.file_path).is_file(),
            "hit path {:?} must point at a real file in the corpus; output:\n{out}",
            hit.file_path
        );
        assert!(
            hit.start_line >= 1 && hit.end_line >= hit.start_line,
            "hit {:?} has a broken line range {}-{}",
            hit.file_path,
            hit.start_line,
            hit.end_line
        );

        // (b) The FULL provenance chain: chunk > section > document > file
        //     @ source, every id a real object identity, the file agreeing
        //     with the hit and the source root with the import.
        let chain = hit
            .provenance
            .strip_prefix("provenance: ")
            .unwrap_or_else(|| panic!("malformed provenance line: {}", hit.provenance));
        let links: Vec<&str> = chain.split(" > ").collect();
        assert_eq!(
            links.len(),
            4,
            "provenance must be a 4-link chain, got: {chain}"
        );
        let chunk_id = real_uuid(
            links[0]
                .strip_prefix("chunk ")
                .unwrap_or_else(|| panic!("provenance must start at the chunk, got: {}", links[0])),
            "chunk",
        );
        let section_id = real_uuid(
            links[1]
                .strip_prefix("section ")
                .unwrap_or_else(|| panic!("chunk must lead to a section, got: {}", links[1])),
            "section",
        );
        let document_id = real_uuid(
            links[2]
                .strip_prefix("document ")
                .unwrap_or_else(|| panic!("section must lead to a document, got: {}", links[2])),
            "document",
        );
        assert_eq!(
            BTreeSet::from([chunk_id, section_id, document_id]).len(),
            3,
            "the chain's three levels must be distinct objects: {chain}"
        );
        let (file, source_root) = links[3]
            .rsplit_once(" @ ")
            .unwrap_or_else(|| panic!("document must lead to a file @ source, got: {}", links[3]));
        assert_eq!(
            file, hit.file_path,
            "the provenance file must be the hit's file"
        );
        assert_eq!(
            source_root, vault_root,
            "the chain must terminate at the imported source root"
        );

        // (c) The explain line decomposes the score: rrf + every boost = final.
        let rrf = float_after(&hit.explain, "rrf ");
        let title = float_after(&hit.explain, "+ title ");
        let path = float_after(&hit.explain, "+ path ");
        let recency = float_after(&hit.explain, "+ recency ");
        let graph = float_after(&hit.explain, "+ graph ");
        let total = float_after(&hit.explain, "= ");
        assert!(
            rrf > 0.0,
            "a fused hit must carry a positive RRF component: {}",
            hit.explain
        );
        assert!(
            (rrf + title + path + recency + graph - total).abs() < 1e-3,
            "the decomposition must sum to the reported total: {}",
            hit.explain
        );
        assert!(
            (total - hit.score).abs() < 1e-3,
            "the explain total ({total}) must be the hit's score ({}): {}",
            hit.score,
            hit.explain
        );

        // (d) The scoring names the index version it was served from.
        assert!(
            hit.index.contains(HashEmbedder::MODEL_NAME),
            "explain must name the serving index's model: {}",
            hit.index
        );
        real_uuid(
            hit.index
                .rsplit_once('(')
                .and_then(|(_, v)| v.strip_suffix(')'))
                .unwrap_or_else(|| panic!("explain must name the index version: {}", hit.index)),
            "index version",
        );
    }

    // The lexical arm really matched corpus TEXT: exactly the files that
    // contain the marker, no more and no fewer.
    let lexical: BTreeSet<&str> = hits
        .iter()
        .filter(|h| h.lex_rank.is_some())
        .map(|h| h.file_path.as_str())
        .collect();
    assert_eq!(
        lexical,
        BTreeSet::from(MARKER_FILES),
        "the FTS arm must return exactly the files containing {MARKER:?}; output:\n{out}"
    );

    // Hits are returned in descending fused-score order.
    for pair in hits.windows(2) {
        assert!(
            pair[0].score >= pair[1].score,
            "hits must be ranked by descending score; output:\n{out}"
        );
    }

    // -- 4. the file on disk IS a CatDB workspace ---------------------------
    // Not an app-private format: the platform's operation log, its
    // materialized object/morphism view, its schema catalogue, and the
    // execution indexes are all present in the same SQLite file.
    let conn = rusqlite::Connection::open(&db).expect("the workspace must open as SQLite");
    let tables: BTreeSet<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for table in [
        "operation_log",
        "operation_log_anchor",
        "objects",
        "morphisms",
        "schema_defs",
        "chunk_fts",
    ] {
        assert!(
            tables.contains(table),
            "the workspace must carry the platform's `{table}` table; found: {tables:?}"
        );
    }
    // The per-index-version vector table, `chunk_vec_v<rowid>` (sqlite-vec
    // shadow tables carry a further `_suffix`, so require an all-digit tail).
    let vec_table = tables
        .iter()
        .find(|t| {
            t.strip_prefix("chunk_vec_v")
                .is_some_and(|tail| !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()))
        })
        .unwrap_or_else(|| {
            panic!("the workspace must carry a chunk_vec_v* vector table; found: {tables:?}")
        });

    let count = |sql: &str| -> i64 { conn.query_row(sql, [], |row| row.get(0)).unwrap() };
    let ops = count("SELECT COUNT(*) FROM operation_log");
    assert!(
        ops > 0,
        "the import must have been recorded in the operation log"
    );
    assert!(
        count("SELECT COUNT(*) FROM objects") > 0,
        "the materialized view must carry objects"
    );
    assert!(
        count("SELECT COUNT(*) FROM morphisms") > 0,
        "the materialized view must carry morphisms"
    );
    // The replay-chain anchor agrees with the log it anchors.
    assert_eq!(
        count("SELECT op_count FROM operation_log_anchor"),
        ops,
        "the operation-log anchor must count the log it anchors"
    );

    // Existence proves only that migrations ran. The EXECUTION INDEXES must
    // additionally be POPULATED — an empty `chunk_fts` means lexical search is
    // backed by nothing, and an empty vector table means the semantic arm has
    // nothing to serve. Both are the failures that actually matter, and both
    // survive a table-existence check. Assert the stronger property: each index
    // carries exactly one entry per stored chunk.
    let chunks = count("SELECT COUNT(*) FROM chunks");
    assert_eq!(chunks, 6, "the workspace must carry the corpus's chunks");
    // `chunk_fts` is an EXTERNAL-CONTENT fts5 table (`content='chunks'`), so
    // `SELECT COUNT(*) FROM chunk_fts` reads the CONTENT table and returns the
    // chunk count even when the index is completely empty — a tautology, not a
    // check (verified: emptying the index leaves that count at 6). The index
    // itself lives in the `_docsize` shadow table, one row per indexed chunk.
    assert_eq!(
        count("SELECT COUNT(*) FROM chunk_fts_docsize"),
        chunks,
        "the FTS index must carry one entry per chunk, not merely exist"
    );
    // And it must actually RESOLVE: the marker term matches exactly the chunks
    // that contain it (one per marker file in this corpus).
    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) FROM chunk_fts WHERE chunk_fts MATCH '{MARKER}'"
        )),
        MARKER_FILES.len() as i64,
        "the FTS index must resolve {MARKER:?} to the chunks that contain it"
    );
    // The vec0 table itself is only queryable with the sqlite-vec extension
    // loaded, which the platform registers behind a private `unsafe` FFI
    // helper this crate cannot call (and `unsafe_code` is denied workspace
    // wide). Its `_rowids` shadow table is a PLAIN table holding exactly one
    // live row per stored vector — verified against deletion: dropping a
    // one-chunk file from the corpus takes the count from 6 to 5, so these are
    // live vectors, not tombstones.
    assert_eq!(
        count(&format!("SELECT COUNT(*) FROM {vec_table}_rowids")),
        chunks,
        "the vector index must carry one embedding per chunk, not merely exist"
    );
    // The catalogue really registered the index version those entries belong to.
    assert_eq!(
        count("SELECT COUNT(*) FROM index_versions WHERE state = 'active'"),
        1,
        "the import must leave exactly one active index version"
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM sources"),
        1,
        "the import must register its source root"
    );
    // The realized domain tables agree with what `status` reported above.
    assert_eq!(count("SELECT COUNT(*) FROM documents"), 3);
    assert_eq!(count("SELECT COUNT(*) FROM sections"), 6);
    assert_eq!(count("SELECT COUNT(*) FROM links"), 2);
    // NOTE: `schema_defs` is deliberately NOT row-counted. It is legitimately
    // EMPTY after a successful import: `CatalogStorage::put_schema_def` is
    // declared and implemented by both storage backends but has no caller
    // outside those crates' own tests — the engine holds registered domain
    // schemas in memory and proves them at open (`catdb validate`, step 5)
    // without ever persisting them to the catalogue table. Asserting
    // non-emptiness here would fail against correct behaviour; its presence
    // above is a migration check only.

    // -- 5. validate: the registered domain schemas pass the prover ---------
    // (Asserted in full by `registered_domains_pass_the_schema_prover`; here
    // only that it holds for the very database this test just built.)
    let out = catdb(&["validate"]);
    assert!(out.contains("0 violation(s)"), "output:\n{out}");
}

#[test]
fn registered_domains_pass_the_schema_prover() {
    // `catdb validate` reads no workspace data — it proves the MDReader
    // domain the composition root registers: schema well-formedness, mapping
    // totality, and the path-equation laws via the egg prover. Exit 0 means
    // every one of them passed.
    let out = catdb(&["validate"]);
    assert!(
        out.contains("schema validation of 1 registered domain(s):"),
        "the binary must compose exactly the MDReader domain; output:\n{out}"
    );
    assert!(
        !out.contains("FAIL"),
        "no schema, mapping or equation may fail; output:\n{out}"
    );
    assert!(
        out.lines().filter(|l| l.contains("PASS")).count() >= 9,
        "every registered schema, mapping and equation must be reported; output:\n{out}"
    );
    assert!(out.contains("0 violation(s)"), "output:\n{out}");
}

#[test]
fn the_binary_refuses_a_workspace_that_was_never_imported() {
    // The guard on the guard: the assertions above are only worth something
    // if the same commands FAIL when the integration is broken. Searching a
    // database that does not exist must exit non-zero with a usable message
    // — never exit 0 with an empty result that a lenient assertion could
    // mistake for success.
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("never-imported.db");
    let missing_arg = missing.to_str().unwrap().to_string();
    let err = catdb_failing(&["search", MARKER, "--db", &missing_arg]);
    assert!(
        err.contains("no database at"),
        "stderr must say the workspace is missing; got:\n{err}"
    );
    let err = catdb_failing(&["status", "--db", &missing_arg]);
    assert!(
        err.contains("no database at"),
        "stderr must say the workspace is missing; got:\n{err}"
    );

    // And an EMPTY (imported-but-corpus-less) workspace returns no hits at
    // all on the lexical arm — the marker only ever appears because the
    // corpus was really ingested.
    let empty_vault = dir.path().join("empty");
    std::fs::create_dir_all(&empty_vault).unwrap();
    let empty_arg = empty_vault.to_str().unwrap().to_string();
    let db = dir.path().join("empty.db");
    let db_arg = db.to_str().unwrap().to_string();
    let out = catdb(&["import", &empty_arg, "--db", &db_arg, "--embedder", "hash"]);
    assert_eq!(
        count_before(&out, " chunk(s) embedded"),
        0,
        "output:\n{out}"
    );
    // "No hits" must be the binary REPORTING no results, not the binary
    // erroring out, changing its output format, or this file's parser simply
    // failing to match. Assert the real user-visible behaviour: a successful
    // exit AND the human message, before trusting the parse.
    let (status, out) = catdb_raw(&[
        "search",
        MARKER,
        "--db",
        &db_arg,
        "--embedder",
        "hash",
        "--lexical-only",
        "--explain",
    ]);
    assert!(
        status.success(),
        "an empty workspace is not an error: searching it must exit 0, got {:?}; output:\n{out}",
        status.code()
    );
    assert!(
        out.contains(&format!("no hits for {MARKER:?}")),
        "the binary must REPORT no hits in its human output; output:\n{out}"
    );
    assert!(
        parse_hits(&out).is_empty(),
        "an empty workspace must yield no hits; output:\n{out}"
    );
}
