//! Stable private profile namespace for native OS custody. The UUID lives in
//! the existing library owner, surviving mobile container relocation. It is
//! neither a source grant nor a remote account/library identity.
use super::*;
const DDL: &str = "CREATE TABLE reader_profile_identity_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1),profile_uuid TEXT NOT NULL)";
impl ReaderStore {
    pub fn profile_uuid(&mut self) -> Result<uuid::Uuid> {
        let identity = self.library_identity()?;
        identity.verify()?;
        self.with_library_connection(|db| {
            let tx = db.transaction()?;
            let present: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='reader_profile_identity_v1')", [], |r| r.get(0))?;
            if !present {
                tx.execute_batch(DDL)?;
                tx.execute("INSERT INTO reader_profile_identity_v1 VALUES(1,1,?1)", [uuid::Uuid::new_v4().to_string()])?;
            }
            let actual: String = tx.query_row("SELECT sql FROM sqlite_schema WHERE name='reader_profile_identity_v1' AND type='table'", [], |r| r.get(0)).map_err(|_| ReaderError::Unavailable)?;
            let normalize = |s: &str| s.chars().filter(|c| !c.is_ascii_whitespace() && *c != ';').collect::<String>();
            if normalize(&actual) != normalize(DDL) { return Err(ReaderError::Unavailable); }
            let count: u64 = tx.query_row("SELECT count(*) FROM reader_profile_identity_v1", [], |r| r.get(0))?;
            let (version, raw): (u32, String) = tx.query_row("SELECT version,profile_uuid FROM reader_profile_identity_v1 WHERE singleton=1", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
            let id = uuid::Uuid::parse_str(&raw).map_err(|_| ReaderError::Unavailable)?;
            if count != 1 || version != 1 || id.get_version_num() != 4 || id.to_string() != raw { return Err(ReaderError::Unavailable); }
            identity.verify()?;
            tx.commit()?;
            Ok(id)
        })
    }
}
