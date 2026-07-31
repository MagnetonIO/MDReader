//! Assembling a `Document` out of three real sources, and asking it which
//! source answered each field.
//!
//! Everything here runs against the committed `fixtures/corpus` (or a real
//! temporary git repository). Every field assertion names the source that
//! answered it, and every test that reads hashes the tree before and after.

mod federation_support;

use catdb_sdk::connectors::{
    Availability, DataSource, FilesystemSource, FrontmatterSource, GitSource, SourceError, SourceId,
};
use catdb_sdk::federation::{
    Execution, ExecutionPolicy, Executor, Federated, Outcome, Ran, SourceReport,
};
use catdb_sdk::validation::EggProver;
use federation_support::{
    corpus, fs_modified_ms, git_author, git_commit_seconds, tree_digest, FIXTURE_AUTHOR,
    FIXTURE_COMMIT_SECONDS,
};
use mdreader_domain::federated::{Document, Federation, FederationError, ProviderVerdict};

fn prover() -> EggProver {
    EggProver::default()
}

/// Assemble over the corpus, asserting the tree is untouched by the read.
///
/// Every corpus-reading test goes through this: the non-destructive premise is
/// not one test's job, it is a property of every read, so it is checked on
/// every read.
async fn assemble(scope: &str) -> Vec<Document> {
    let root = corpus();
    let before = tree_digest(&root);
    let fs = FilesystemSource::new(&root);
    let git = GitSource::open(&root);
    let front = FrontmatterSource::new(&root);
    let federation =
        Federation::new(vec![&fs, &git, &front], &prover()).expect("the federation declares");
    let context = federation_support::read_context();
    let documents = federation
        .assemble(&federation.scope(scope).expect("scope validates"), &context)
        .await
        .expect("the corpus assembles");
    assert_eq!(
        before,
        tree_digest(&root),
        "assembling modified the corpus — the sources must be read-only"
    );
    documents
}

async fn one(scope: &str, path: &str) -> Document {
    assemble(scope)
        .await
        .into_iter()
        .find(|document| document.path() == path)
        .unwrap_or_else(|| panic!("no document for `{path}`"))
}

/// The source that answered a field, by name.
fn answered_by(document: &Document, field: &str) -> String {
    document
        .source_of(field)
        .unwrap_or_else(|| {
            panic!(
                "`{field}` was not answered:\n{}",
                document.explain().join("\n")
            )
        })
        .to_string()
}

// --- the decisive assertion -------------------------------------------------

/// One document, six fields, four different physical origins — and the
/// document can say which source answered each one, by name.
#[tokio::test]
async fn a_document_from_the_corpus_names_the_source_that_answered_every_field() {
    let rel = "docs/reference/headings-duplicate.md";
    let document = one("path:docs/**", rel).await;

    let sourced: Vec<(&str, String)> = ["path", "title", "author", "modifiedAt", "size", "body"]
        .into_iter()
        .map(|field| (field, answered_by(&document, field)))
        .collect();
    assert_eq!(
        sourced,
        [
            ("path", "filesystem".to_string()),
            // The YAML block, read by the frontmatter adapter.
            ("title", "frontmatter".to_string()),
            // No YAML `author:` in this file, so the commit author answers —
            // a genuinely different physical source from the one above.
            ("author", "git".to_string()),
            ("modifiedAt", "filesystem".to_string()),
            ("size", "filesystem".to_string()),
            ("body", "frontmatter".to_string()),
        ],
        "{}",
        document.explain().join("\n")
    );

    // And the values really are the ones those sources hold.
    let root = corpus();
    assert_eq!(document.text("title"), Some("Duplicate Headings"));
    assert_eq!(
        document.text("author"),
        Some(git_author(&root, rel).as_str())
    );
    assert_eq!(
        document
            .value("modifiedAt")
            .and_then(serde_json::Value::as_i64),
        Some(fs_modified_ms(&root, rel))
    );
    assert_eq!(
        document.value("size").and_then(serde_json::Value::as_i64),
        Some(i64::try_from(std::fs::metadata(root.join(rel)).expect("stat").len()).expect("fits"))
    );
}

