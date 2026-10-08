//! Readonly reader scope from actual opt-in grants. Never opens a spool, creates
//! directories, imports history or turns a caller's source ID into a grant.
use super::*;

pub fn sources(store: &ReaderStore, directory: &Path) -> Result<Vec<mdreader_core::Source>> {
    let grants = policies(store)?;
    if grants.len() > 128 {
        return Err(invalid());
    }
    let mut sources = std::collections::BTreeMap::new();
    for grant in grants.into_iter().filter(|g| g.policy.enabled) {
        let valid = (|| -> Result<mdreader_core::Source> {
            grant.policy.validate().map_err(|_| invalid())?;
            let root = SourceRoot::capture(directory.canonicalize()?)?;
            let relative = Path::new("captures")
                .join("policies")
                .join(artifact_name(&grant.policy.project_id));
            let file = root
                .open_optional_file_relative(&relative)?
                .ok_or_else(invalid)?;
            let mut bytes = Vec::new();
            file.take(128 * 1024 + 1).read_to_end(&mut bytes)?;
            if bytes.len() > 128 * 1024 {
                return Err(invalid());
            }
            let observed: ProjectPolicy = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
            if serde_json::to_value(observed).map_err(|_| invalid())?
                != serde_json::to_value(&grant.policy).map_err(|_| invalid())?
            {
                return Err(invalid());
            }
            let mut paths = vec![
                grant.policy.project_root.clone(),
                grant.policy.repository_root.clone(),
            ];
            paths.extend(grant.policy.worktree_roots.clone());
            paths.sort();
            paths.dedup();
            if grant.roots.iter().map(|r| &r.path).collect::<Vec<_>>()
                != paths.iter().collect::<Vec<_>>()
            {
                return Err(invalid());
            }
            for source in &grant.roots {
                source.restore()?.verify()?;
            }
            let projection = grant
                .projection_root
                .as_ref()
                .ok_or_else(invalid)?
                .restore()?;
            if projection.path != root.path.join("captures").join("markdown") {
                return Err(invalid());
            }
            let source = store
                .find_local_source(&projection.path, &projection.identity)?
                .filter(|s| s.enabled && Some(&s.id) == grant.source_id.as_ref())
                .ok_or_else(invalid)?;
            projection.verify()?;
            root.verify()?;
            Ok(source)
        })();
        // Revocation/replaced project roots remove this authority; never regrant.
        if let Ok(source) = valid {
            sources.insert(source.id.clone(), source);
        }
    }
    Ok(sources.into_values().collect())
}
