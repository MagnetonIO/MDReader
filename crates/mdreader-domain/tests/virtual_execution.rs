//! Virtual execution: push what each source declared, apply the rest here.

mod support;

use catdb_connectors::{DataSource, FilesystemSource, FrontmatterSource, SourceError, SourceId};
use catdb_federation::{Completeness, Execution, ExecutionPolicy, Executor, Outcome};
use support::{
    brute_force_paths, corpus, execute_query, execute_source, federated_query, keyed, read_source,
    Counting,
};

/// The pattern the agreement test uses.
///
/// Chosen so that **both halves of the plan do work**: `notes/` is a literal
/// prefix both sources declared they can push, and the full pattern rejects
/// most of what that prefix returns. A pattern the prefix alone answered would
/// make the agreement vacuous.
const PATTERN: &str = "notes/**/node-*.md";

/// Virtual execution over two sources agrees with a brute-force scan that
/// shares none of its machinery — no plan, no capability declaration, no
/// pushdown, no residual, just `read_dir` and the pattern.
#[test]
fn virtual_execution_over_two_sources_agrees_with_an_independent_brute_force_scan() {
    let root = corpus();
    let fs = FilesystemSource::new(&root);
    let front = FrontmatterSource::new(&root);
    let sources: Vec<&dyn DataSource> = vec![&fs, &front];
    let query = federated_query(&sources, &format!("path:{PATTERN}"));

    let executor = Executor::new(ExecutionPolicy::default());
    let answer = execute_query(&executor, &sources, &query).expect("executes");

    let mut expected: Vec<(String, String)> = Vec::new();
    for path in brute_force_paths(&root, PATTERN, None) {
        expected.push((FilesystemSource::ID.to_string(), path));
    }
    for path in brute_force_paths(&root, PATTERN, Some(FrontmatterSource::EXTENSIONS)) {
        expected.push((FrontmatterSource::ID.to_string(), path));
    }
    expected.sort();

    // The oracle has to be non-trivial in both directions, or agreement proves
    // nothing: some documents match, and most of the tree does not.
    assert!(!expected.is_empty(), "the oracle must match something");
    assert!(
        expected.len() < brute_force_paths(&root, "**", None).len(),
        "the oracle must reject something"
    );
    assert_eq!(keyed(&answer.rows), expected);
    assert_eq!(answer.completeness(), Completeness::Complete);
}

/// The residual half is applied **by the executor**, over rows the source
/// returned. Proved the only way it can be: the same source, executed with the
/// residual half stripped, returns strictly more rows than the answer carries.
///
/// Without this the agreement test above proves nothing — a pushdown that
/// happened to be exact would pass it either way.
#[test]
fn a_residual_is_applied_by_the_executor_over_more_rows_than_it_keeps() {
    let root = corpus();
    let fs = FilesystemSource::new(&root);
    let front = FrontmatterSource::new(&root);
    let sources: Vec<&dyn DataSource> = vec![&fs, &front];
    let query = federated_query(&sources, &format!("path:{PATTERN}"));

    let executor = Executor::new(ExecutionPolicy::default());
    let answer = execute_query(&executor, &sources, &query).expect("executes");

    for source in &sources {
        let plan = source
            .compile(&query, &source.capabilities().observed())
            .expect("compiles");
        assert!(
            !plan.residual.is_empty(),
            "`{}` must have a residual half for this test to mean anything",
            source.id()
        );
        // What the source hands back when only its declared pushdown is asked
        // for — i.e. what the executor has to filter.
        let mut pushed_only = plan.clone();
        pushed_only.residual.clear();
        let unfiltered = execute_source(*source, &pushed_only)
            .expect("executes")
            .rows
            .len();
        let kept = answer
            .rows
            .iter()
            .filter(|row| &row.source == source.id())
            .count();
        assert!(
            unfiltered > kept,
            "`{}` returned {unfiltered} row(s) and the answer kept {kept}: the residual \
             did no work, so this test cannot tell whether it was applied",
            source.id()
        );
    }
}