/// The same file seen by three adapters is ONE document — and the three rows
/// are still there, verbatim, each linked by its own identity claim.
#[tokio::test]
async fn one_file_seen_by_three_adapters_is_one_document_with_three_intact_observations() {
    let document = one("path:docs/**", "docs/reference/headings-duplicate.md").await;

    let mut sources: Vec<&str> = document
        .observations()
        .iter()
        .map(|row| row.source.as_str())
        .collect();
    sources.sort();
    assert_eq!(sources, ["filesystem", "frontmatter", "git"]);

    // Nothing was merged: each row still carries exactly what its own adapter
    // observed, under its own key.
    for row in document.observations() {
        assert_eq!(row.key, document.path());
        assert_eq!(row.entity, "doc.Document");
        assert_eq!(
            row.value.get("path").and_then(serde_json::Value::as_str),
            Some(document.path())
        );
    }
    let frontmatter = document
        .observations()
        .iter()
        .find(|row| row.source.as_str() == "frontmatter")
        .expect("frontmatter observed it");
    assert!(
        frontmatter.value.get("size").is_none(),
        "the frontmatter row must not have grown a filesystem attribute"
    );

    // One claim per observation. Both ends of every claim keep their identity:
    // three distinct source records, one canonical document.
    assert_eq!(document.claims().len(), 3);
    let canonical: std::collections::BTreeSet<_> = document
        .claims()
        .iter()
        .map(catdb_sdk::core::IdentityClaim::canonical)
        .collect();
    let observed: std::collections::BTreeSet<_> = document
        .claims()
        .iter()
        .map(catdb_sdk::core::IdentityClaim::source)
        .collect();
    assert_eq!(canonical.len(), 1, "all three identify into one document");
    assert_eq!(observed.len(), 3, "and each source record keeps its own id");
    assert!(canonical.is_disjoint(&observed));
    for claim in document.claims() {
        assert_eq!(
            claim.confidence(),
            catdb_sdk::core::IdentityConfidence::Known,
            "path equality within one root is a key by construction"
        );
    }
}

// --- declared precedence ----------------------------------------------------

/// A document with BOTH a YAML `title:` and a different first H1 takes the
/// declared winner — the YAML field.
#[tokio::test]
async fn a_yaml_title_wins_over_a_differing_first_h1() {
    let document = one("path:docs/**", "docs/reference/headings-duplicate.md").await;
    assert_eq!(document.text("title"), Some("Duplicate Headings"));
    assert_ne!(
        document.text("title"),
        Some("Overview"),
        "`# Overview` is this file's first H1, and it must not win"
    );
    assert_eq!(answered_by(&document, "title"), "frontmatter");
}

/// A document with no YAML `title:` falls back to its first H1 — the second
/// half of the same adapter-declared chain.
#[tokio::test]
async fn a_document_with_no_yaml_title_falls_back_to_its_first_h1() {
    let document = one("path:docs/**", "docs/arch/catdb-core.md").await;
    assert_eq!(document.text("title"), Some("catdb-core"));
    assert_eq!(answered_by(&document, "title"), "frontmatter");
}

/// `author` is the cross-source case: a declared YAML author outranks the
/// commit author, and the loser is recorded rather than forgotten.
#[tokio::test]
async fn a_declared_yaml_author_outranks_the_commit_author() {
    let rel = "frontmatter/typical.md";
    let document = one("path:frontmatter/typical.md", rel).await;
    assert_eq!(document.text("author"), Some("M. Long"));
    assert_eq!(answered_by(&document, "author"), "frontmatter");

    let author = document.field("author").expect("author is declared");
    let outranked: Vec<String> = author
        .outranked()
        .iter()
        .map(|provider| provider.source.to_string())
        .collect();
    assert_eq!(
        outranked,
        ["git"],
        "git observed a commit author and was outranked, not ignored"
    );
    // The outranked value is still on the record.
    assert_eq!(
        author.verdicts[1].1.observed().and_then(|v| v.as_str()),
        Some(git_author(&corpus(), rel).as_str())
    );
}

/// Where no YAML author was declared, the commit author answers — and the
/// record says the higher-precedence source was silent.
#[tokio::test]
async fn the_commit_author_answers_where_the_document_declared_none() {
    let rel = "docs/arch/catdb-core.md";
    let document = one("path:docs/arch/**", rel).await;
    assert_eq!(answered_by(&document, "author"), "git");
    assert_eq!(
        document.text("author"),
        Some(git_author(&corpus(), rel).as_str())
    );

    let author = document.field("author").expect("author is declared");
    let silent: Vec<String> = author
        .silent_above()
        .iter()
        .map(|provider| provider.to_string())
        .collect();
    assert_eq!(silent, ["frontmatter.author"]);
    assert_eq!(author.verdicts[0].1, ProviderVerdict::ObservedNothing);
    assert!(author.explain().contains("declared first but had nothing"));
}

