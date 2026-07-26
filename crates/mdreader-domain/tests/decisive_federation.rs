//! **THE DECISIVE TEST** for P8. The acceptance criterion for the milestone,
//! and the claim that separates CatDB from a metadata catalogue:
//!
//! > ONE semantic `Document` query, answered from THREE genuinely different
//! > physical sources, with provenance naming WHICH SOURCE answered EACH FIELD
//! > — and every source file byte-identical afterwards.
//!
//! Every other test in this crate proves a component. This one proves the
//! *claim*. If it does not run, P8's thesis is unproven however many unit tests
//! are green.
//!
//! # The scenario, and what each step actually asserts
//!
//! 1. **Fingerprint every source file first.** Every file under the corpus
//!    root — not only the ones in scope — as path → (length, blake3, mtime).
//!    The mtime is in the fingerprint because a read that merely *touched* a
//!    file would leave the bytes identical, and touching a source is modifying
//!    it. `git status --porcelain` for the same subtree is captured alongside.
//! 2. **ONE query, executed through the real [`Executor`].** The three adapters
//!    are asserted to be genuinely different readers first (three different
//!    observed-attribute sets, three different physical mechanisms), the scope
//!    is validated once by [`Federation::scope`], and that single [`EntityQuery`]
//!    goes to `Executor::execute` with a per-source [`ExecutionPolicy`].
//! 3. **Complete and non-empty, asserted FIRST**, so nothing below can pass
//!    vacuously: all three sources ran, the answer is [`Completeness::Complete`],
//!    and the number of assembled documents equals the number of matching files
//!    the test counted itself in step 1.
//! 4. **Per-field attribution BY NAME.** The precedence *declaration* is
//!    asserted first — the whole order, provider by provider, with every field
//!    required to carry a rationale — and then the assembled document's fields
//!    are asserted against the sources that declaration predicts:
//!    `path`/`size`/`modifiedAt` ← filesystem, `title`/`body` ← frontmatter,
//!    `author` ← git. Every field's record is asserted to carry *every*
//!    declared provider in declared order, and the losers are asserted present
//!    with their verdicts: frontmatter is on the `author` record having
//!    observed nothing, git is on the `modifiedAt` record having observed a
//!    different value, and all three sources are on the `path` record having
//!    observed the same key.
//! 5. **Values checked against the physical sources, independently.** The
//!    mtime comes from `std::fs::metadata`, the size from the same stat *and*
//!    from the byte count, the author from a real `git log -1 --format=%an`,
//!    the title and body from this test's own parse of the file's bytes. None
//!    of it is taken from anything the federation produced — a test that checks
//!    the federation against itself proves nothing.
//! 6. **A field no source can supply is ABSENT**, with every declared provider
//!    on the record saying why, and the empty file's `size` of `0` next to it
//!    to show that a real zero and an absent value are different things.
//! 7. **The reported execution mode per source**, asserted as the typed
//!    [`Execution`] value (two sources virtual, one materialized), plus the
//!    pushdown line: `scanned` equals the number of files that physically exist
//!    under the pushed prefix (counted by this test), which is far fewer than
//!    the corpus holds — so the prefix really pruned — and `scanned > returned`,
//!    because the residual half rejected a file the pushed half admitted.
//! 8. **Re-fingerprint.** Byte-identical, mtimes unmoved, none added, none
//!    removed, `git status` unchanged.
//!
//! # Why the query is scoped to a subtree
//!
//! `fixtures/corpus/frontmatter/malformed-frontmatter.md` is invalid YAML *on
//! purpose*, and the frontmatter adapter fails the read rather than skipping
//! it. A source's filters are applied to rows it has already parsed, so any
//! scope whose **pushed prefix** does not exclude that file physically reads it
//! and the whole query fails — correctly. The decisive query therefore runs
//! over `edge-cases/`, which is the subtree that exercises every step: three
//! sources over the same eight files, a document whose YAML title differs from
//! its first H1, a document with no title at all, and a file the residual half
//! rejects (`notes.markdown`, which the `**.md` half of the pattern excludes
//! while the pushed `edge-cases/` prefix admits it).
//!
//! # What this scope cannot show, and where it is shown instead
//!
//! Every markdown file in the committed corpus is tracked and committed, so
//! there is no corpus document for which *both* `author` providers are silent.
//! That two-provider absence — the case where collapsing verdicts into an
//! `Option` would make "git has no commit for this" look like "nobody wrote
//! it" — is asserted in [`the_scenarios_own_guards_are_not_vacuous`] against a
//! real temporary repository with a staged-but-never-committed file. Likewise
//! `edge-cases/` holds no non-markdown file, so the
//! [`ProviderVerdict::DidNotSeeDocument`] verdict is exercised by
//! `federated_assembly.rs` over `assets/logo.png` rather than here.
//!
//! # The guard on the guard
//!
//! [`the_scenarios_own_guards_are_not_vacuous`] proves the load-bearing
//! equalities above can actually fail: `Complete` becomes `Partial` when a
//! source is genuinely missing, the same by-name attribution assertion yields a
//! *different* source on different data, the fingerprint moves for an appended
//! byte, for a file added, for a file removed and for a file merely touched,
//! and `git status --porcelain` moves when the index does. Without it,
//! "unchanged" and "answered by git" could both be true because nothing else
//! was ever possible.

