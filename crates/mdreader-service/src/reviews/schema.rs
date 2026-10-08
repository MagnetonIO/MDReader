use super::*;
use rusqlite::Connection;

pub(super) fn initialize(connection: &mut Connection) -> Result<Uuid> {
    let tx = connection.transaction().map_err(|_| ReviewError::Storage)?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='mdreader_review_owner')",
            [],
            |r| r.get(0),
        )
        .map_err(|_| ReviewError::Storage)?;
    if !exists {
        tx.execute_batch(include_str!("migrations/001-owner.sql"))
            .map_err(|_| ReviewError::Storage)?;
        tx.execute(
            "INSERT INTO mdreader_review_owner VALUES(1,1,?1)",
            [Uuid::new_v4().to_string()],
        )
        .map_err(|_| ReviewError::Storage)?;
    }
    let scope = read(&tx)?;
    tx.commit().map_err(|_| ReviewError::Storage)?;
    Ok(scope)
}
pub(super) fn read(connection: &Connection) -> Result<Uuid> {
    let (version, scope): (u32, String) = connection
        .query_row(
            "SELECT version,owner_uuid FROM mdreader_review_owner WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| ReviewError::Storage)?;
    if version != 1 {
        return Err(ReviewError::Unsupported);
    }
    Uuid::parse_str(&scope)
        .ok()
        .filter(|id| id.get_version_num() == 4 && id.get_variant() == uuid::Variant::RFC4122)
        .ok_or(ReviewError::Storage)
}