/// `modifiedAt` is answered by both sources and the declared order decides:
/// the filesystem mtime wins over the commit timestamp, which is genuinely a
/// different number.
#[tokio::test]
async fn the_filesystem_mtime_outranks_the_commit_timestamp() {
    let rel = "docs/arch/catdb-core.md";
    let document = one("path:docs/arch/**", rel).await;
    let root = corpus();
    assert_eq!(answered_by(&document, "modifiedAt"), "filesystem");
    assert_eq!(
        document
            .value("modifiedAt")
            .and_then(serde_json::Value::as_i64),
        Some(fs_modified_ms(&root, rel))
    );

    let modified = document.field("modifiedAt").expect("declared");
    let outranked: Vec<String> = modified
        .outranked()
        .iter()
        .map(|provider| provider.source.to_string())
        .collect();
    assert_eq!(outranked, ["git"]);
    assert_eq!(
        modified.verdicts[1].1.observed().and_then(|v| v.as_i64()),
        Some(git_commit_seconds(&root, rel) * 1000),
        "git's own answer is kept even though it lost"
    );
    assert_ne!(
        modified.value(),
        modified.verdicts[1].1.observed(),
        "the two sources must genuinely disagree, or this proves nothing"
    );
}

// --- absence ----------------------------------------------------------------

/// A field no source can supply is ABSENT. Not empty, not the filename, not
/// the epoch.
#[tokio::test]
async fn a_title_no_source_can_supply_is_absent_rather_than_defaulted() {
    let document = one("path:edge-cases/empty.md", "edge-cases/empty.md").await;
    let title = document.field("title").expect("title is declared");
    assert!(title.is_absent(), "{}", title.explain());
    assert_eq!(document.value("title"), None);
    assert_eq!(document.text("title"), None);
    assert_eq!(document.source_of("title"), None);
    // Every declared provider is on the record, and each says why it had
    // nothing.
    let asked: Vec<(String, &str)> = title
        .verdicts
        .iter()
        .map(|(provider, verdict)| (provider.to_string(), verdict.as_str()))
        .collect();
    assert_eq!(
        asked,
        [
            ("frontmatter.title".to_string(), "observed-nothing"),
            ("google-drive.name".to_string(), "not-asked"),
            ("dropbox.name".to_string(), "not-asked"),
        ]
    );
    assert!(title.explain().starts_with("`title` is absent"));

    // The file IS there and IS readable — absence is about the title, not
    // about the document.
    assert_eq!(document.text("body"), Some(""));
    assert_eq!(document.path(), "edge-cases/empty.md");
}