mod federation_support;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use catdb_connectors::{
    DataSource, FilesystemSource, FrontmatterSource, GitSource, SourceId, SourceRow,
};
use catdb_core::AttrName;
use catdb_federation::{
    Completeness, Execution, ExecutionPolicy, Executor, Federated, Freshness, Outcome, Ran,
};
use catdb_validation::EggProver;
use federation_support::{corpus, fs_modified_ms, git_author, git_commit_seconds, git_status};
use mdreader_domain::federated::{Document, Federation, ProviderVerdict};

// ---------------------------------------------------------------------------
// The scenario's fixed points
// ---------------------------------------------------------------------------

/// The ONE scope the decisive query runs over. See the module docs for why it
/// is a subtree and not the whole corpus.
///
/// The `edge-cases/` prefix is pushed to every source (all three declare a
/// `Prefix` pushdown on the natural key); the full pattern stays residual and
/// rejects `edge-cases/notes.markdown`, which is what makes the two halves of
/// the plan separately observable in step 7.
const SCOPE: &str = "path:edge-cases/**.md";

/// The document every by-name attribution assertion in step 4 is about.
///
/// Chosen because its YAML `title:` and its first H1 are genuinely different
/// strings, so "the title came from the frontmatter source" is a claim with
/// content rather than a coincidence of two readers agreeing.
const TARGET: &str = "edge-cases/unicode-emoji-cjk.md";

/// The document whose `title` no source can supply — an empty file. Step 6.
const TITLELESS: &str = "edge-cases/empty.md";

/// The `Document` fields, in the order step 4 asserts them.
const FIELDS: &[&str] = &["path", "title", "author", "modifiedAt", "size", "body"];

fn prover() -> EggProver {
    EggProver::default()
}

// ---------------------------------------------------------------------------
// Step 1's instrument: the fingerprint
// ---------------------------------------------------------------------------

/// One source file, as step 1 and step 8 see it.
///
/// The modification time is part of it deliberately: a read that opened a file
/// for writing and wrote back the same bytes leaves `len` and `blake3`
/// identical, and is still a modification of a source this project promises
/// never to modify.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileFingerprint {
    len: u64,
    blake3: String,
    modified_ns: u128,
}

/// Every file under `root`, keyed by its root-relative path.
///
/// Nothing is skipped — dotfiles included — so a file appearing or vanishing
/// anywhere under the root is a key-set difference, not a silent pass.
fn fingerprint(root: &Path) -> BTreeMap<String, FileFingerprint> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, FileFingerprint>) {
        let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .expect("readable directory")
            .map(|entry| entry.expect("dir entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(root, &path, out);
                continue;
            }
            let bytes = std::fs::read(&path).expect("readable file");
            let meta = std::fs::metadata(&path).expect("stat");
            out.insert(
                path.strip_prefix(root)
                    .expect("under root")
                    .to_string_lossy()
                    .replace('\\', "/"),
                FileFingerprint {
                    len: bytes.len() as u64,
                    blake3: format!("blake3:{}", blake3::hash(&bytes).to_hex()),
                    modified_ns: meta
                        .modified()
                        .expect("mtime")
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("after epoch")
                        .as_nanos(),
                },
            );
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// The files this fingerprint holds under `prefix`, in path order.
fn under<'a>(files: &'a BTreeMap<String, FileFingerprint>, prefix: &str) -> Vec<&'a str> {
    files
        .keys()
        .filter(|rel| rel.starts_with(prefix))
        .map(String::as_str)
        .collect()
}

