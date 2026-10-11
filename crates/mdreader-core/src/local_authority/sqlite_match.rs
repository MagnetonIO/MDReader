//! Pure bounded exclusion predicate shared by fresh and legacy index views.
//! No filesystem access or source admission occurs while evaluating SQL.
use crate::store::Result;
use rusqlite::{functions::FunctionFlags, Connection};
use std::path::Path;
pub(crate) fn register(db: &Connection) -> Result<()> {
    db.create_scalar_function(
        "mdreader_private_prefix_v1",
        2,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_INNOCUOUS,
        |ctx| {
            let (Ok(path), Ok(prefix)) = (ctx.get_raw(0).as_str(), ctx.get_raw(1).as_str()) else {
                return Ok(true);
            };
            // Corrupt/unbounded legacy evidence and Windows short-name aliases
            // are refused before copying, normalizing or evaluating prefixes.
            if path.is_empty()
                || prefix.chars().any(char::is_control)
                || crate::source_root::private_path_ambiguous(Path::new(path))
                || crate::source_root::private_path_ambiguous(Path::new(prefix))
                || path.chars().any(char::is_control)
                || Path::new(path)
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
            {
                return Ok(true);
            }
            Ok(
                crate::source_root::exclusion_relative(Path::new(path), Path::new(prefix))
                    .is_some(),
            )
        },
    )?;
    Ok(())
}
