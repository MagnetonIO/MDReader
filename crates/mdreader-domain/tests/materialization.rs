//! On-demand materialization: what a cached answer promises, and what it does
//! not.

mod exec_support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use catdb_sdk::connectors::{CancellationToken, DataSource, ReadContext, SourceAttr, SourceCapabilities, SourceCursor, SourceError, SourceId, SourceModel, SourcePage, SourcePlan, SourceRow};
use mdreader_connector_local::{FilesystemSource};
use catdb_sdk::federation::{
    Clock, Execution, ExecutionPolicy, Executor, FederationError, Freshness, SystemClock,
};
use exec_support::{corpus, execute_query, federated_query, tree, write, Counting, Flaky, ManualClock};

/// An arbitrary but fixed wall-clock instant the manual clock starts at.
const T0: i64 = 1_700_000_000_000;

fn fs_id() -> SourceId {
    SourceId::new(FilesystemSource::ID)
}

fn materialized(max_age: Duration) -> ExecutionPolicy {
    ExecutionPolicy::default().materialize(fs_id(), max_age)
}

#[derive(Debug)]
struct SparseWorkSource {
    model: SourceModel,
    dispatched_page_size: AtomicUsize,
}

impl SparseWorkSource {
    fn new() -> Self {
        Self {
            model: SourceModel::new(
                SourceId::new("sparse-work"),
                "doc",
                "Document",
                vec![SourceAttr::text("path")],
            ),
            dispatched_page_size: AtomicUsize::new(0),
        }
    }

    fn dispatched_page_size(&self) -> usize {
        self.dispatched_page_size.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DataSource for SparseWorkSource {
    fn id(&self) -> &SourceId {
        self.model.id()
    }

    fn schema(&self) -> &catdb_sdk::core::Schema {
        self.model.schema()
    }

    fn capabilities(&self) -> &SourceCapabilities {
        self.model.capabilities()
    }

    fn source_spec(&self) -> serde_json::Value {
        serde_json::json!({"kind": "sparse-work"})
    }

    fn compile(
        &self,
        query: &catdb_sdk::query::EntityQuery,
        authorized_fields: &[String],
    ) -> Result<SourcePlan, SourceError> {
        self.model.compile(query, authorized_fields)
    }

    async fn execute_page(
        &self,
        _plan: &SourcePlan,
        _cursor: Option<&SourceCursor>,
        context: &ReadContext,
    ) -> Result<SourcePage, SourceError> {
        self.dispatched_page_size
            .store(context.page_size(), Ordering::SeqCst);
        Ok(SourcePage {
            source: self.id().clone(),
            scanned: 5,
            rows: vec![SourceRow {
                source: self.id().clone(),
                entity: "doc.Document".to_string(),
                key: "one".to_string(),
                value: serde_json::json!({"path": "one"}),
            }],
            next_cursor: None,
            observed_at_ms: T0,
            freshness_ms: 1,
        })
    }
}

#[tokio::test]
async fn legacy_federation_reserves_and_narrows_budget_before_source_dispatch() {
    let source = SparseWorkSource::new();
    let sources: Vec<&dyn DataSource> = vec![&source];
    let query = federated_query(&sources, "");
    let executor = Executor::new(ExecutionPolicy::default());
    let context = ReadContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_secs(5),
        1,
        1_000_000,
        100,
        CancellationToken::new(),
    )
    .unwrap();

    assert!(matches!(
        executor.execute(&sources, &query, &context).await,
        Err(FederationError::RowBudgetExceeded)
    ));
    assert_eq!(
        source.dispatched_page_size(),
        1,
        "the source must receive only the pre-reserved work quantum"
    );
}

/// The load-bearing property: a reused answer is a *different value* from a
/// fresh one, and it carries both when it was taken and how old it is. A caller
/// can never be looking at cached data without being told.
#[test]
fn a_reused_answer_is_distinguishable_from_a_fresh_one_and_reports_its_age() {
    let root = corpus();
    let fs = Counting::new(FilesystemSource::new(&root));
    let sources: Vec<&dyn DataSource> = vec![&fs];
    let query = federated_query(&sources, "path:notes/**");

    let clock = Arc::new(ManualClock::new(T0));
    let executor = Executor::with_clock(materialized(Duration::from_secs(60)), clock.clone());

    let first = execute_query(&executor, &sources, &query).expect("executes");
    let fresh = first
        .report(&fs_id())
        .expect("reported")
        .ran()
        .expect("ran");
    assert_eq!(fresh.execution, Execution::Materialized(Freshness::Fresh));
    assert_eq!(fresh.taken_at_ms, T0);
    assert!(fresh.execution.is_fresh());
    assert_eq!(fs.executed(), 1);

    clock.advance(5_000);
    let second = execute_query(&executor, &sources, &query).expect("executes");
    let reused = second
        .report(&fs_id())
        .expect("reported")
        .ran()
        .expect("ran");
    assert_eq!(
        reused.execution,
        Execution::Materialized(Freshness::Cached { age_ms: 5_000 }),
        "a reused answer must report its age"
    );
    assert!(!reused.execution.is_fresh());
    assert_eq!(
        reused.taken_at_ms, T0,
        "a cached answer is dated when it was taken, not when it was served"
    );
    assert_ne!(
        fresh.execution, reused.execution,
        "fresh and reused must not be the same value"
    );
    assert_eq!(fs.executed(), 1, "the second query must not have read");
    assert_eq!(first.rows, second.rows);
}

#[tokio::test]
async fn a_cached_answer_is_charged_to_the_current_queries_global_budgets() {
    let root = corpus();
    let fs = Counting::new(FilesystemSource::new(&root));
    let sources: Vec<&dyn DataSource> = vec![&fs];
    let query = federated_query(&sources, "path:notes/**");
    let executor = Executor::new(materialized(Duration::from_secs(60)));
    let generous = ReadContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_secs(5),
        1_000,
        1_000_000,
        100,
        CancellationToken::new(),
    )
    .unwrap();
    executor
        .execute(&sources, &query, &generous)
        .await
        .expect("primes the cache");