// ---------------------------------------------------------------------------
// Independent oracles — none of these consults the federation
// ---------------------------------------------------------------------------

/// The paths git itself tracks under a prefix, straight from `git ls-files`.
fn git_tracked(root: &Path, prefix: &str) -> Vec<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "--", prefix])
        .output()
        .expect("git ls-files runs");
    assert!(output.status.success(), "git ls-files failed");
    String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|rel| !rel.is_empty())
        .map(str::to_string)
        .collect()
}

/// The value of a top-level `key:` line in the document's leading `---` block,
/// parsed here rather than taken from the adapter — this is the oracle the
/// federated `title` is checked against.
fn yaml_field(text: &str, key: &str) -> Option<String> {
    let block = text
        .strip_prefix("---\n")?
        .split("\n---\n")
        .next()?
        .to_string();
    block
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}: ")))
        .map(str::to_string)
}

/// The document's first `# ` heading — the *other* candidate for a title, kept
/// so step 5 can assert the federated title is not it.
fn first_h1(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("# "))
        .map(|heading| heading.trim().to_string())
}

/// Everything after the closing `---` of the leading block.
fn after_frontmatter(text: &str) -> String {
    let rest = text
        .strip_prefix("---\n")
        .expect("the fixture opens with a frontmatter block");
    let (_, body) = rest
        .split_once("\n---\n")
        .expect("the fixture's frontmatter block is closed");
    body.to_string()
}

// ---------------------------------------------------------------------------
// Small readers over the answer
// ---------------------------------------------------------------------------

fn document<'a>(documents: &'a [Document], path: &str) -> &'a Document {
    documents
        .iter()
        .find(|document| document.path() == path)
        .unwrap_or_else(|| {
            panic!(
                "no document for `{path}`; the answer holds {:?}",
                documents.iter().map(Document::path).collect::<Vec<_>>()
            )
        })
}

/// The source that answered a field, by name — panicking with the whole
/// explanation rather than returning `None`, so an unanswered field can never
/// read as a passing comparison.
fn answered_by(document: &Document, field: &str) -> String {
    document
        .source_of(field)
        .unwrap_or_else(|| {
            panic!(
                "`{field}` was not answered at all:\n{}",
                document.explain().join("\n")
            )
        })
        .to_string()
}

/// One source's execution details, insisting it ran.
fn ran<'a>(answer: &'a Federated, source: &str) -> &'a Ran {
    let report = answer
        .report(&SourceId::new(source))
        .unwrap_or_else(|| panic!("no report for `{source}`:\n{answer}"));
    report
        .ran()
        .unwrap_or_else(|| panic!("`{source}` did not run:\n{answer}"))
}

/// Every declared provider on a field's record, with the verdict it gave.
fn record(document: &Document, field: &str) -> Vec<(String, String)> {
    document
        .field(field)
        .unwrap_or_else(|| panic!("`{field}` is not a declared field"))
        .verdicts
        .iter()
        .map(|(provider, verdict)| (provider.to_string(), verdict.as_str().to_string()))
        .collect()
}

fn as_i64(document: &Document, field: &str) -> Option<i64> {
    document.value(field).and_then(serde_json::Value::as_i64)
}

// ===========================================================================
// THE DECISIVE SCENARIO
// ===========================================================================

