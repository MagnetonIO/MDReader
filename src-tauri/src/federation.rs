//! Native product boundary for the federated `Document` concept.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::process::Command;

    use super::query_federated_document_guarded;
    use crate::commands::WorkspaceState;

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn fingerprint(root: &Path) -> BTreeMap<String, (u64, blake3::Hash, std::time::SystemTime)> {
        walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| {
                let path = entry.path();
                let relative = path
                    .strip_prefix(root)
                    .expect("under root")
                    .to_string_lossy()
                    .replace('\\', "/");
                let bytes = std::fs::read(path).expect("read source bytes");
                let metadata = std::fs::metadata(path).expect("read source metadata");
                (
                    relative,
                    (
                        metadata.len(),
                        blake3::hash(&bytes),
                        metadata.modified().expect("source mtime"),
                    ),
                )
            })
            .collect()
    }

    fn committed_workspace() -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("temporary workspace");
        std::fs::write(
            directory.path().join("a file.md"),
            "---\ntitle: Declared title\nauthor: Ada\n---\n# Different heading\n\nBody.\n",
        )
        .expect("write markdown");
        git(directory.path(), &["init", "-q"]);
        git(directory.path(), &["config", "user.name", "Committer"]);
        git(
            directory.path(),
            &["config", "user.email", "committer@example.invalid"],
        );
        git(directory.path(), &["add", "a file.md"]);
        git(directory.path(), &["commit", "-qm", "seed"]);
        directory
    }

    #[tokio::test]
    async fn one_native_query_returns_three_source_reports_and_per_field_attribution() {
        let directory = committed_workspace();
        let state = WorkspaceState::granted(directory.path().to_path_buf());
        let document = directory.path().join("a file.md");
        let before = fingerprint(directory.path());

        let answer = query_federated_document_guarded(
            &state,
            document.to_str().expect("utf-8 test path"),
            || {},
        )
        .await
        .expect("federated document query");

        assert_eq!(answer.completeness, "complete");
        assert_eq!(answer.sources.len(), 3);
        assert!(answer.sources.iter().all(|source| source.outcome == "ran"));
        assert_eq!(answer.document.path, "a file.md");
        assert_eq!(answer.document.fields.len(), 6);
        let title = answer
            .document
            .fields
            .iter()
            .find(|field| field.name == "title")
            .expect("title field");
        assert_eq!(title.winner.as_deref(), Some("frontmatter"));
        assert_eq!(title.value.as_deref(), Some("\"Declared title\""));
        let modified = answer
            .document
            .fields
            .iter()
            .find(|field| field.name == "modifiedAt")
            .expect("modifiedAt field");
        assert_eq!(modified.winner.as_deref(), Some("filesystem"));
        assert!(
            modified
                .providers
                .iter()
                .any(|provider| provider.source == "git" && provider.verdict == "observed"),
            "the losing git observation stays visible"
        );
        assert_eq!(
            fingerprint(directory.path()),
            before,
            "the product query may not change bytes, mtimes, git metadata, or membership"
        );
    }

    #[tokio::test]
    async fn native_query_refuses_a_document_outside_the_active_library() {
        let directory = committed_workspace();
        let outsider = tempfile::tempdir().expect("outside directory");
        let outside_path = outsider.path().join("outside.md");
        std::fs::write(&outside_path, "# Outside").expect("write outsider");
        let state = WorkspaceState::granted(directory.path().to_path_buf());

        let error = query_federated_document_guarded(
            &state,
            outside_path.to_str().expect("utf-8 test path"),
            || {},
        )
        .await
        .expect_err("outside path must not cross the workspace grant");

        assert!(error.contains("escapes the workspace"), "{error}");
    }
}
