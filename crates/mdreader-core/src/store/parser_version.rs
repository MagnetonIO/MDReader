//! Derived parser upgrades retain useful rows until their source can be read
//! again, then replace the old parse transactionally at the same raw revision.
use super::*;

pub(super) const MIGRATION: &str = include_str!("migrations/006_parser_version.sql");

pub(super) fn queue_stale(library: &Connection, index: &Connection) -> Result<()> {
    let sources: Vec<String> = index
        .prepare("SELECT DISTINCT source_id FROM documents WHERE parser_version<?1")?
        .query_map([crate::parse::PARSER_VERSION], |row| row.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    if sources.is_empty() {
        return Ok(());
    }
    let tx = library.unchecked_transaction()?;
    for source in sources {
        tx.execute(
            "INSERT INTO index_jobs(source_id,phase) SELECT id,'queued' FROM sources WHERE id=?1 AND enabled=1
             ON CONFLICT(source_id) DO UPDATE SET phase='queued',error=NULL,generation=index_jobs.generation+1
             WHERE index_jobs.phase NOT IN('queued','indexing','loading','embedding')",
            [source],
        )?;
    }
    tx.commit()?;
    Ok(())
}