/// A source that cannot **observe** an attribute the query filters on is
/// refused and named — never asked for its rows and never allowed to contribute
/// them unfiltered, which would over-return and look like success.
#[test]
fn a_source_that_cannot_observe_a_filtered_attribute_is_refused_not_returned_unfiltered() {
    let root = corpus();
    let fs = FilesystemSource::new(&root);
    let front = FrontmatterSource::new(&root);
    let sources: Vec<&dyn DataSource> = vec![&fs, &front];
    let query = federated_query(&sources, "path:notes/** title:S*");

    let executor = Executor::new(ExecutionPolicy::default());
    let answer = execute_query(&executor, &sources, &query).expect("executes");

    // The filesystem would have had plenty to say if it had been let through
    // unfiltered — that is exactly what must not happen.
    let would_have = federated_query(&sources, "path:notes/**");
    let unfiltered = read_source(&fs, &would_have).expect("reads").rows.len();
    assert!(unfiltered > 0, "the fixture must give the filesystem rows");

    assert!(
        !answer
            .rows
            .iter()
            .any(|row| row.source.as_str() == FilesystemSource::ID),
        "the filesystem cannot observe `title`, so none of its rows may appear"
    );
    assert!(
        answer
            .rows
            .iter()
            .all(|row| row.source.as_str() == FrontmatterSource::ID),
        "only the source that observes `title` may answer"
    );
    assert!(!answer.rows.is_empty(), "frontmatter must answer something");

    let report = answer
        .report(&SourceId::new(FilesystemSource::ID))
        .expect("reported");
    match &report.outcome {
        Outcome::Refused { reason } => assert!(
            matches!(reason, SourceError::UnobservableAttribute { attr, .. } if attr == "title"),
            "the refusal must name the attribute: {reason}"
        ),
        other => panic!("the filesystem must be refused, got {other:?}"),
    }

    // An answer that consulted fewer sources than the federation holds is not
    // complete, and says so with the sources it left out.
    match answer.completeness() {
        Completeness::Partial { excluded } => {
            assert_eq!(excluded.len(), 1);
            assert_eq!(excluded[0].source.as_str(), FilesystemSource::ID);
            assert!(!excluded[0].reason.is_empty(), "an exclusion must say why");
        }
        Completeness::Complete => panic!("an answer missing a source is not complete"),
    }
}

#[test]
fn legacy_execution_reports_successes_and_refusals_in_caller_input_order() {
    let root = corpus();
    let front = FrontmatterSource::new(&root);
    let fs = FilesystemSource::new(&root);
    let sources: Vec<&dyn DataSource> = vec![&front, &fs];
    let query = federated_query(&sources, "path:notes/** title:S*");
    let executor = Executor::new(ExecutionPolicy::default());

    let answer = execute_query(&executor, &sources, &query).expect("partial execution succeeds");

    assert_eq!(
        answer
            .reports
            .iter()
            .map(|report| report.source.as_str())
            .collect::<Vec<_>>(),
        [FrontmatterSource::ID, FilesystemSource::ID],
        "report order is the source input order even when the latter source refuses compilation"
    );
    assert!(matches!(answer.reports[0].outcome, Outcome::Ran(_)));
    assert!(matches!(answer.reports[1].outcome, Outcome::Refused { .. }));
}

