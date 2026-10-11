//! Source-root scanner: an [`ignore`]-crate walk honoring `.gitignore` and
//! `.contextignore`, hidden-file filtering, size limits, and markdown
//! extension selection, with per-file git-repo identity detection.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::Match;

use crate::source_root::{SourceEntryKind, SourceRoot};

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
#[derive(Debug, Clone)]
pub struct ScannedFile {
    /// Absolute path on disk.
    pub abs: PathBuf,
    /// Path relative to the scanned root, `/`-separated.
    pub rel: String,
    /// Repository identity: the name of the nearest enclosing git root under
    /// the scan root, or the root's own name as a synthetic repo.
    pub repo: String,
    authority: Option<Arc<File>>,
}

impl ScannedFile {
    pub(crate) fn stamp(&self) -> std::io::Result<crate::source_root::file_stamp::SourceFileStamp> {
        let file = self
            .authority
            .as_ref()
            .ok_or_else(|| std::io::Error::other("Missing retained file"))?;
        crate::source_root::file_stamp::single_link_stamp(file)
    }
    /// Read the exact opened file selected by the scan.
    pub fn read(&self) -> std::io::Result<Vec<u8>> {
        match &self.authority {
            Some(file) => {
                let mut file = file.try_clone()?;
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)?;
                Ok(bytes)
            }
            None => std::fs::read(&self.abs),
        }
    }

    /// Bound allocation even when a discovered file grows before it is read.
    pub fn read_bounded(&self, max_bytes: u64) -> std::io::Result<Vec<u8>> {
        let file = match &self.authority {
            Some(file) => file.try_clone()?,
            None => File::open(&self.abs)?,
        };
        let mut bytes = Vec::new();
        file.take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "document exceeds size limit",
            ));
        }
        Ok(bytes)
    }

    /// Read metadata from the same opened file authority selected by the scan.
    pub fn metadata(&self) -> std::io::Result<std::fs::Metadata> {
        match &self.authority {
            Some(file) => file.metadata(),
            None => std::fs::metadata(&self.abs),
        }
    }
}

pub fn is_markdown(path: &Path, include_mdx: bool) -> bool {
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
            authority: None,
        });
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

/// Scan through a retained source-root authority.
///
/// Directory enumeration and every selected file handle are descriptor-relative,
/// so replacing the admitted root path or a descendant with a symlink cannot
/// redirect the import to a different filesystem object.
pub fn scan_source_root(
    root: &SourceRoot,
    options: &ScanOptions,
) -> std::io::Result<Vec<ScannedFile>> {
    let mut out = Vec::new();
    visit_source_root(root, options, |file| {
        out.push(file);
        Ok::<(), std::io::Error>(())
    })?;
    out.sort_by(|left, right| left.rel.cmp(&right.rel));
    Ok(out)
}

/// Process selected files while their descriptor-relative authority is alive.
/// The visitor must consume each file before returning to keep descriptor use
/// bounded by traversal depth, rather than total document count.
pub fn visit_source_root<E: From<std::io::Error>>(
    root: &SourceRoot,
    options: &ScanOptions,
    mut visit: impl FnMut(ScannedFile) -> Result<(), E>,
) -> Result<usize, E> {
    let synthetic = root
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "root".into());
    let mut count = 0;
    scan_bound_dir(
        root,
        Path::new(""),
        options,
        &synthetic,
        &[],
        &mut |file| -> Result<(), E> {
            visit(file)?;
            count += 1;
            Ok(())
        },
    )?;
    Ok(count)
}

fn source_io(error: crate::SourceRootError) -> std::io::Error {
    let kind = if matches!(error, crate::SourceRootError::PermissionDenied { .. }) {
        std::io::ErrorKind::PermissionDenied
    } else {
        std::io::ErrorKind::Other
    };
    std::io::Error::new(kind, error.to_string())
}

