use super::*;

pub(super) fn filename(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 160
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
        || !matches!(
            Path::new(name).extension().and_then(|x| x.to_str()),
            Some("md" | "zip")
        )
    {
        return Err("unavailable".into());
    }
    Ok(())
}

fn file(root: &SourceRoot, name: &Path) -> Result<(u64, bool), String> {
    let input = root
        .open_optional_file_relative(name)
        .map_err(failure)?
        .ok_or("unavailable")?;
    let meta = input.metadata().map_err(failure)?;
    if !meta.is_file() || !private(&meta, false) || meta.len() > MAX_COPY {
        return Err("unavailable".into());
    }
    let expired = SystemTime::now()
        .duration_since(meta.modified().map_err(failure)?)
        .unwrap_or_default()
        > Duration::from_secs(3600);
    Ok((meta.len(), expired))
}

pub(super) fn expired(root: &SourceRoot) -> Result<(), String> {
    let mut retained = 0;
    let mut bytes = 0;
    for (observed, item) in fs::read_dir(&root.path)
        .map_err(failure)?
        .take(65)
        .enumerate()
    {
        if observed >= 64 {
            return Err("too_large".into());
        }
        let name = item.map_err(failure)?.file_name();
        let text = name.to_str().ok_or("unavailable")?;
        let relative = Path::new(&name);
        if uuid::Uuid::parse_str(text).is_ok() {
            let child = root
                .create_or_capture_directory_relative(relative)
                .map_err(failure)?;
            if !private(&fs::symlink_metadata(&child.path).map_err(failure)?, true) {
                return Err("unavailable".into());
            }
            let entries = fs::read_dir(&child.path)
                .map_err(failure)?
                .take(2)
                .collect::<Result<Vec<_>, _>>()
                .map_err(failure)?;
            if entries.len() > 1 {
                return Err("unavailable".into());
            }
            if let Some(entry) = entries.first() {
                let leaf = entry.file_name();
                filename(leaf.to_str().ok_or("unavailable")?)?;
                let (size, old) = file(&child, Path::new(&leaf))?;
                child.verify().map_err(failure)?;
                if old {
                    child
                        .remove_file_relative(Path::new(&leaf))
                        .map_err(failure)?;
                } else {
                    retained += 1;
                    bytes += size;
                    continue;
                }
            }
            child.verify().map_err(failure)?;
            root.remove_empty_directory_relative(relative)
                .map_err(failure)?;
        } else {
            // Preserve compatibility with copies retained by the previous app.
            filename(text)?;
            if text
                .get(..36)
                .and_then(|id| uuid::Uuid::parse_str(id).ok())
                .is_none()
                || text.as_bytes().get(36) != Some(&b'-')
            {
                return Err("unavailable".into());
            }
            let (size, old) = file(root, relative)?;
            if old {
                root.remove_file_relative(relative).map_err(failure)?;
            } else {
                retained += 1;
                bytes += size;
            }
        }
    }
    root.verify().map_err(failure)?;
    if retained >= 3 || bytes > 60 * 1024 * 1024 {
        return Err("busy".into());
    }
    Ok(())
}
