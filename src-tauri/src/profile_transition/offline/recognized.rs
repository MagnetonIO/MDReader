use super::*;
use mdreader_core::SourceRoot;
/// Recognize only fixed legacy store locations. This is format detection, not
/// proof of owner closure or permission to read/import any document bodies.
pub(crate) fn recognized(root: &SourceRoot) -> Result<bool> {
    use mdreader_core::source_root::SourceEntryKind;
    root.verify().map_err(|_| Error::Conflict)?;
    if root
        .open_optional_file_relative(Path::new("cloud-library.catdb"))
        .map_err(|_| Error::Conflict)?
        .is_some()
    {
        return Ok(true);
    }
    let entries = root
        .read_dir_relative_bounded(Path::new(""), 4096)
        .map_err(|_| Error::Conflict)?;
    let Some(workspaces) = entries.iter().find(|entry| entry.name == "workspaces") else {
        return Ok(false);
    };
    if workspaces.kind != SourceEntryKind::Directory {
        return Err(Error::Conflict);
    }
    let children = root
        .read_dir_relative_bounded(Path::new("workspaces"), 1000)
        .map_err(|_| Error::Conflict)?;
    for child in children {
        if child.kind != SourceEntryKind::Directory {
            continue;
        }
        let parent = Path::new("workspaces").join(child.name);
        for name in ["catdb.db", "proposals.db"] {
            if root
                .open_optional_file_relative(&parent.join(name))
                .map_err(|_| Error::Conflict)?
                .is_some()
            {
                root.verify().map_err(|_| Error::Conflict)?;
                return Ok(true);
            }
        }
    }
    root.verify().map_err(|_| Error::Conflict)?;
    Ok(false)
}
