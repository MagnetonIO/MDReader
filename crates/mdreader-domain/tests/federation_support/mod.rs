//! Fixtures for the federated-`Document` tests.
//!
//! Every test here runs against a **real** tree: the committed
//! `fixtures/corpus` at the repository root, or a temporary git repository
//! built by [`git_fixture`]. Nothing is mocked — a federation that only works
//! over a fake filesystem has not been tested at all — and every read is
//! bracketed by [`tree_digest`], which hashes bytes *and* modification times,
//! because the premise under test is that the sources are never modified.

#![allow(dead_code)] // each integration test uses a different subset

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use catdb_sdk::connectors::{CancellationToken, DataSource, ReadContext};
use catdb_sdk::federation::{Executor, Federated, FederationError};
use catdb_sdk::query::EntityQuery;
use uuid::Uuid;

pub fn read_context() -> ReadContext {
    ReadContext::new(
        Uuid::new_v4(),
        Instant::now() + Duration::from_secs(30),
        100_000,
        128 * 1024 * 1024,
        128,
        CancellationToken::new(),
    )
    .expect("test read context")
}

pub fn execute_query(
    executor: &mut Executor,
    sources: &[&dyn DataSource],
    query: &EntityQuery,
) -> Result<Federated, FederationError> {
    let context = read_context();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(executor.execute(sources, query, &context))
}

/// The committed corpus every adapter reads.
pub fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus")
}

/// A content digest of a whole directory tree: every file's relative path,
/// length, modification time, and bytes.
///
/// The modification time is in the digest on purpose — a read that merely
/// *touched* a file would leave the bytes identical, and touching a source is
/// still modifying it.
///
/// `.git` is excluded: git's own plumbing may refresh its index's stat cache
/// as a side effect of reading, which is git housekeeping rather than a change
/// to the source. What the adapters must not touch — the work tree, and the
/// set of tracked/staged paths — is covered here and by [`git_status`].
pub fn tree_digest(root: &Path) -> String {
    let mut hasher = blake3::Hasher::new();
    let mut entries: Vec<PathBuf> = Vec::new();
    collect(root, &mut entries);
    entries.sort();
    for path in entries {
        let rel = path.strip_prefix(root).expect("under root");
        let meta = std::fs::metadata(&path).expect("stat");
        let mtime = meta
            .modified()
            .expect("mtime")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after epoch")
            .as_nanos();
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update(&meta.len().to_le_bytes());
        hasher.update(&mtime.to_le_bytes());
        hasher.update(&std::fs::read(&path).expect("read"));
    }
    hasher.finalize().to_hex().to_string()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        if path.is_dir() {
            collect(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// The repository's porcelain status for one subtree: which paths are tracked,
/// staged, or dirty. A read-only federation must leave it exactly as it found
/// it.
pub fn git_status(root: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "."])
        .output()
        .expect("git status runs");
    assert!(output.status.success(), "git status failed");
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// The last commit's author for one path, straight from git — so a test can
/// assert the federated value IS the commit author without hard-coding whoever
/// happens to have committed the corpus.
pub fn git_author(root: &Path, rel: &str) -> String {
    git_field(root, rel, "%an")
}

/// The last commit's committer timestamp (epoch seconds) for one path.
pub fn git_commit_seconds(root: &Path, rel: &str) -> i64 {
    git_field(root, rel, "%ct").parse().expect("epoch seconds")
}

fn git_field(root: &Path, rel: &str, format: &str) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["log", "-1", &format!("--format={format}"), "--", rel])
        .output()
        .expect("git log runs");
    assert!(output.status.success(), "git log failed for {rel}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// The filesystem mtime of one path, in epoch milliseconds — the exact form
/// the filesystem adapter declares.
pub fn fs_modified_ms(root: &Path, rel: &str) -> i64 {
    let meta = std::fs::metadata(root.join(rel)).expect("stat");
    i64::try_from(
        meta.modified()
            .expect("mtime")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after epoch")
            .as_millis(),
    )
    .expect("fits in i64")
}

/// The epoch seconds every commit in [`git_fixture`] is pinned to.
pub const FIXTURE_COMMIT_SECONDS: i64 = 1_700_000_000;

/// The author every commit in [`git_fixture`] is attributed to.
pub const FIXTURE_AUTHOR: &str = "Fixture Author";

/// A temporary git repository with three markdown documents:
///
/// * `docs/committed.md` — committed, no YAML at all;
/// * `docs/declared.md` — committed, with a YAML `title:` and `author:` that
///   differ from its H1 and its committer;
/// * `docs/staged.md` — added to the index but never committed.
///
/// The staged file is the point of the fixture as much as the committed ones:
/// git genuinely has no commit author or timestamp for it, so a field only git
/// could have answered has to come out **absent**, not invented.
pub fn git_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();
    git(root, &["init", "--quiet"]);
    write(root, "docs/committed.md", "# Committed\n\nbody\n");
    write(
        root,
        "docs/declared.md",
        "---\ntitle: Declared Title\nauthor: Declared Author\n---\n\n# Heading Not Title\n\nbody\n",
    );
    git(root, &["add", "docs/committed.md", "docs/declared.md"]);
    let stamp = format!("{FIXTURE_COMMIT_SECONDS} +0000");
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            &format!("user.name={FIXTURE_AUTHOR}"),
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ])
        .env("GIT_AUTHOR_DATE", &stamp)
        .env("GIT_COMMITTER_DATE", &stamp)
        .output()
        .expect("git commit runs");
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    write(root, "docs/staged.md", "# Staged\n\nbody\n");
    git(root, &["add", "docs/staged.md"]);
    dir
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("git {args:?} runs: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write");
}

/// A directory tree written from `(relative path, contents)` pairs.
pub fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    for (rel, contents) in files {
        write(dir.path(), rel, contents);
    }
    dir
}