/// One `Document` query, three physically different sources, per-field
/// provenance naming which source answered what — over a corpus that is
/// byte-identical afterwards.
#[test]
fn one_query_answered_by_three_physical_sources_names_the_source_of_every_field_and_touches_nothing(
) {
    // -- 1. Fingerprint every source file, before anything reads them --------
    let root = corpus();
    let before = fingerprint(&root);
    let status_before = git_status(&root);
    assert!(
        before.len() > 40,
        "the corpus fingerprint covers only {} files — the scenario would be asserting about \
         almost nothing",
        before.len()
    );
    for rel in [TARGET, TITLELESS] {
        assert!(
            before.contains_key(rel),
            "`{rel}` is not in the corpus, so the assertions about it cannot mean anything"
        );
    }
    // The two counts the pushdown line is asserted against in step 7, taken
    // from the filesystem by this test rather than from any source's report.
    let under_prefix = under(&before, "edge-cases/");
    let in_scope: Vec<&str> = under_prefix
        .iter()
        .copied()
        .filter(|rel| rel.ends_with(".md"))
        .collect();
    assert!(
        in_scope.len() < under_prefix.len(),
        "the scope's residual half must reject at least one file the pushed half admits, or \
         step 7 proves nothing about the split"
    );

    // -- 2. ONE query, through the real Executor -----------------------------
    let fs = FilesystemSource::new(&root);
    let git = GitSource::open(&root);
    let front = FrontmatterSource::new(&root);

    // Three genuinely different readers: a stat walk, `git` plumbing, and a
    // YAML/markdown parse — and they observe three different sets of things.
    assert_eq!(
        fs.capabilities().observed(),
        ["modifiedAt", "path", "size"],
        "the filesystem source is not observing what this scenario assumes"
    );
    assert_eq!(
        git.capabilities().observed(),
        ["author", "modifiedAt", "path"],
        "the git source is not observing what this scenario assumes"
    );
    assert_eq!(
        front.capabilities().observed(),
        ["author", "body", "path", "title"],
        "the frontmatter source is not observing what this scenario assumes"
    );

    let sources: Vec<&dyn DataSource> = vec![&fs, &git, &front];
    let federation = Federation::new(sources.clone(), &prover()).expect("the federation declares");
    let query = federation.scope(SCOPE).expect("the scope validates");
    assert_eq!(query.target.as_str(), "doc.Document");
    assert_eq!(
        query.filters.len(),
        1,
        "the decisive scenario is ONE query with one filter over the natural key"
    );
    assert_eq!(query.filters[0].attr, "path");

    // The mode is a caller declaration, never an inference: git spawns a
    // process per path, so its reads are declared reusable and the other two
    // are not. Step 7 asserts the executor reported exactly that.
    let policy = ExecutionPolicy::default()
        .materialize(SourceId::new(GitSource::ID), Duration::from_secs(300));
    let mut executor = Executor::new(policy);
    let answer = executor
        .execute(&sources, &query)
        .expect("the federated read runs");

    // -- 3. Complete and non-empty, before anything else ---------------------
    assert_eq!(
        answer.completeness(),
        Completeness::Complete,
        "a source was left out, so nothing below is about three sources:\n{answer}"
    );
    assert_eq!(answer.reports.len(), 3, "one report per source:\n{answer}");
    assert!(
        !answer.rows.is_empty(),
        "the query returned no rows at all:\n{answer}"
    );

    // The attribution comes from THIS answer — the same execution the reports
    // in step 7 describe, not a second read that happens to agree.
    let documents = federation
        .assemble_from(&answer)
        .expect("the answer attributes into documents");
    assert_eq!(
        documents.iter().map(Document::path).collect::<Vec<_>>(),
        in_scope,
        "the assembled documents are not the files that match the scope"
    );
    assert_eq!(
        answer.rows.len(),
        3 * in_scope.len(),
        "every document should carry one row from each of the three sources:\n{answer}"
    );
    for document in &documents {
        let mut observers: Vec<&str> = document
            .observations()
            .iter()
            .map(|row: &SourceRow| row.source.as_str())
            .collect();
        observers.sort_unstable();
        assert_eq!(
            observers,
            ["filesystem", "frontmatter", "git"],
            "`{}` was not seen by all three sources",
            document.path()
        );
    }

    // -- 4. Per-field attribution, by name — and the declaration itself ------
    let declared: Vec<(String, Vec<String>)> = federation
        .precedence()
        .fields()
        .iter()
        .map(|entry| {
            (
                entry.field.to_string(),
                entry.order.iter().map(ToString::to_string).collect(),
            )
        })
        .collect();
    assert_eq!(
        declared,
        vec![
            (
                "author".to_string(),
                vec!["frontmatter.author".to_string(), "git.author".to_string()]
            ),
            ("body".to_string(), vec!["frontmatter.body".to_string()]),
            (
                "modifiedAt".to_string(),
                vec![
                    "filesystem.modifiedAt".to_string(),
                    "git.modifiedAt".to_string()
                ]
            ),
            (
                "path".to_string(),
                vec![
                    "filesystem.path".to_string(),
                    "git.path".to_string(),
                    "frontmatter.path".to_string()
                ]
            ),
            ("size".to_string(), vec!["filesystem.size".to_string()]),
            ("title".to_string(), vec!["frontmatter.title".to_string()]),
        ],
        "the precedence DECLARATION has changed; the by-name assertions below are about it"
    );
    for entry in federation.precedence().fields() {
        assert!(
            entry.rationale.len() > 40,
            "`{}` orders its providers without saying why",
            entry.field
        );
    }

    let target = document(&documents, TARGET);
    let answered: Vec<(&str, String)> = FIELDS
        .iter()
        .map(|field| (*field, answered_by(target, field)))
        .collect();
    assert_eq!(
        answered,
        [
            // The natural key, from the source that is the authority on what is
            // on disk.
            ("path", "filesystem".to_string()),
            // The YAML block — a parse of the file's text.
            ("title", "frontmatter".to_string()),
            // No YAML `author:` here, so the COMMIT author answers: a genuinely
            // different physical source from the one directly above.
            ("author", "git".to_string()),
            // The stat, not the commit timestamp.
            ("modifiedAt", "filesystem".to_string()),
            ("size", "filesystem".to_string()),
            ("body", "frontmatter".to_string()),
        ],
        "{}",
        target.explain().join("\n")
    );

    // Every declared provider is on every field's record, in declared order —
    // the losers are kept, not dropped once a winner is found.
    for entry in federation.precedence().fields() {
        let field = entry.field.to_string();
        let recorded: Vec<String> = record(target, &field)
            .into_iter()
            .map(|(provider, _)| provider)
            .collect();
        assert_eq!(
            recorded,
            entry
                .order
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            "`{field}`'s record does not carry every declared provider in declared order"
        );
    }

    // The losers, with their verdicts. `author`: the higher-precedence source
    // read the document and had nothing.
    assert_eq!(
        record(target, "author"),
        [
            (
                "frontmatter.author".to_string(),
                "observed-nothing".to_string()
            ),
            ("git.author".to_string(), "observed".to_string()),
        ]
    );
    let author = target.field("author").expect("`author` is declared");
    assert_eq!(
        author
            .silent_above()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["frontmatter.author"]
    );

    // `modifiedAt`: the lower-precedence source DID observe a value, and it is
    // kept on the record rather than forgotten.
    let modified = target
        .field("modifiedAt")
        .expect("`modifiedAt` is declared");
    assert_eq!(
        record(target, "modifiedAt"),
        [
            ("filesystem.modifiedAt".to_string(), "observed".to_string()),
            ("git.modifiedAt".to_string(), "observed".to_string()),
        ]
    );
    assert_eq!(
        modified
            .outranked()
            .iter()
            .map(|provider| provider.source.to_string())
            .collect::<Vec<_>>(),
        ["git"],
        "git was outranked, not ignored"
    );

    // `path`: all three sources observed it, and they agree — which is what the
    // identity rule rests on.
    assert_eq!(
        record(target, "path"),
        [
            ("filesystem.path".to_string(), "observed".to_string()),
            ("git.path".to_string(), "observed".to_string()),
            ("frontmatter.path".to_string(), "observed".to_string()),
        ]
    );
    let path_field = target.field("path").expect("`path` is declared");
    for (provider, verdict) in &path_field.verdicts {
        assert_eq!(
            verdict.observed().and_then(serde_json::Value::as_str),
            Some(TARGET),
            "`{provider}` disagrees about the natural key"
        );
    }
    assert_eq!(
        target.claims().len(),
        3,
        "one identity claim per observation"
    );

    // -- 5. The VALUES, against the physical sources, independently ----------
    let bytes = std::fs::read(root.join(TARGET)).expect("the target file is readable");
    let text = String::from_utf8(bytes.clone()).expect("the target file is UTF-8");
    let meta = std::fs::metadata(root.join(TARGET)).expect("stat");

    // `title` — parsed out of the bytes here, and asserted NOT to be the first
    // H1, so "the frontmatter source answered it" has content.
    let yaml_title = yaml_field(&text, "title").expect("the target declares a YAML title");
    let h1 = first_h1(&text).expect("the target also has a first H1");
    assert_ne!(
        yaml_title, h1,
        "the target's YAML title and first H1 must differ, or `title`'s provenance is untestable"
    );
    assert_eq!(target.text("title"), Some(yaml_title.as_str()));
    assert_ne!(target.text("title"), Some(h1.as_str()));

    // `body` — the bytes after the closing delimiter, split here.
    assert_eq!(target.text("body"), Some(after_frontmatter(&text).as_str()));

    // `size` — the stat, cross-checked against the byte count.
    assert_eq!(
        meta.len(),
        bytes.len() as u64,
        "the stat disagrees with the bytes"
    );
    assert_eq!(as_i64(target, "size"), Some(bytes.len() as i64));

    // `modifiedAt` — the mtime from `fs::metadata`, and git's own answer from a
    // real `git log`, which is a genuinely different number.
    assert_eq!(
        as_i64(target, "modifiedAt"),
        Some(fs_modified_ms(&root, TARGET))
    );
    let commit_ms = git_commit_seconds(&root, TARGET) * 1000;
    assert_eq!(
        modified.verdicts[1]
            .1
            .observed()
            .and_then(serde_json::Value::as_i64),
        Some(commit_ms),
        "git's outranked answer must be git's actual commit timestamp"
    );
    assert_ne!(
        as_i64(target, "modifiedAt"),
        Some(commit_ms),
        "the two sources must genuinely disagree, or the declared order decided nothing"
    );

    // `author` — from `git log -1 --format=%an`, not from anything above.
    let commit_author = git_author(&root, TARGET);
    assert!(!commit_author.is_empty(), "the corpus has a commit author");
    assert_eq!(target.text("author"), Some(commit_author.as_str()));
    assert!(
        !text.contains("\nauthor:"),
        "the target must NOT declare a YAML author, or `author` would not be git's to answer"
    );

    // -- 6. A field no source can supply is ABSENT ---------------------------
    let titleless = document(&documents, TITLELESS);
    let title = titleless.field("title").expect("`title` is declared");
    assert!(title.is_absent(), "{}", title.explain());
    assert_eq!(titleless.value("title"), None);
    assert_eq!(titleless.text("title"), None);
    assert_eq!(titleless.source_of("title"), None);
    // Every declared provider is on the record, saying why. The declaration
    // says there is exactly one, and that is asserted rather than assumed.
    assert_eq!(
        federation
            .precedence()
            .of(&AttrName::from("title"))
            .expect("`title` is declared")
            .order
            .len(),
        1
    );
    assert_eq!(
        record(titleless, "title"),
        [(
            "frontmatter.title".to_string(),
            "observed-nothing".to_string()
        )]
    );
    assert_ne!(
        title.verdicts[0].1,
        ProviderVerdict::DidNotSeeDocument,
        "the source READ this document and found no title; that is a different fact from never \
         having seen it"
    );
    assert!(title.explain().starts_with("`title` is absent"));
    // And absence is per field: the same document's `size` is a real zero, from
    // a source that did answer.
    assert_eq!(
        std::fs::read(root.join(TITLELESS)).expect("readable").len(),
        0
    );
    assert_eq!(answered_by(titleless, "size"), "filesystem");
    assert_eq!(
        as_i64(titleless, "size"),
        Some(0),
        "an empty file's size is 0, which is a value — not the absence above"
    );

    // -- 7. How each source ran, and where the pushdown line fell ------------
    assert_eq!(
        ran(&answer, "filesystem").execution,
        Execution::Virtual,
        "the filesystem source was declared virtual"
    );
    assert_eq!(
        ran(&answer, "frontmatter").execution,
        Execution::Virtual,
        "the frontmatter source was declared virtual"
    );
    assert_eq!(
        ran(&answer, "git").execution,
        Execution::Materialized(Freshness::Fresh),
        "git was declared materializable, and this is its first read"
    );

    // Each source's own physical population under the pushed prefix, counted
    // here: every file for the walk, every markdown file for the parser, every
    // tracked path for git.
    let markdown = under_prefix
        .iter()
        .filter(|rel| {
            FrontmatterSource::EXTENSIONS
                .iter()
                .any(|ext| rel.ends_with(&format!(".{ext}")))
        })
        .count();
    let tracked = git_tracked(&root, "edge-cases/");
    for (source, scanned) in [
        ("filesystem", under_prefix.len()),
        ("frontmatter", markdown),
        ("git", tracked.len()),
    ] {
        let report = ran(&answer, source);
        assert_eq!(
            report.pushed,
            ["path"],
            "`{source}` did not push the natural key down"
        );
        assert_eq!(
            report.residual,
            ["path"],
            "`{source}` did not leave the full glob to the executor"
        );
        assert_eq!(
            report.scanned, scanned,
            "`{source}` examined {} candidates; the tree physically holds {scanned} under the \
             pushed prefix",
            report.scanned
        );
        assert!(
            report.scanned < before.len(),
            "`{source}` examined {} of {} files: the pushed prefix did not prune the read",
            report.scanned,
            before.len()
        );
        assert_eq!(report.returned, in_scope.len());
        assert!(
            report.scanned > report.returned,
            "`{source}` returned everything it scanned, so the residual half decided nothing"
        );
        assert!(report.taken_at_ms > 0, "`{source}` reported no read time");
    }

    // -- 8. Re-fingerprint: nothing was written, touched, added or removed ---
    let after = fingerprint(&root);
    let added: Vec<&String> = after
        .keys()
        .filter(|rel| !before.contains_key(*rel))
        .collect();
    let removed: Vec<&String> = before
        .keys()
        .filter(|rel| !after.contains_key(*rel))
        .collect();
    assert!(
        added.is_empty(),
        "files appeared under the corpus root: {added:?}"
    );
    assert!(
        removed.is_empty(),
        "files vanished from the corpus root: {removed:?}"
    );
    let changed: Vec<&String> = before
        .iter()
        .filter(|(rel, fingerprint)| after.get(*rel) != Some(fingerprint))
        .map(|(rel, _)| rel)
        .collect();
    assert!(
        changed.is_empty(),
        "these source files were modified or touched by the query: {changed:?}"
    );
    assert_eq!(before, after);
    assert_eq!(
        status_before,
        git_status(&root),
        "reading changed what git tracks"
    );
}