    let tight = ReadContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_secs(5),
        1,
        1_000_000,
        100,
        CancellationToken::new(),
    )
    .unwrap();
    assert!(matches!(
        executor.execute(&sources, &query, &tight).await,
        Err(FederationError::RowBudgetExceeded)
    ));
    assert_eq!(fs.executed(), 1, "the second answer came from the cache");
}

#[tokio::test]
async fn cached_answers_charge_original_physical_scans_not_only_returned_rows() {
    let source = SparseWorkSource::new();
    let sources: Vec<&dyn DataSource> = vec![&source];
    let query = federated_query(&sources, "");
    let executor = Executor::new(
        ExecutionPolicy::default()
            .materialize(SourceId::new("sparse-work"), Duration::from_secs(60)),
    );
    let generous = ReadContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_secs(5),
        10,
        1_000_000,
        10,
        CancellationToken::new(),
    )
    .unwrap();
    executor
        .execute(&sources, &query, &generous)
        .await
        .expect("primes the sparse cache");

    let rows_only_budget = ReadContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_secs(5),
        1,
        1_000_000,
        10,
        CancellationToken::new(),
    )
    .unwrap();
    assert!(matches!(
        executor.execute(&sources, &query, &rows_only_budget).await,
        Err(FederationError::RowBudgetExceeded)
    ));
}

/// An entry past its declared maximum age is never served. The re-read is
/// itself reported as fresh, and re-dated.
#[test]
fn an_entry_older_than_its_max_age_is_re_read() {
    let root = corpus();
    let fs = Counting::new(FilesystemSource::new(&root));
    let sources: Vec<&dyn DataSource> = vec![&fs];
    let query = federated_query(&sources, "path:notes/**");

    let clock = Arc::new(ManualClock::new(T0));
    let executor = Executor::with_clock(materialized(Duration::from_secs(10)), clock.clone());

    execute_query(&executor, &sources, &query).expect("executes");
    clock.advance(9_999);
    execute_query(&executor, &sources, &query).expect("executes");
    assert_eq!(fs.executed(), 1, "still inside the window");

    clock.advance(1);
    let after = execute_query(&executor, &sources, &query).expect("executes");
    let ran = after
        .report(&fs_id())
        .expect("reported")
        .ran()
        .expect("ran");
    assert_eq!(ran.execution, Execution::Materialized(Freshness::Fresh));
    assert_eq!(ran.taken_at_ms, T0 + 10_000);
    assert_eq!(fs.executed(), 2, "an expired entry must be re-read");
}

