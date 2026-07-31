//! Fixtures and test doubles for the execution layer.
//!
//! Every source under test is a *real* one reading the committed
//! `fixtures/corpus`. The doubles here wrap real sources rather than replace
//! them: [`Counting`] only observes how often a real source was executed, and
//! [`Flaky`] only decides whether a real read is allowed to happen. An executor
//! proved correct against mocks has not been proved correct.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};

use async_trait::async_trait;
use catdb_sdk::connectors::{CancellationToken, DataSource, ReadContext, SourceCapabilities, SourceCursor, SourceError, SourceId, SourcePage, SourcePlan, MAX_PAGE_SIZE};
use catdb_sdk::core::Schema;
use catdb_sdk::federation::{Clock, Executor, Federated, FederationError};
use catdb_sdk::query::{parse_query, validate_entity, EntityQuery, EntityTarget, EntityVocabulary};

/// The qualified entity all three adapters describe.
pub const ENTITY: &str = "doc.Document";

/// The committed corpus every local adapter reads.
pub fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus")
}

/// Validate a filter string against the federation of these sources — the
/// honest path: the caller states a query against the *federated* vocabulary
/// without knowing which source will answer it.
pub fn federated_query(sources: &[&dyn DataSource], filters: &str) -> EntityQuery {
    let schema = catdb_sdk::connectors::federate(sources).expect("sources federate");
    query(&schema, filters)
}

/// Validate a filter string against one schema.
pub fn query(schema: &Schema, filters: &str) -> EntityQuery {
    let vocab = EntityVocabulary::derive(std::slice::from_ref(schema));
    validate_entity(
        EntityTarget::parse(ENTITY),
        &parse_query(filters).expect("filter string parses"),
        &vocab,
    )
    .expect("filter string validates against the schema")
}

pub fn read_context() -> ReadContext {
    ReadContext::new(
        uuid::Uuid::new_v4(),
        std::time::Instant::now() + std::time::Duration::from_secs(30),
        100_000,
        128 * 1024 * 1024,
        MAX_PAGE_SIZE,
        CancellationToken::new(),
    )
    .expect("test read context")
}

pub fn execute_query(
    executor: &Executor,
    sources: &[&dyn DataSource],
    query: &EntityQuery,
) -> Result<Federated, FederationError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(executor.execute(sources, query, &read_context()))
}

pub fn execute_source(
    source: &dyn DataSource,
    plan: &SourcePlan,
) -> Result<SourcePage, SourceError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(source.execute_page(plan, None, &read_context()))
}

pub fn read_source(
    source: &dyn DataSource,
    query: &EntityQuery,
) -> Result<catdb_sdk::connectors::Answer, SourceError> {
    let fields = source.capabilities().observed();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(catdb_sdk::connectors::read(
            source,
            query,
            &fields,
            &read_context(),
        ))
}

/// A clock the test moves by hand, so an age is asserted rather than slept for.
#[derive(Debug)]
pub struct ManualClock {
    now_ms: AtomicI64,
}

impl ManualClock {
    /// Start at an arbitrary but fixed epoch instant.
    pub fn new(start_ms: i64) -> Self {
        Self {
            now_ms: AtomicI64::new(start_ms),
        }
    }