// ===========================================================================
// The guard on the guard
// ===========================================================================

/// The decisive scenario's own equality guards can fail.
///
/// Four of its assertions would be worthless if the value they compare could
/// never be anything else, so each is shown moving:
///
/// 1. `Complete` becomes `Partial`, naming the excluded source, when a source
///    genuinely is not there — and that source's providers are recorded as
///    never asked, with the executor's own reason.
/// 2. The very same by-name attribution assertion returns `frontmatter` for one
///    document and `git` for another, over the same code path — so
///    "answered by git" is read from the data, not printed by construction.
///    The same fixture holds the two-provider absence the committed corpus
///    cannot show: a staged-but-uncommitted file whose `author` neither
///    provider can supply, with both on the record saying so.
/// 3. The fingerprint moves for an appended byte, for a file added, for a file
///    removed, and for a file rewritten with identical bytes — the last of
///    which is why the modification time is in it at all.
/// 4. `git status --porcelain` moves when the index or the work tree does.
#[test]
fn the_scenarios_own_guards_are_not_vacuous() {
    // (1) `Complete` is not a constant.
    let outside = federation_support::tree(&[(
        "note.md",
        "---\ntitle: Outside Any Repository\nauthor: A. Writer\n---\n\n# Note\n\nbody\n",
    )]);
    let root = outside.path();
    let fs = FilesystemSource::new(root);
    let git = GitSource::open(root);
    let front = FrontmatterSource::new(root);
    let sources: Vec<&dyn DataSource> = vec![&fs, &git, &front];
    let federation = Federation::new(sources.clone(), &prover()).expect("declares");
    let query = federation.scope("").expect("scope validates");
    let mut executor = Executor::new(ExecutionPolicy::default());
    let answer = executor
        .execute(&sources, &query)
        .expect("two sources can still answer");
    match answer.completeness() {
        Completeness::Complete => {
            panic!("a federation missing a whole source reported itself complete:\n{answer}")
        }
        Completeness::Partial { excluded } => {
            assert_eq!(
                excluded
                    .iter()
                    .map(|e| e.source.to_string())
                    .collect::<Vec<_>>(),
                ["git"]
            );
            assert!(
                excluded[0].reason.contains("git work tree"),
                "the exclusion must carry the source's own reason: {}",
                excluded[0].reason
            );
        }
    }
    assert!(matches!(
        answer
            .report(&SourceId::new("git"))
            .expect("git has a report")
            .outcome,
        Outcome::Refused { .. }
    ));
    let documents = federation.assemble_from(&answer).expect("attributes");
    let note = document(&documents, "note.md");
    let author = note.field("author").expect("declared");
    assert_eq!(
        record(note, "author"),
        [
            ("frontmatter.author".to_string(), "observed".to_string()),
            ("git.author".to_string(), "not-asked".to_string()),
        ],
        "a source the executor never ran must be on the record as never asked"
    );
    match author.verdicts.get(1).map(|(_, verdict)| verdict) {
        Some(ProviderVerdict::NotAsked { reason }) => assert!(
            reason.contains("git work tree"),
            "the verdict must carry the executor's own reason: {reason}"
        ),
        other => panic!("git must be recorded as never asked, got {other:?}"),
    }
    assert_eq!(answered_by(note, "author"), "frontmatter");

    // (2) The same assertion, different answer — and a two-provider absence.
    let repo = federation_support::git_fixture();
    let root = repo.path();
    let fs = FilesystemSource::new(root);
    let git = GitSource::open(root);
    let front = FrontmatterSource::new(root);
    let sources: Vec<&dyn DataSource> = vec![&fs, &git, &front];
    let federation = Federation::new(sources.clone(), &prover()).expect("declares");
    let query = federation.scope("").expect("scope validates");
    let mut executor = Executor::new(ExecutionPolicy::default());
    let answer = executor
        .execute(&sources, &query)
        .expect("all three answer");
    assert_eq!(answer.completeness(), Completeness::Complete);
    let documents = federation.assemble_from(&answer).expect("attributes");

    assert_eq!(
        answered_by(document(&documents, "docs/declared.md"), "author"),
        "frontmatter",
        "a document that declares its own author is answered by the frontmatter source"
    );
    assert_eq!(
        answered_by(document(&documents, "docs/committed.md"), "author"),
        "git",
        "…and one that does not is answered by git — same assertion, different source"
    );
    let staged = document(&documents, "docs/staged.md");
    assert!(staged.field("author").expect("declared").is_absent());
    assert_eq!(
        record(staged, "author"),
        [
            (
                "frontmatter.author".to_string(),
                "observed-nothing".to_string()
            ),
            ("git.author".to_string(), "observed-nothing".to_string()),
        ],
        "a file git has never committed must leave `author` absent with BOTH providers on the \
         record — not silently answered, and not collapsed into one `None`"
    );
    // The same document's `modifiedAt` still resolves, from the source that did
    // observe it: absence is per field, not per document.
    assert_eq!(answered_by(staged, "modifiedAt"), "filesystem");

    // (4) `git status --porcelain` moves when the work tree does. Asserted here
    // because this fixture is a real repository.
    let status_before = git_status(root);
    std::fs::write(root.join("docs/untracked.md"), "# New\n").expect("write");
    assert_ne!(
        status_before,
        git_status(root),
        "step 8's git-status comparison cannot notice a new file"
    );

    // (3) The fingerprint moves — for a byte, a file, a deletion, and a touch.
    let tree = federation_support::tree(&[("a.md", "# A\n"), ("nested/b.md", "# B\n")]);
    let root = tree.path();
    let before = fingerprint(root);
    assert_eq!(before.len(), 2);

    std::fs::write(root.join("a.md"), "# A\n!").expect("write");
    let appended = fingerprint(root);
    assert_ne!(
        before, appended,
        "an appended byte must move the fingerprint"
    );
    assert_ne!(before["a.md"].blake3, appended["a.md"].blake3);

    std::fs::write(root.join("c.md"), "# C\n").expect("write");
    assert!(
        fingerprint(root).len() > appended.len(),
        "a file appearing must move the fingerprint's key set"
    );

    std::fs::remove_file(root.join("nested/b.md")).expect("remove");
    let removed = fingerprint(root);
    assert!(
        !removed.contains_key("nested/b.md"),
        "a file vanishing must move the fingerprint's key set"
    );

    // A rewrite with IDENTICAL bytes: length and hash are unchanged, and the
    // fingerprint must still notice. This is the whole reason the modification
    // time is in it. The pause is for filesystems whose timestamps are coarser
    // than one write.
    let touched_before = fingerprint(root);
    std::thread::sleep(Duration::from_millis(50));
    let bytes = std::fs::read(root.join("a.md")).expect("read");
    std::fs::write(root.join("a.md"), &bytes).expect("rewrite the same bytes");
    let touched_after = fingerprint(root);
    assert_eq!(
        touched_before["a.md"].blake3, touched_after["a.md"].blake3,
        "the rewrite really did leave the bytes identical"
    );
    assert_eq!(touched_before["a.md"].len, touched_after["a.md"].len);
    assert_ne!(
        touched_before, touched_after,
        "a source file that was TOUCHED but not changed must still fail step 8"
    );
}