/// An author neither the document nor git declares is absent, with both
/// providers named. `author` has no fallback anywhere in the chain: that is a
/// fact to represent, not a gap to fill.
#[tokio::test]
async fn an_author_neither_source_can_supply_is_absent_with_both_providers_named() {
    let repo = federation_support::git_fixture();
    let root = repo.path();
    let before = tree_digest(root);
    let status = federation_support::git_status(root);

    let fs = FilesystemSource::new(root);
    let git = GitSource::open(root);
    let front = FrontmatterSource::new(root);
    let federation = Federation::new(vec![&fs, &git, &front], &prover()).expect("declares");
    let context = federation_support::read_context();
    let documents = federation
        .assemble(&federation.scope("").expect("scope validates"), &context)
        .await
        .expect("the fixture assembles");

    // `docs/staged.md` is in the index but has never been committed, so git
    // genuinely has no author and no timestamp for it.
    let staged = documents
        .iter()
        .find(|document| document.path() == "docs/staged.md")
        .expect("the staged file is there");
    let author = staged.field("author").expect("author is declared");
    assert!(author.is_absent(), "{}", author.explain());
    let asked: Vec<(String, &str)> = author
        .verdicts
        .iter()
        .map(|(provider, verdict)| (provider.to_string(), verdict.as_str()))
        .collect();
    assert_eq!(
        asked,
        [
            ("frontmatter.author".to_string(), "observed-nothing"),
            ("git.author".to_string(), "observed-nothing"),
        ]
    );
    // Its modifiedAt still resolves — from the filesystem, which did observe
    // it. Absence is per field, not per document.
    assert_eq!(
        staged.source_of("modifiedAt").map(ToString::to_string),
        Some("filesystem".to_string())
    );

    // A committed file in the same repository shows the other half: git
    // answers with the pinned fixture author and timestamp.
    let committed = documents
        .iter()
        .find(|document| document.path() == "docs/committed.md")
        .expect("the committed file is there");
    assert_eq!(committed.text("author"), Some(FIXTURE_AUTHOR));
    assert_eq!(
        committed.source_of("author").map(ToString::to_string),
        Some("git".to_string())
    );
    let modified = committed.field("modifiedAt").expect("declared");
    assert_eq!(
        modified.verdicts[1].1.observed().and_then(|v| v.as_i64()),
        Some(FIXTURE_COMMIT_SECONDS * 1000)
    );

    // And the declared file shows the frontmatter half in the same repository.
    let declared = documents
        .iter()
        .find(|document| document.path() == "docs/declared.md")
        .expect("the declared file is there");
    assert_eq!(declared.text("author"), Some("Declared Author"));
    assert_eq!(
        declared.source_of("author").map(ToString::to_string),
        Some("frontmatter".to_string())
    );
    assert_eq!(declared.text("title"), Some("Declared Title"));

    assert_eq!(
        before,
        tree_digest(root),
        "the fixture repository was modified"
    );
    assert_eq!(
        status,
        federation_support::git_status(root),
        "reading changed what git tracks"
    );
}

/// A document one source cannot see at all is not the same as one it saw and
/// had nothing for, and the record keeps them apart.
#[tokio::test]
async fn a_source_that_never_saw_the_document_says_so_distinctly() {
    let document = one("path:assets/**", "assets/logo.png").await;
    let title = document.field("title").expect("title is declared");
    assert_eq!(title.verdicts[0].1, ProviderVerdict::DidNotSeeDocument);
    assert!(title.is_absent());
    // The filesystem did see it, and answered what it observes.
    assert_eq!(answered_by(&document, "size"), "filesystem");
    assert_eq!(answered_by(&document, "path"), "filesystem");
}

// --- unavailability and failure --------------------------------------------

/// A source that is not here declares itself unavailable, is never read, and
/// its providers are recorded as never asked — which is a different thing from
/// having nothing to say.
#[tokio::test]
async fn an_unavailable_source_is_never_asked_and_never_forged_into_an_answer() {
    let tree = federation_support::tree(&[(
        "note.md",
        "---\ntitle: Outside Any Repository\n---\n\n# Note\n\nbody\n",
    )]);
    let root = tree.path();
    let fs = FilesystemSource::new(root);
    // Repository discovery is intentionally deferred for existing
    // directories. A missing root is the declaration-time unavailable case.
    let git = GitSource::open(root.join("missing-git-root"));
    let front = FrontmatterSource::new(root);
    assert!(matches!(
        git.capabilities().availability(),
        Availability::Unavailable { .. }
    ));

    let federation = Federation::new(vec![&fs, &git, &front], &prover()).expect("declares");
    let unavailable: Vec<String> = federation
        .unavailable()
        .iter()
        .map(|(id, _)| id.to_string())
        .collect();
    assert_eq!(unavailable, ["git"]);
    assert!(federation.unavailable()[0].1.contains("not a directory"));

    let context = federation_support::read_context();
    let documents = federation
        .assemble(&federation.scope("").expect("scope validates"), &context)
        .await
        .expect("a federation with an unavailable source still assembles");
    let note = &documents[0];
    let author = note.field("author").expect("author is declared");
    assert!(author.is_absent());
    assert_eq!(author.verdicts[0].1, ProviderVerdict::ObservedNothing);
    match &author.verdicts[1].1 {
        ProviderVerdict::NotAsked { reason } => {
            assert!(
                reason.contains("not a directory"),
                "the reason must be the source's own: {reason}"
            );
        }
        other => panic!("git must be recorded as never asked, got {other:?}"),
    }
    // Everything the remaining sources DO observe is still answered.
    assert_eq!(note.text("title"), Some("Outside Any Repository"));
    assert_eq!(answered_by(note, "modifiedAt"), "filesystem");
}