/// A zero maximum age is not a cache that "usually" refreshes: it never serves
/// a reused answer at all, because an age of zero is already not less than zero.
#[test]
fn a_zero_max_age_never_reuses() {
    let root = corpus();
    let fs = Counting::new(FilesystemSource::new(&root));
    let sources: Vec<&dyn DataSource> = vec![&fs];
    let query = federated_query(&sources, "path:notes/**");

    let clock = Arc::new(ManualClock::new(T0));
    let executor = Executor::with_clock(materialized(Duration::ZERO), clock.clone());
    execute_query(&executor, &sources, &query).expect("executes");
    let again = execute_query(&executor, &sources, &query).expect("executes");
    assert_eq!(fs.executed(), 2);
    assert!(again
        .report(&fs_id())
        .expect("reported")
        .ran()
        .expect("ran")
        .execution
        .is_fresh());
}

/// What is cached is the **pushed half** — the physical read — and nothing
/// narrower.
///
/// The order matters and is the point: the *narrow* query runs first and stores
/// the entry. If what were stored were its answer rather than its read, the
/// wider query behind it would be served the narrow query's leftovers and would
/// under-return, with a perfectly plausible-looking row count. So the reused
/// answer is compared against a live, uncached read of the same query.
#[test]
fn a_cached_read_is_the_pushed_half_only_so_a_later_residual_sees_every_row() {
    let root = corpus();
    let fs = Counting::new(FilesystemSource::new(&root));
    let sources: Vec<&dyn DataSource> = vec![&fs];
    let wide = federated_query(&sources, "path:notes/**");
    let narrow = federated_query(&sources, "path:notes/**/node-*.md");

    let clock = Arc::new(ManualClock::new(T0));
    let executor = Executor::with_clock(materialized(Duration::from_secs(60)), clock.clone());

    let some = execute_query(&executor, &sources, &narrow).expect("executes");
    clock.advance(1_000);
    let all = execute_query(&executor, &sources, &wide).expect("executes");

    assert_eq!(
        fs.executed(),
        1,
        "both queries push `notes/`, so one read answers both"
    );
    assert_eq!(
        all.report(&fs_id())
            .expect("reported")
            .ran()
            .expect("ran")
            .execution,
        Execution::Materialized(Freshness::Cached { age_ms: 1_000 })
    );
    assert!(
        some.rows.len() < all.rows.len() && !some.rows.is_empty(),
        "the residual is re-applied per query: {} of {}",
        some.rows.len(),
        all.rows.len()
    );

    // The reused answer must be the whole answer, not what the previous
    // query's residual happened to leave behind.
    let live_fs = FilesystemSource::new(&root);
    let live_sources: Vec<&dyn DataSource> = vec![&live_fs];
    let live_executor = Executor::new(ExecutionPolicy::default());
    let live = execute_query(&live_executor, &live_sources, &wide).expect("executes");
    assert_eq!(
        all.rows.len(),
        live.rows.len(),
        "a cached read must answer the wider query exactly as a live read would"
    );
}

/// A cache entry answers only the read it was taken for. A query that pushes
/// something different reads again rather than being served a narrower or wider
/// answer that merely looks plausible.
#[test]
fn a_different_pushed_half_is_a_different_cache_entry() {
    let root = corpus();
    let fs = Counting::new(FilesystemSource::new(&root));
    let sources: Vec<&dyn DataSource> = vec![&fs];
    let notes = federated_query(&sources, "path:notes/**");
    let docs = federated_query(&sources, "path:docs/**");

    let clock = Arc::new(ManualClock::new(T0));
    let executor = Executor::with_clock(materialized(Duration::from_secs(60)), clock.clone());

    let a = execute_query(&executor, &sources, &notes).expect("executes");
    let b = execute_query(&executor, &sources, &docs).expect("executes");
    assert_eq!(fs.executed(), 2, "different pushdowns are different reads");
    assert!(b
        .report(&fs_id())
        .expect("reported")
        .ran()
        .expect("ran")
        .execution
        .is_fresh());
    assert!(a.rows.iter().all(|r| r.key.starts_with("notes/")));
    assert!(b.rows.iter().all(|r| r.key.starts_with("docs/")));
}

