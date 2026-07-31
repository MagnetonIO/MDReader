//! A failure never degrades into a smaller answer.

mod exec_support;

use catdb_sdk::connectors::{DataSource, SourceError};
use mdreader_connector_local::{FilesystemSource, FrontmatterSource, GitSource};
use catdb_sdk::federation::{Completeness, ExecutionPolicy, Executor, FederationError};
use exec_support::{corpus, execute_query, federated_query, read_source, Flaky};

/// A source whose read fails fails the **query**. Its rows are not quietly
/// omitted, and the other source's rows are not handed back as if they were the
/// whole answer.
///
/// The failure is a real one: `fixtures/corpus/frontmatter/malformed-frontmatter.md`
/// has a frontmatter block that is not valid YAML.
#[test]
fn a_failing_source_fails_the_query_rather_than_shrinking_the_result() {
    let root = corpus();
    let fs = FilesystemSource::new(&root);
    let front = FrontmatterSource::new(&root);
    let sources: Vec<&dyn DataSource> = vec![&fs, &front];
    let query = federated_query(&sources, "path:frontmatter/**");

    // The healthy source has plenty to say — which is exactly what must not be
    // returned as a complete answer.
    let healthy = read_source(&fs, &query).expect("reads").rows.len();
    assert!(healthy > 0, "the fixture must give the filesystem rows");

    let executor = Executor::new(ExecutionPolicy::default());
    match execute_query(&executor, &sources, &query) {
        Ok(answer) => panic!(
            "a failing source must fail the query, got {} row(s) from {:?}",
            answer.rows.len(),
            answer.completeness()
        ),
        Err(FederationError::SourceFailed { source_id, error }) => {
            assert_eq!(source_id.as_str(), FrontmatterSource::ID);
            assert!(
                matches!(error, SourceError::Malformed { .. }),
                "the failure must keep its own type: {error}"
            );
            assert!(
                format!("{}", FederationError::SourceFailed { source_id, error })
                    .contains(FrontmatterSource::ID),
                "the overall failure must name the source"
            );
        }
        Err(other) => panic!("expected a named source failure, got {other}"),
    }
}

/// The same, with the failure injected rather than fixture-borne: the point is
/// structural, not a property of one malformed file.
#[test]
fn an_unreachable_source_fails_the_query_it_was_part_of() {
    let root = corpus();
    let fs = FilesystemSource::new(&root);
    let front = Flaky::broken(FrontmatterSource::new(&root));
    let sources: Vec<&dyn DataSource> = vec![&fs, &front];
    let query = federated_query(&sources, "path:notes/**");

    let executor = Executor::new(ExecutionPolicy::default());
    let error =
        execute_query(&executor, &sources, &query).expect_err("a broken source fails the query");
    assert!(matches!(
        error,
        FederationError::SourceFailed { ref source_id, .. }
            if source_id.as_str() == FrontmatterSource::ID
    ));

    // Repaired, the very same query succeeds — so the failure was the failure,
    // not the query.
    front.repair();
    let answer = execute_query(&executor, &sources, &query).expect("executes");
    assert_eq!(answer.completeness(), Completeness::Complete);
    assert!(!answer.rows.is_empty());
}

/// If *no* source can answer, the query fails. An empty answer that every
/// source refused is the exact shape of a real failure rendered as a benign
/// empty state.
#[test]
fn a_federation_where_no_source_can_answer_is_refused_rather_than_empty() {
    let root = corpus();
    let repo = corpus(); // any directory; the git source is only here for its vocabulary
    let fs = FilesystemSource::new(&root);
    let front = FrontmatterSource::new(&root);
    let git = GitSource::open(&repo);
    // `size` is vocabulary because the filesystem declares it...
    let all: Vec<&dyn DataSource> = vec![&fs, &front, &git];
    let query = federated_query(&all, "size:>1");
    // ...but the filesystem is not among the sources actually executed.
    let asked: Vec<&dyn DataSource> = vec![&front, &git];

    let executor = Executor::new(ExecutionPolicy::default());
    match execute_query(&executor, &asked, &query) {
        Ok(answer) => panic!(
            "no source could answer, so this must not be an empty success: {:?}",
            answer.completeness()
        ),
        Err(FederationError::NoSourceCouldAnswer { refusals }) => {
            assert_eq!(
                refusals.len(),
                2,
                "every refusal must be named: {refusals:?}"
            );
            for refusal in &refusals {
                assert!(matches!(
                    refusal.reason,
                    SourceError::UnobservableAttribute { .. }
                ));
            }
        }
        Err(other) => panic!("expected NoSourceCouldAnswer, got {other}"),
    }
}

/// Executing against no sources at all is a refusal, not an empty answer: there
/// was no question asked of anything.
#[test]
fn executing_against_no_sources_is_refused() {
    let root = corpus();
    let fs = FilesystemSource::new(&root);
    let query = federated_query(&[&fs], "path:notes/**");
    let executor = Executor::new(ExecutionPolicy::default());
    assert!(matches!(
        execute_query(&executor, &[], &query),
        Err(FederationError::NoSources)
    ));
}

/// A partial answer is marked partial and names what it left out — it is never
/// presented as complete.
#[test]
fn a_partial_answer_names_the_sources_it_left_out() {
    let root = corpus();
    let fs = FilesystemSource::new(&root);
    let front = FrontmatterSource::new(&root);
    let absent = FilesystemSource::new(root.join("definitely-not-here"));
    let sources: Vec<&dyn DataSource> = vec![&front, &absent];
    let query = federated_query(&[&fs, &front], "path:notes/**");

    let executor = Executor::new(ExecutionPolicy::default());
    let answer = execute_query(&executor, &sources, &query).expect("executes");
    assert!(!answer.rows.is_empty());
    match answer.completeness() {
        Completeness::Partial { excluded } => {
            assert_eq!(excluded.len(), 1);
            assert_eq!(excluded[0].source.as_str(), FilesystemSource::ID);
            assert!(
                excluded[0].reason.contains("not a directory"),
                "the exclusion must carry the reason: {}",
                excluded[0].reason
            );
        }
        Completeness::Complete => {
            panic!("an answer with a source that could not be read is not complete")
        }
    }
}

/// Repository absence is a runtime refusal, not corruption. A normal Markdown
/// folder still has truthful filesystem/frontmatter evidence, and the answer
/// must retain Git's terminal refusal instead of aborting before a caller can
/// render the source report.
#[test]
fn a_non_git_markdown_library_returns_a_partial_answer_with_all_source_reports() {
    let root = exec_support::tree(&[(
        "note.md",
        "---\ntitle: Plain folder\n---\n# Plain folder\n\nBody.\n",
    )]);
    let fs = FilesystemSource::new(root.path());
    let git = GitSource::open(root.path());
    let front = FrontmatterSource::new(root.path());
    let sources: Vec<&dyn DataSource> = vec![&fs, &git, &front];
    let query = federated_query(&sources, "path:note.md");
    let executor = Executor::new(ExecutionPolicy::default());

    let answer = execute_query(&executor, &sources, &query)
        .expect("repository absence is a reported partial answer");

    assert_eq!(answer.reports.len(), 3);
    assert!(matches!(
        answer.completeness(),
        Completeness::Partial { .. }
    ));
    assert!(answer.reports[0].ran().is_some(), "filesystem ran");
    assert!(matches!(
        answer.reports[1].outcome,
        catdb_sdk::federation::Outcome::Refused {
            reason: SourceError::Unavailable { .. }
        }
    ));
    assert!(answer.reports[2].ran().is_some(), "frontmatter ran");
}