/// A source that cannot complete a read fails the whole assembly. A silently
/// shorter answer is indistinguishable from a correct one.
#[tokio::test]
async fn a_malformed_source_fails_the_assembly_rather_than_shrinking_it() {
    let root = corpus();
    let before = tree_digest(&root);
    let fs = FilesystemSource::new(&root);
    let git = GitSource::open(&root);
    let front = FrontmatterSource::new(&root);
    let federation = Federation::new(vec![&fs, &git, &front], &prover()).expect("declares");
    let scope = federation.scope("path:frontmatter/**").expect("scope");
    let context = federation_support::read_context();

    match federation.assemble(&scope, &context).await {
        Ok(documents) => panic!(
            "expected a typed refusal, got {} documents — a malformed frontmatter block must \
             not be quietly skipped",
            documents.len()
        ),
        Err(FederationError::Source(SourceError::Malformed { path, attr, .. })) => {
            assert_eq!(path, "frontmatter/malformed-frontmatter.md");
            assert_eq!(attr, "frontmatter");
        }
        Err(other) => panic!("expected a Malformed refusal, got {other}"),
    }
    assert_eq!(
        before,
        tree_digest(&root),
        "a FAILED read must not have modified anything either"
    );
}

// --- scope ------------------------------------------------------------------

/// The scope narrows by the natural key. Anything else is refused rather than
/// re-implementing the connector's pushed/residual split here.
#[test]
fn a_scope_may_only_narrow_the_natural_key() {
    let root = corpus();
    let fs = FilesystemSource::new(&root);
    let front = FrontmatterSource::new(&root);
    let federation = Federation::new(vec![&fs, &front], &prover()).expect("declares");

    federation.scope("").expect("the whole root is a scope");
    federation
        .scope("path:docs/**")
        .expect("a key glob is a scope");
    match federation.scope("title:Code*") {
        Err(FederationError::ScopeNotPath { attr }) => assert_eq!(attr, "title"),
        other => panic!("expected a refusal naming `title`, got {other:?}"),
    }
}

/// Two federations over the same unchanged corpus produce the same documents,
/// the same attributions and the same identity claims.
#[tokio::test]
async fn assembling_twice_over_an_unchanged_corpus_gives_the_same_answer() {
    let first = assemble("path:docs/guides/**").await;
    let second = assemble("path:docs/guides/**").await;
    assert!(!first.is_empty());
    assert_eq!(first, second);
}

// --- attributing an answer the executor produced ----------------------------

/// Attributing the federating executor's own answer gives exactly what reading
/// the sources here gives.
///
/// The two paths must not drift: `assemble` exists for callers who need no
/// execution report, `assemble_from` for callers who do, and a difference
/// between them would mean the reported execution describes something other
/// than the documents in hand.
#[tokio::test]
async fn attributing_the_executors_answer_gives_the_same_documents_as_reading_directly() {
    let root = corpus();
    let fs = FilesystemSource::new(&root);
    let git = GitSource::open(&root);
    let front = FrontmatterSource::new(&root);
    let sources: Vec<&dyn DataSource> = vec![&fs, &git, &front];
    let federation = Federation::new(sources.clone(), &prover()).expect("declares");
    let scope = federation.scope("path:docs/guides/**").expect("scope");

    let executor = Executor::new(ExecutionPolicy::default());
    let execution_context = federation_support::read_context();
    let answer = executor
        .execute(&sources, &scope, &execution_context)
        .await
        .expect("the query runs");
    let attributed = federation.assemble_from(&answer).expect("attributes");
    let context = federation_support::read_context();
    let read_here = federation
        .assemble(&scope, &context)
        .await
        .expect("assembles");

    assert!(!attributed.is_empty());
    assert_eq!(attributed, read_here);
}

