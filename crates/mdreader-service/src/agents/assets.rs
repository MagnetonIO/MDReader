//! Small, versioned, owned skill asset. Never overwrites a user-authored skill.
use super::{Agent, Selection};
use mdreader_core::{ReaderError, Result, SourceRoot};
use std::{
    io::{Read, Write},
    path::Path,
};
const SKILL: &str = include_str!("../../../../skills/mdreader/SKILL.md");
const PREVIOUS_SKILL:&str=include_str!("previous-skill.md");
fn directory(selection: &Selection, create: bool) -> Result<Option<SourceRoot>> {
    let mut root = selection.checked()?;
    let pieces = match selection.agent {
        Agent::Claude => [".claude", "skills", "mdreader"],
        Agent::Codex => [".agents", "skills", "mdreader"],
    };
    for piece in pieces {
        if !create {
            match std::fs::symlink_metadata(root.path.join(piece)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    root.verify()?;
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
                Ok(_) => {}
            }
        }
        root = if create {
            root.create_or_capture_directory_relative(Path::new(piece))?
        } else {
            root.verify_directory_relative(Path::new(piece))?;
            SourceRoot::capture(root.path.join(piece))?
        };
    }
    Ok(Some(root))
}
fn contents(root: &SourceRoot) -> Result<Option<String>> {
    let Some(file) = root.open_optional_file_relative(Path::new("SKILL.md"))? else {
        return Ok(None);
    };
    let mut text = String::new();
    file.take(4097).read_to_string(&mut text)?;
    root.verify()?;
    Ok(Some(text))
}
pub(super) fn check_compatible(selection: &Selection,owned:bool) -> Result<()> {
    if let Some(root) = directory(selection, false)? {
        if contents(&root)?.is_some_and(|text| text != SKILL && !(owned&&text==PREVIOUS_SKILL)) {
            return Err(ReaderError::Conflict);
        }
    }
    Ok(())
}
pub(super) fn present(selection: &Selection,owned:bool) -> Result<bool> {
    Ok(match directory(selection, false)? {
        Some(root) => contents(&root)?.is_some_and(|text|text==SKILL||(owned&&text==PREVIOUS_SKILL)),
        None => false,
    })
}
// Existing bytes confer no ownership. Only a prior durable ownership record or
// successful exclusive creation permits this installer to remove the skill later.
pub(super) fn install(selection: &Selection, owned: bool) -> Result<bool> {
    let root = directory(selection, true)?.ok_or(ReaderError::Unavailable)?;
    match contents(&root)? {
        Some(text) if text == SKILL => Ok(owned),
        Some(text) if owned&&text==PREVIOUS_SKILL=>{
            let expected=mdreader_core::local_write::body_revision(&text);
            mdreader_core::local_write::PreparedWrite::stage(root,"SKILL.md",Some(&expected),SKILL)?.commit(||selection.checked()?.verify().map_err(Into::into))?;
            Ok(true)
        },
        Some(_) => Err(ReaderError::Conflict),
        None => {
            let mut file = root.create_new_relative(Path::new("SKILL.md"))?;
            file.write_all(SKILL.as_bytes())?;
            file.sync_all()?;
            root.try_clone_directory_handle()?.sync_all()?;
            root.verify()?;
            Ok(true)
        }
    }
}
pub(super) fn uninstall(selection: &Selection) -> Result<()> {
    if let Some(root) = directory(selection, false)? {
        if let Some(text) = contents(&root)? {
            if text != SKILL && text!=PREVIOUS_SKILL {
                return Err(ReaderError::Conflict);
            }
            root.remove_file_relative(Path::new("SKILL.md"))?;
            root.try_clone_directory_handle()?.sync_all()?;
        }
    }
    Ok(())
}