/// Residuals are **ANDed**: a row has to pass every one of them. Passing one is
/// not passing the filter.
///
/// Worth its own test because the failure mode is quiet — under an OR every row
/// the pushed prefix returned would satisfy the `path` half and come straight
/// back, and the answer would over-return with a row count that looks entirely
/// reasonable.
#[test]
fn a_row_must_pass_every_residual_not_merely_one_of_them() {
    let root = corpus();
    let fs = FilesystemSource::new(&root);
    let front = FrontmatterSource::new(&root);
    let sources: Vec<&dyn DataSource> = vec![&front];
    let query = federated_query(&[&fs, &front], "path:notes/** title:S*");

    let executor = Executor::new(ExecutionPolicy::default());
    let answer = execute_query(&executor, &sources, &query).expect("executes");

    let ran = answer
        .report(&SourceId::new(FrontmatterSource::ID))
        .expect("reported")
        .ran()
        .expect("ran");
    assert_eq!(
        ran.residual,
        vec!["path".to_string(), "title".to_string()],
        "this test needs two residuals to mean anything"
    );
    assert!(!answer.rows.is_empty(), "the fixture must match something");
    assert!(
        ran.returned < ran.scanned,
        "scanned {} and returned {}: both residuals must bite",
        ran.scanned,
        ran.returned
    );
    for row in &answer.rows {
        let title = row
            .value
            .get("title")
            .and_then(|value| value.as_str())
            .expect("a row filtered on `title` must carry one");
        assert!(
            title.starts_with('S'),
            "`{title}` does not satisfy `title:S*`"
        );
        assert!(
            row.key.starts_with("notes/"),
            "`{}` does not satisfy `path:notes/**`",
            row.key
        );
    }
}

/// The reported mode is not a label: with the default policy nothing is cached,
/// so a second identical query physically reads again — and the report says
/// `virtual` both times.
#[test]
fn the_reported_mode_matches_what_actually_ran() {
    let root = corpus();
    let fs = Counting::new(FilesystemSource::new(&root));
    let sources: Vec<&dyn DataSource> = vec![&fs];
    let query = federated_query(&sources, "path:notes/**");

    let executor = Executor::new(ExecutionPolicy::default());
    for _ in 0..2 {
        let answer = execute_query(&executor, &sources, &query).expect("executes");
        let ran = answer
            .report(&SourceId::new(FilesystemSource::ID))
            .expect("reported")
            .ran()
            .expect("ran");
        assert_eq!(ran.execution, Execution::Virtual);
    }
    assert_eq!(
        fs.executed(),
        2,
        "virtual execution caches nothing, so both queries must have read"
    );
}

/// The report says where the pushdown line fell for each source, so a
/// capability declaration is checkable from the outside.
#[test]
fn the_report_names_both_halves_of_each_source_plan() {
    let root = corpus();
    let fs = FilesystemSource::new(&root);
    let front = FrontmatterSource::new(&root);
    let sources: Vec<&dyn DataSource> = vec![&fs, &front];
    let query = federated_query(&sources, &format!("path:{PATTERN}"));

    let executor = Executor::new(ExecutionPolicy::default());
    let answer = execute_query(&executor, &sources, &query).expect("executes");

    assert_eq!(
        answer
            .reports
            .iter()
            .map(|r| r.source.to_string())
            .collect::<Vec<_>>(),
        vec![FilesystemSource::ID, FrontmatterSource::ID],
        "every source is reported exactly once, in the order it was given"
    );
    for report in &answer.reports {
        let ran = report.ran().expect("ran");
        assert_eq!(ran.pushed, vec!["path".to_string()]);
        assert_eq!(ran.residual, vec!["path".to_string()]);
        assert!(
            ran.scanned > ran.returned,
            "`{}` scanned {} and returned {}: the counts must stay distinguishable",
            report.source,
            ran.scanned,
            ran.returned
        );
        assert!(ran.execution.is_fresh());
    }
}

/// A query every source ran and none matched is a **complete** answer with a
/// scan count — not a failure, and not indistinguishable from one.
#[test]
fn matching_nothing_is_a_complete_answer_with_a_scan_count() {
    let root = corpus();
    let fs = FilesystemSource::new(&root);
    let front = FrontmatterSource::new(&root);
    let sources: Vec<&dyn DataSource> = vec![&fs, &front];
    let query = federated_query(&sources, "path:notes/**/nothing-is-called-this-*.md");

    let executor = Executor::new(ExecutionPolicy::default());
    let answer = execute_query(&executor, &sources, &query).expect("executes");

    assert!(answer.rows.is_empty());
    assert_eq!(answer.completeness(), Completeness::Complete);
    for report in &answer.reports {
        let ran = report.ran().expect("ran");
        assert!(
            ran.scanned > 0,
            "`{}` must say it looked at something",
            report.source
        );
        assert_eq!(ran.returned, 0);
    }
}