fn load_ignore(root: &SourceRoot, directory: &Path, name: &str) -> std::io::Result<Gitignore> {
    let relative = directory.join(name);
    let read = root.read_relative(&relative).map_err(source_io)?;
    let text = String::from_utf8_lossy(&read.bytes);
    let mut builder = GitignoreBuilder::new(directory);
    for line in text.lines() {
        builder
            .add_line(Some(root.path.join(&relative)), line)
            .map_err(std::io::Error::other)?;
    }
    builder.build().map_err(std::io::Error::other)
}

fn declared_ignore(
    root: &SourceRoot,
    directory: &Path,
    entries: &[crate::source_root::SourceEntry],
    name: &str,
) -> std::io::Result<Option<Gitignore>> {
    let Some(entry) = entries.iter().find(|entry| entry.name == name) else {
        return Ok(None);
    };
    if entry.kind != SourceEntryKind::File {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "ignore policy is not a regular file: {}",
                root.path.join(directory).join(name).display()
            ),
        ));
    }
    load_ignore(root, directory, name).map(Some)
}

fn ignored_by(matchers: &[Gitignore], relative: &Path, is_dir: bool) -> bool {
    for matcher in matchers.iter().rev() {
        match matcher.matched(relative, is_dir) {
            Match::Ignore(_) => return true,
            Match::Whitelist(_) => return false,
            Match::None => {}
        }
    }
    false
}

