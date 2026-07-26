//! Immutable demo deployment identity.
//!
//! The fingerprint intentionally binds three independent inputs: the deployed
//! source bytes, the authoritative catalog revision, and the build commit. It
//! is not a substitute for verifying that revision against the opened catalog;
//! the demo server remains latched off until Task 3 supplies that authority.

/// Compute the immutable workspace fingerprint.
///
/// `files` are sorted by source-relative path before hashing, so filesystem
/// traversal order cannot change the identity.
pub fn workspace_fingerprint(
    build_commit: &str,
    catalog_revision: &str,
    files: &[(String, Vec<u8>)],
) -> String {
    let mut ordered = files.to_vec();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"catdb-demo-workspace/v1\0");
    hash_part(&mut hasher, build_commit.as_bytes());
    hash_part(&mut hasher, catalog_revision.as_bytes());
    for (path, bytes) in ordered {
        hash_part(&mut hasher, path.as_bytes());
        hash_part(&mut hasher, &bytes);
    }
    hasher.finalize().to_hex().to_string()
}

fn hash_part(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Vec<(String, Vec<u8>)> {
        vec![
            ("notes/b.md".into(), b"# B\n".to_vec()),
            ("notes/a.md".into(), b"# A\n".to_vec()),
        ]
    }

    #[test]
    fn fingerprint_is_order_independent_but_binds_every_pinned_input() {
        let baseline = workspace_fingerprint("a".repeat(40).as_str(), "catalog-1", &files());
        let mut reversed = files();
        reversed.reverse();
        assert_eq!(
            baseline,
            workspace_fingerprint("a".repeat(40).as_str(), "catalog-1", &reversed)
        );

        let mut changed_bytes = files();
        changed_bytes[0].1.push(b'!');
        assert_ne!(
            baseline,
            workspace_fingerprint("a".repeat(40).as_str(), "catalog-1", &changed_bytes)
        );
        assert_ne!(
            baseline,
            workspace_fingerprint("a".repeat(40).as_str(), "catalog-2", &files())
        );
        assert_ne!(
            baseline,
            workspace_fingerprint("b".repeat(40).as_str(), "catalog-1", &files())
        );
    }
}
