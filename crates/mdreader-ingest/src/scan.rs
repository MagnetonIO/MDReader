//! Source-root scanner: an [`ignore`]-crate walk honoring `.gitignore` and
//! `.contextignore`, hidden-file filtering, size limits, and markdown
//! extension selection, with per-file git-repo identity detection.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Scanner configuration, derived from the catalog source row and import
/// options.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Also pick up `.mdx` files.
    pub include_mdx: bool,
    /// Skip files larger than this many bytes.
    pub max_file_size: u64,
    /// Honor `.gitignore` (and git excludes). `.contextignore` is always
    /// honored.
    pub respect_gitignore: bool,
}

/// One scannable markdown file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFile {
    /// Absolute path on disk.
    pub abs: PathBuf,
    /// Path relative to the scanned root, `/`-separated.
    pub rel: String,
    /// Repository identity: the name of the nearest enclosing git root under
    /// the scan root, or the root's own name as a synthetic repo.
    pub repo: String,
}

fn is_markdown(path: &Path, include_mdx: bool) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => {
            let ext = ext.to_ascii_lowercase();
            ext == "md" || ext == "markdown" || (include_mdx && ext == "mdx")
        }
        None => false,
    }
}

/// Find the repo identity for `file`: walk ancestors up to (and including)
/// `root` looking for a `.git` entry; memoized per directory.
fn repo_identity(
    root: &Path,
    synthetic: &str,
    file: &Path,
    cache: &mut HashMap<PathBuf, Option<String>>,
) -> String {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if let Some(cached) = cache.get(d) {
            if let Some(repo) = cached {
                return repo.clone();
            }
        } else {
            let found = if d.join(".git").exists() {
                Some(
                    d.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| synthetic.to_string()),
                )
            } else {
                None
            };
            cache.insert(d.to_path_buf(), found.clone());
            if let Some(repo) = found {
                return repo.clone();
            }
        }
        if d == root {
            break;
        }
        dir = d.parent();
    }
    synthetic.to_string()
}

/// Convert a walk error into an [`std::io::Error`] that KEEPS the operating
/// system's own [`ErrorKind`](std::io::ErrorKind).
///
/// This is load-bearing. [`std::io::Error::other`] flattens every kind to
/// [`Other`](std::io::ErrorKind::Other), which erased exactly the fact callers
/// need: that the OS *refused* to read the folder (macOS privacy/TCC returns
/// `EPERM`, a unix mode denial returns `EACCES`; both surface as
/// [`PermissionDenied`](std::io::ErrorKind::PermissionDenied)). With the kind
/// erased, a denied walk was indistinguishable from any other I/O failure, and
/// the shell had nothing to tell the user apart from "empty".
fn walk_error(err: ignore::Error) -> std::io::Error {
    match err.io_error().map(std::io::Error::kind) {
        Some(kind) => std::io::Error::new(kind, err.to_string()),
        None => std::io::Error::other(err.to_string()),
    }
}