    /// Move time forward (or, with a negative delta, backwards).
    pub fn advance(&self, delta_ms: i64) {
        self.now_ms.fetch_add(delta_ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.now_ms.load(Ordering::SeqCst)
    }
}

/// A real source that counts how many times it was physically executed.
///
/// This is how "the reported mode matches what actually ran" is checked: the
/// report says `virtual` or `materialized`, and the counter says whether a read
/// happened.
#[derive(Debug)]
pub struct Counting<S> {
    inner: S,
    executed: AtomicUsize,
}

impl<S: DataSource> Counting<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            executed: AtomicUsize::new(0),
        }
    }

    /// How many times the wrapped source actually read.
    pub fn executed(&self) -> usize {
        self.executed.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl<S: DataSource> DataSource for Counting<S> {
    fn id(&self) -> &SourceId {
        self.inner.id()
    }
    fn schema(&self) -> &Schema {
        self.inner.schema()
    }
    fn capabilities(&self) -> &SourceCapabilities {
        self.inner.capabilities()
    }
    fn source_spec(&self) -> serde_json::Value {
        self.inner.source_spec()
    }
    fn compile(&self, q: &EntityQuery, fields: &[String]) -> Result<SourcePlan, SourceError> {
        self.inner.compile(q, fields)
    }
    async fn execute_page(
        &self,
        plan: &SourcePlan,
        cursor: Option<&SourceCursor>,
        context: &ReadContext,
    ) -> Result<SourcePage, SourceError> {
        self.executed.fetch_add(1, Ordering::SeqCst);
        self.inner.execute_page(plan, cursor, context).await
    }
}

/// A real source whose *read* can be made to fail on demand.
///
/// Compilation is untouched, so the failure is an execution failure — the kind
/// that must fail the whole query rather than contribute zero rows.
#[derive(Debug)]
pub struct Flaky<S> {
    inner: S,
    broken: AtomicBool,
}

impl<S: DataSource> Flaky<S> {
    pub fn broken(inner: S) -> Self {
        Self {
            inner,
            broken: AtomicBool::new(true),
        }
    }

    /// Let the underlying source read from now on.
    pub fn repair(&self) {
        self.broken.store(false, Ordering::SeqCst);
    }
}

#[async_trait]
impl<S: DataSource> DataSource for Flaky<S> {
    fn id(&self) -> &SourceId {
        self.inner.id()
    }
    fn schema(&self) -> &Schema {
        self.inner.schema()
    }
    fn capabilities(&self) -> &SourceCapabilities {
        self.inner.capabilities()
    }
    fn source_spec(&self) -> serde_json::Value {
        self.inner.source_spec()
    }
    fn compile(&self, q: &EntityQuery, fields: &[String]) -> Result<SourcePlan, SourceError> {
        self.inner.compile(q, fields)
    }
    async fn execute_page(
        &self,
        plan: &SourcePlan,
        cursor: Option<&SourceCursor>,
        context: &ReadContext,
    ) -> Result<SourcePage, SourceError> {
        if self.broken.load(Ordering::SeqCst) {
            return Err(SourceError::Read {
                source_id: self.inner.id().clone(),
                path: "<the whole source>".to_string(),
                detail: "the underlying store is unreachable".to_string(),
            });
        }
        self.inner.execute_page(plan, cursor, context).await
    }
}

/// An independent brute-force oracle: walk the tree with nothing but `std::fs`
/// and answer "does this path match?" directly.
///
/// It shares no code with the executor — no plan, no capability declaration, no
/// pushdown, no residual. That is the point: if the two agree, the pushdown
/// split did not change the answer.
pub fn brute_force_paths(root: &Path, pattern: &str, extensions: Option<&[&str]>) -> Vec<String> {
    let glob = catdb_sdk::query::Glob::new(pattern);
    let mut all = Vec::new();
    collect(root, root, &mut all);
    all.sort();
    all.into_iter()
        .filter(|rel| match extensions {
            None => true,
            Some(exts) => rel
                .rsplit_once('.')
                .is_some_and(|(_, ext)| exts.contains(&ext)),
        })
        .filter(|rel| glob.matches(rel))
        .collect()
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out);
        } else {
            out.push(
                path.strip_prefix(root)
                    .expect("under root")
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/"),
            );
        }
    }
}

/// The `(source, key)` pairs of an answer, sorted — the comparable shape of a
/// federated result.
pub fn keyed(rows: &[catdb_sdk::connectors::SourceRow]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = rows
        .iter()
        .map(|row| (row.source.to_string(), row.key.clone()))
        .collect();
    pairs.sort();
    pairs
}

/// A directory tree written from `(relative path, contents)` pairs.
pub fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    for (rel, contents) in files {
        write(dir.path(), rel, contents);
    }
    dir
}

/// Write one file under a root, creating parents.
pub fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write");
}