#[test]
fn cache_identity_includes_the_verified_source_instance_and_source_spec() {
    let left_root = tree(&[("left.md", "left")]);
    let right_root = tree(&[("right.md", "right")]);
    let left = FilesystemSource::new(left_root.path());
    let right = FilesystemSource::new(right_root.path());
    let left_sources: Vec<&dyn DataSource> = vec![&left];
    let right_sources: Vec<&dyn DataSource> = vec![&right];
    let left_query = federated_query(&left_sources, "");
    let right_query = federated_query(&right_sources, "");
    let clock = Arc::new(ManualClock::new(T0));
    let executor = Executor::with_clock(materialized(Duration::from_secs(60)), clock);

    let first = execute_query(&executor, &left_sources, &left_query).unwrap();
    let second = execute_query(&executor, &right_sources, &right_query).unwrap();

    assert_eq!(first.rows[0].key, "left.md");
    assert_eq!(
        second.rows[0].key, "right.md",
        "equal SourceId and pushdown are not enough to reuse another configured source instance"
    );
    assert!(second
        .report(&fs_id())
        .unwrap()
        .ran()
        .unwrap()
        .execution
        .is_fresh());
}

/// A failure is never remembered. Only an answer that ran is stored, so a
/// broken source cannot poison the cache into serving "no rows" later.
#[test]
fn a_failed_read_is_never_cached() {
    let root = corpus();
    let fs = Flaky::broken(FilesystemSource::new(&root));
    let sources: Vec<&dyn DataSource> = vec![&fs];
    let query = federated_query(&sources, "path:notes/**");

    let clock = Arc::new(ManualClock::new(T0));
    let executor = Executor::with_clock(materialized(Duration::from_secs(60)), clock.clone());

    assert!(
        execute_query(&executor, &sources, &query).is_err(),
        "a broken source must fail the query"
    );
    fs.repair();
    clock.advance(1_000);
    let answer = execute_query(&executor, &sources, &query).expect("executes");
    assert!(
        answer
            .report(&fs_id())
            .expect("reported")
            .ran()
            .expect("ran")
            .execution
            .is_fresh(),
        "the repaired source must be read, not served from a remembered failure"
    );
    assert!(!answer.rows.is_empty());
}

/// **The documented non-guarantee, as a test.** The cache is time-based only:
/// it does not watch the source, so a document added after an entry was taken is
/// not in the reused answer. What it does guarantee is that the caller is told —
/// the answer is reported as cached, with its age — so stale data is visible
/// rather than silent.
#[test]
fn a_cached_answer_does_not_see_a_change_but_never_hides_that_it_is_old() {
    let dir = tree(&[("docs/one.md", "# One\n"), ("docs/two.md", "# Two\n")]);
    let fs = Counting::new(FilesystemSource::new(dir.path()));
    let sources: Vec<&dyn DataSource> = vec![&fs];
    let query = federated_query(&sources, "path:docs/**");

    let clock = Arc::new(ManualClock::new(T0));
    let executor = Executor::with_clock(materialized(Duration::from_secs(60)), clock.clone());

    let before = execute_query(&executor, &sources, &query).expect("executes");
    assert_eq!(before.rows.len(), 2);

    write(dir.path(), "docs/three.md", "# Three\n");
    clock.advance(30_000);
    let during = execute_query(&executor, &sources, &query).expect("executes");
    assert_eq!(
        during.rows.len(),
        2,
        "the cache does not watch the source — this is the documented limit"
    );
    assert_eq!(
        during
            .report(&fs_id())
            .expect("reported")
            .ran()
            .expect("ran")
            .execution,
        Execution::Materialized(Freshness::Cached { age_ms: 30_000 }),
        "and the caller is told, in the same breath, that it is 30s old"
    );

    clock.advance(30_001);
    let after = execute_query(&executor, &sources, &query).expect("executes");
    assert_eq!(after.rows.len(), 3, "past the window, the change is seen");
    assert_eq!(fs.executed(), 2);
}