/// Walk `root` and return every markdown file that passes the filters,
/// sorted by relative path for deterministic imports.
///
/// A walk error is **never** downgraded to a short listing: the first one ends
/// the scan and is returned, so a caller can never mistake "the OS refused us
/// half of this tree" for "this tree is small". A root the OS will not open
/// fails with [`PermissionDenied`](std::io::ErrorKind::PermissionDenied)
/// rather than yielding zero files.
pub fn scan_root(root: &Path, options: &ScanOptions) -> std::io::Result<Vec<ScannedFile>> {
    let synthetic = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "root".to_string());

    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(true)
        .follow_links(false)
        .git_ignore(options.respect_gitignore)
        .git_global(false)
        .git_exclude(options.respect_gitignore)
        .require_git(false)
        .ignore(true)
        .add_custom_ignore_filename(".contextignore")
        .max_filesize(Some(options.max_file_size));

    let mut cache: HashMap<PathBuf, Option<String>> = HashMap::new();
    let mut out = Vec::new();
    for entry in builder.build() {
        let entry = entry.map_err(walk_error)?;
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        if !is_markdown(path, options.include_mdx) {
            continue;
        }
        let rel = match path.strip_prefix(root) {
            Ok(rel) => rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
            Err(_) => continue,
        };
        let repo = repo_identity(root, &synthetic, path, &mut cache);
        out.push(ScannedFile {
            abs: path.to_path_buf(),
            rel,
            repo,
        });
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn opts() -> ScanOptions {
        ScanOptions {
            include_mdx: false,
            max_file_size: 1024 * 1024,
            respect_gitignore: true,
        }
    }

    #[test]
    fn scans_markdown_extensions_only_sorted() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "b.md", "# B");
        write(dir.path(), "a.markdown", "# A");
        write(dir.path(), "c.txt", "not markdown");
        write(dir.path(), "d.mdx", "# D");
        let files = scan_root(dir.path(), &opts()).unwrap();
        let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, vec!["a.markdown", "b.md"]);

        let with_mdx = scan_root(
            dir.path(),
            &ScanOptions {
                include_mdx: true,
                ..opts()
            },
        )
        .unwrap();
        let rels: Vec<&str> = with_mdx.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, vec!["a.markdown", "b.md", "d.mdx"]);
    }

    #[test]
    fn honors_gitignore_and_contextignore_and_hidden() {
        let dir = tempfile::tempdir().unwrap();
        // Make the root a git repo so .gitignore applies.
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        write(dir.path(), ".gitignore", "generated/\n");
        write(dir.path(), ".contextignore", "drafts/\n");
        write(dir.path(), "keep.md", "# K");
        write(dir.path(), "generated/skip.md", "# S");
        write(dir.path(), "drafts/skip-too.md", "# S2");
        write(dir.path(), ".hidden/secret.md", "# H");
        let files = scan_root(dir.path(), &opts()).unwrap();
        let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, vec!["keep.md"]);
        // Gitignore off: generated/ comes back, .contextignore still applies.
        let files = scan_root(
            dir.path(),
            &ScanOptions {
                respect_gitignore: false,
                ..opts()
            },
        )
        .unwrap();
        let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, vec!["generated/skip.md", "keep.md"]);
    }

    #[test]
    fn skips_files_over_the_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "small.md", "# ok");
        write(dir.path(), "big.md", &"x".repeat(4096));
        let files = scan_root(
            dir.path(),
            &ScanOptions {
                max_file_size: 1024,
                ..opts()
            },
        )
        .unwrap();
        let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, vec!["small.md"]);
    }

    /// A root the OS will not let us read is reported as `PermissionDenied` —
    /// NOT as an empty scan. The genuinely-empty root right beside it still
    /// scans clean and returns zero files, so the two outcomes stay distinct.
    ///
    /// Revert-RED: put `std::io::Error::other` back in place of `walk_error`
    /// and the `ErrorKind::PermissionDenied` assertion fails (kind becomes
    /// `Other`). Make `scan_root` swallow walk errors (`continue` instead of
    /// `?`) and the `is_err` assertion fails — the denied root comes back as
    /// an empty `Ok(vec![])`, which is the bug.
    #[test]
    #[cfg(unix)]
    fn an_unreadable_root_is_permission_denied_and_an_empty_root_is_empty() {
        use std::io::ErrorKind;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();

        // A root that genuinely holds no markdown: readable, just empty.
        let empty = dir.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let scanned = scan_root(&empty, &opts()).expect("a readable empty root scans clean");
        assert!(scanned.is_empty(), "and reports zero files: {scanned:?}");

        // A root that DOES hold markdown, which the OS then refuses to open.
        let locked = dir.path().join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        write(&locked, "a.md", "# A");
        let mut mode = std::fs::metadata(&locked).unwrap().permissions();
        mode.set_mode(0o000);
        std::fs::set_permissions(&locked, mode).unwrap();

        let denied = scan_root(&locked, &opts());
        // Restore before asserting so a failure still leaves a removable tmpdir.
        let mut mode = std::fs::metadata(&locked).unwrap().permissions();
        mode.set_mode(0o700);
        std::fs::set_permissions(&locked, mode).unwrap();

        let err = denied.expect_err("a root the OS refuses is an error, never an empty listing");
        assert_eq!(
            err.kind(),
            ErrorKind::PermissionDenied,
            "and it keeps the OS's own kind so callers can name it: {err}"
        );
    }

    #[test]
    fn detects_git_repo_identity_and_synthetic_fallback() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "loose.md", "# L");
        std::fs::create_dir_all(dir.path().join("project/.git")).unwrap();
        write(dir.path(), "project/inside.md", "# I");
        let files = scan_root(dir.path(), &opts()).unwrap();
        let by_rel: HashMap<&str, &str> = files
            .iter()
            .map(|f| (f.rel.as_str(), f.repo.as_str()))
            .collect();
        let synthetic = dir.path().file_name().unwrap().to_string_lossy();
        assert_eq!(by_rel["loose.md"], synthetic);
        assert_eq!(by_rel["project/inside.md"], "project");
    }
}