/// An answer that does not account for exactly this federation's sources is
/// refused. Attributing it would drop observations, or credit rows to a source
/// with no declared mapping — either way silently.
#[test]
fn an_answer_that_does_not_cover_this_federations_sources_is_refused() {
    let root = corpus();
    let fs = FilesystemSource::new(&root);
    let git = GitSource::open(&root);
    let front = FrontmatterSource::new(&root);
    let sources: Vec<&dyn DataSource> = vec![&fs, &git, &front];
    let federation = Federation::new(sources.clone(), &prover()).expect("declares");
    let scope = federation.scope("path:docs/guides/**").expect("scope");

    let mut executor = Executor::new(ExecutionPolicy::default());
    let mut answer =
        federation_support::execute_query(&mut executor, &sources, &scope).expect("the query runs");
    // The rows are all still there; only git's account of itself is gone.
    answer
        .reports
        .retain(|report| report.source != SourceId::new("git"));

    match federation.assemble_from(&answer) {
        Err(FederationError::SourceSetMismatch {
            federated,
            answered,
        }) => {
            assert_eq!(federated, ["filesystem", "frontmatter", "git"]);
            assert_eq!(answered, ["filesystem", "frontmatter"]);
        }
        other => panic!("expected a refusal naming both source sets, got {other:?}"),
    }
}

/// A source the answer reports as having refused cannot also have answered.
///
/// Its rows would land on `Document` fields whose provider the same answer
/// records as never having run — a value with a provenance that denies it.
#[test]
fn rows_from_a_source_the_answer_says_refused_are_refused_rather_than_attributed() {
    let root = corpus();
    let fs = FilesystemSource::new(&root);
    let git = GitSource::open(&root);
    let front = FrontmatterSource::new(&root);
    let sources: Vec<&dyn DataSource> = vec![&fs, &git, &front];
    let federation = Federation::new(sources.clone(), &prover()).expect("declares");
    let scope = federation.scope("path:docs/guides/**").expect("scope");

    let mut executor = Executor::new(ExecutionPolicy::default());
    let mut answer =
        federation_support::execute_query(&mut executor, &sources, &scope).expect("the query runs");
    // git's rows stay in the answer while its report becomes a refusal.
    for report in &mut answer.reports {
        if report.source == SourceId::new("git") {
            report.outcome = Outcome::Refused {
                reason: SourceError::Unavailable {
                    source_id: SourceId::new("git"),
                    reason: "not a git work tree".to_string(),
                },
            };
        }
    }

    match federation.assemble_from(&answer) {
        Err(FederationError::RefusedButAnswered {
            source_id,
            rows,
            reason,
        }) => {
            assert_eq!(source_id.as_str(), "git");
            assert!(rows > 0);
            assert!(reason.contains("not a git work tree"), "{reason}");
        }
        other => panic!("expected a refusal naming `git`, got {other:?}"),
    }
}

/// An answer whose rows carry no natural key is refused rather than attributed
/// to a document nobody can identify — the same refusal the direct read gives,
/// because both go through one keying step.
#[test]
fn a_row_with_no_natural_key_is_refused_however_the_answer_arrived() {
    let root = corpus();
    let fs = FilesystemSource::new(&root);
    let front = FrontmatterSource::new(&root);
    let sources: Vec<&dyn DataSource> = vec![&fs, &front];
    let federation = Federation::new(sources.clone(), &prover()).expect("declares");

    // A row carrying a size and no `path` at all: the natural key is absent,
    // not empty.
    let rows = vec![catdb_sdk::connectors::SourceRow {
        source: SourceId::new("filesystem"),
        entity: "doc.Document".to_string(),
        key: "docs/guides/task-lists.md".to_string(),
        value: serde_json::json!({ "size": 12 }),
    }];
    let answer = Federated {
        rows,
        reports: vec![
            SourceReport {
                source: SourceId::new("filesystem"),
                outcome: Outcome::Ran(Ran {
                    execution: Execution::Virtual,
                    taken_at_ms: 1,
                    scanned: 1,
                    returned: 1,
                    pushed: Vec::new(),
                    residual: Vec::new(),
                }),
            },
            SourceReport {
                source: SourceId::new("frontmatter"),
                outcome: Outcome::Ran(Ran {
                    execution: Execution::Virtual,
                    taken_at_ms: 1,
                    scanned: 0,
                    returned: 0,
                    pushed: Vec::new(),
                    residual: Vec::new(),
                }),
            },
        ],
    };

    match federation.assemble_from(&answer) {
        Err(FederationError::KeylessRow { source_id, key }) => {
            assert_eq!(source_id.as_str(), "filesystem");
            assert_eq!(key, "docs/guides/task-lists.md");
        }
        other => panic!("expected a keyless-row refusal, got {other:?}"),
    }
}