/// A clock that moves backwards makes an age unmeasurable. The entry is re-read
/// rather than served with a nonsense age — the same fail-closed rule as an
/// absent attribute.
#[test]
fn a_clock_that_goes_backwards_re_reads_rather_than_inventing_an_age() {
    let root = corpus();
    let fs = Counting::new(FilesystemSource::new(&root));
    let sources: Vec<&dyn DataSource> = vec![&fs];
    let query = federated_query(&sources, "path:notes/**");

    let clock = Arc::new(ManualClock::new(T0));
    let executor = Executor::with_clock(materialized(Duration::from_secs(600)), clock.clone());

    execute_query(&executor, &sources, &query).expect("executes");
    clock.advance(-1);
    let answer = execute_query(&executor, &sources, &query).expect("executes");
    assert!(answer
        .report(&fs_id())
        .expect("reported")
        .ran()
        .expect("ran")
        .execution
        .is_fresh());
    assert_eq!(fs.executed(), 2);
}

/// The report renders in words, because "never have to guess whether you are
/// looking at live data" has to survive being printed.
#[test]
fn a_rendered_report_says_the_mode_and_the_age_in_words() {
    let root = corpus();
    let fs = FilesystemSource::new(&root);
    let sources: Vec<&dyn DataSource> = vec![&fs];
    let query = federated_query(&sources, "path:notes/**");

    let clock = Arc::new(ManualClock::new(T0));
    let executor = Executor::with_clock(materialized(Duration::from_secs(60)), clock.clone());
    execute_query(&executor, &sources, &query).expect("executes");
    clock.advance(42_000);
    let answer = execute_query(&executor, &sources, &query).expect("executes");

    let report = answer.report(&fs_id()).expect("reported");
    let rendered = report.to_string();
    assert!(rendered.contains("materialized"), "{rendered}");
    assert!(rendered.contains("cached 42000ms ago"), "{rendered}");
    assert_eq!(report.ran().expect("ran").execution.mode(), "materialized");
    assert_eq!(report.ran().expect("ran").execution.age_ms(), 42_000);
    assert!(
        answer.to_string().contains("complete"),
        "the whole answer renders too: {answer}"
    );
}

/// The default executor runs on a clock that is really the wall clock.
///
/// Everything else here uses a hand-driven clock, which is what makes ages
/// assertable — and which would happily keep passing over a [`SystemClock`] that
/// had stopped. A stopped clock is not a cosmetic bug: every reused answer would
/// report an age of zero forever, which is stale data presented as fresh.
#[test]
fn the_default_executor_runs_on_a_wall_clock_that_actually_moves() {
    let now = SystemClock.now_ms();
    // A plausible present, in milliseconds: 2020-01-01 < now < 2100-01-01. A
    // constant zero fails here, and so does a second count worn as a
    // millisecond count.
    assert!(
        (1_577_836_800_000..4_102_444_800_000).contains(&now),
        "{now} is not a millisecond instant in the present"
    );

    let start = std::time::Instant::now();
    let mut later = SystemClock.now_ms();
    while later == now && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(1));
        later = SystemClock.now_ms();
    }
    assert!(later > now, "the wall clock did not advance");

    // And that is the clock `Executor::new` dates its answers with.
    let root = corpus();
    let fs = FilesystemSource::new(&root);
    let sources: Vec<&dyn DataSource> = vec![&fs];
    let query = federated_query(&sources, "path:notes/**");
    let before = SystemClock.now_ms();
    let executor = Executor::new(materialized(Duration::from_secs(60)));
    let answer = execute_query(&executor, &sources, &query).expect("executes");
    let after = SystemClock.now_ms();
    let taken = answer
        .report(&fs_id())
        .expect("reported")
        .ran()
        .expect("ran")
        .taken_at_ms;
    assert!(
        (before..=after).contains(&taken),
        "an answer taken at {taken} is not between {before} and {after}"
    );
}

/// A source with no materialization policy is never cached, even alongside one
/// that is. The choice is per source, and it is the caller's declaration.
#[test]
fn a_source_without_a_materialization_policy_is_never_cached() {
    let root = corpus();
    let fs = Counting::new(FilesystemSource::new(&root));
    let sources: Vec<&dyn DataSource> = vec![&fs];
    let query = federated_query(&sources, "path:notes/**");

    let clock = Arc::new(ManualClock::new(T0));
    // The policy names a *different* source.
    let policy =
        ExecutionPolicy::default().materialize(SourceId::new("git"), Duration::from_secs(60));
    let executor = Executor::with_clock(policy, clock.clone());
    execute_query(&executor, &sources, &query).expect("executes");
    execute_query(&executor, &sources, &query).expect("executes");
    assert_eq!(fs.executed(), 2);
}