/// Reconcile explicit native event paths under the original root and inherited
/// ignore rules. Missing targets are observed by successful parent enumeration.
/// A directory move/ignore-file change can request its affected subtree.
pub fn visit_source_targets<E: From<std::io::Error>>(
    root: &SourceRoot,
    options: &ScanOptions,
    targets: &[std::path::PathBuf],
    mut visit: impl FnMut(ScannedFile) -> Result<(), E>,
) -> Result<(), E> {
    if targets.len() > 256
        || targets.iter().any(|p| {
            p.is_absolute()
                || p.components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
        })
    {
        return Err(std::io::Error::other("Invalid incremental paths").into());
    }
    root.verify().map_err(source_io)?;
    scan_bound_dir_targets(
        root,
        Path::new(""),
        options,
        "",
        &[],
        Some(targets),
        &mut visit,
    )?;
    root.verify().map_err(source_io)?;
    Ok(())
}
fn scan_bound_dir<E: From<std::io::Error>>(
    root: &SourceRoot,
    relative: &Path,
    options: &ScanOptions,
    inherited_repo: &str,
    inherited_matchers: &[Gitignore],
    visit: &mut impl FnMut(ScannedFile) -> Result<(), E>,
) -> Result<(), E> {
    scan_bound_dir_targets(
        root,
        relative,
        options,
        inherited_repo,
        inherited_matchers,
        None,
        visit,
    )
}
fn scan_bound_dir_targets<E: From<std::io::Error>>(
    root: &SourceRoot,
    relative: &Path,
    options: &ScanOptions,
    inherited_repo: &str,
    inherited_matchers: &[Gitignore],
    targets: Option<&[std::path::PathBuf]>,
    visit: &mut impl FnMut(ScannedFile) -> Result<(), E>,
) -> Result<(), E> {
    if relative.components().count() > 256 {
        return Err(std::io::Error::other("Folder depth budget exceeded").into());
    }
    let entries = root
        .read_scan_directory(relative, 100_000)
        .map_err(source_io)?;
    let mut matchers: Vec<Gitignore> = inherited_matchers.to_vec();
    if let Some(context) = declared_ignore(root, relative, &entries, ".contextignore")? {
        matchers.push(context);
    }
    if options.respect_gitignore {
        if let Some(ignore) = declared_ignore(root, relative, &entries, ".ignore")? {
            matchers.push(ignore);
        }
        if let Some(gitignore) = declared_ignore(root, relative, &entries, ".gitignore")? {
            matchers.push(gitignore);
        }
        if entries
            .iter()
            .any(|entry| entry.name == ".git" && entry.kind == SourceEntryKind::Directory)
        {
            let git = relative.join(".git");
            let git_entries = root.read_dir_relative(&git).map_err(source_io)?;
            if git_entries
                .iter()
                .any(|entry| entry.name == "info" && entry.kind == SourceEntryKind::Directory)
            {
                let info = git.join("info");
                let info_entries = root.read_dir_relative(&info).map_err(source_io)?;
                if let Some(exclude) = declared_ignore(root, &info, &info_entries, "exclude")? {
                    matchers.push(exclude);
                }
            }
        }
    }

    let has_git = entries.iter().any(|entry| entry.name == ".git");
    let repo = if has_git {
        relative
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| inherited_repo.to_string())
    } else {
        inherited_repo.to_string()
    };

    for entry in entries {
        let name = entry.name;
        if name.to_string_lossy().starts_with('.')
            || matches!(
                name.to_str(),
                Some(
                    "node_modules"
                        | "target"
                        | "dist"
                        | "build"
                        | "vendor"
                        | "venv"
                        | "__pycache__"
                )
            )
        {
            continue;
        }
        let child = relative.join(&name);
        if !root.allows_relative(&child)
            || targets.is_some_and(|targets| {
                !targets
                    .iter()
                    .any(|target| child.starts_with(target) || target.starts_with(&child))
            })
        {
            continue;
        }
        let is_dir = entry.kind == SourceEntryKind::Directory;
        if ignored_by(&matchers, &child, is_dir) {
            continue;
        }
        match entry.kind {
            SourceEntryKind::Directory => {
                if !root.same_filesystem(&child).map_err(source_io)? {
                    continue;
                }
                scan_bound_dir_targets(root, &child, options, &repo, &matchers, targets, visit)?;
            }
            SourceEntryKind::File
                if entry.size <= options.max_file_size
                    && is_markdown(&child, options.include_mdx) =>
            {
                let authority = root.open_file_relative(&child).map_err(source_io)?;
                let rel = child
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                visit(ScannedFile {
                    abs: root.path.join(&child),
                    rel,
                    repo: repo.clone(),
                    authority: Some(Arc::new(authority)),
                })?;
            }
            SourceEntryKind::File | SourceEntryKind::SymlinkOrReparse | SourceEntryKind::Other => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::SourceRoot;

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

    #[cfg(unix)]
    #[test]
    fn bound_scan_reads_the_retained_root_and_never_indexes_a_same_path_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        write(&root, "original.md", "# original");
        let canonical = std::fs::canonicalize(&root).unwrap();
        let admitted = SourceRoot::capture(canonical.clone()).unwrap();

        let moved = temp.path().join("moved-original");
        std::fs::rename(&canonical, &moved).unwrap();
        std::fs::create_dir(&canonical).unwrap();
        write(&canonical, "replacement.md", "# replacement");

        let scanned = scan_source_root(&admitted, &opts()).expect("scan retained authority");
        let rels: Vec<&str> = scanned.iter().map(|file| file.rel.as_str()).collect();
        assert_eq!(rels, vec!["original.md"]);
        assert_eq!(
            scanned[0].read().expect("read retained file"),
            b"# original"
        );
    }

    #[cfg(unix)]
    #[test]
    fn bound_scan_does_not_treat_an_unreadable_ignore_file_as_absent() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), "keep.md", "# keep");
        write(temp.path(), ".contextignore", "drafts/\n");
        let canonical = std::fs::canonicalize(temp.path()).unwrap();
        let admitted = SourceRoot::capture(canonical).unwrap();
        let ignore = temp.path().join(".contextignore");
        let mut permissions = std::fs::metadata(&ignore).unwrap().permissions();
        permissions.set_mode(0o000);
        std::fs::set_permissions(&ignore, permissions).unwrap();

        let result = scan_source_root(&admitted, &opts());

        let mut permissions = std::fs::metadata(&ignore).unwrap().permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(&ignore, permissions).unwrap();
        assert!(
            result.is_err(),
            "an ignore policy that cannot be read must fail closed rather than scan as if it were absent"
        );
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
