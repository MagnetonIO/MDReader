//! Import receipt publication on the destination's EXISTING owner transaction.
//! Domain converters own imported rows/grants; this module invents neither.
use crate::{
    lifecycle::CandidateReceipt,
    model::{digest, valid_hash},
    Error, Limits, Result, RootBinding, VerifiedSnapshot,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::path::Path;
use uuid::Uuid;
const SQL: &str = include_str!("candidate/migrations/001-import-receipt.sql");
#[path = "candidate/connection.rs"]
mod connection;
use connection::check as check_connection;
#[cfg(all(test, unix))]
#[path = "candidate/connection_tests.rs"]
mod connection_tests;
/// Prepared after expensive snapshot verification/materialization, outside the
/// destination owner lock. It is not cloneable and never grants a source.
pub struct CandidatePlan {
    receipt: CandidateReceipt,
    #[cfg(unix)]
    root: crate::platform::Root,
}
pub fn prepare(
    operation: Uuid,
    snapshot: &VerifiedSnapshot,
    candidate: &Path,
    limits: &Limits,
) -> Result<CandidatePlan> {
    snapshot.revalidate(limits)?;
    #[cfg(not(unix))]
    {
        let _ = (operation, candidate);
        Err(Error::Unsupported)
    }
    #[cfg(unix)]
    {
        let root = crate::platform::Root::private(candidate)?;
        let original = &snapshot.manifest().original;
        let original_path = original.locator.restore().map_err(|_| Error::Invalid)?;
        if root.path.starts_with(&original_path) || original_path.starts_with(&root.path) {
            return Err(Error::Invalid);
        }
        let receipt = CandidateReceipt {
            id: Uuid::new_v4(),
            operation,
            original: original.clone(),
            candidate: root.binding.clone(),
            snapshot: snapshot.identity().clone(),
            imported_records: 0,
            import_digest: crate::model::hash(&[]),
        };
        Ok(CandidatePlan { receipt, root })
    }
}
/// Call after all domain import rows in the SAME supplied destination TX. The
/// caller commits only after its own final authority checks. Receipt visibility
/// alone inside an uncommitted TX is deliberately insufficient for activation.
pub fn record(
    tx: &Transaction<'_>,
    plan: &CandidatePlan,
    imported_records: u64,
    import_digest: &str,
) -> Result<CandidateReceipt> {
    plan.verify_root()?;
    check_connection(tx, &plan.receipt.candidate)?;
    if imported_records > 1_000_000 || !valid_hash(import_digest) {
        return Err(Error::Bounds);
    }
    schema(tx, true)?;
    let mut receipt = plan.receipt.clone();
    receipt.imported_records = imported_records;
    receipt.import_digest = import_digest.to_owned();
    receipt.validate()?;
    let encoded = serde_json::to_string(&receipt).map_err(|_| Error::Invalid)?;
    if encoded.len() > 262144 {
        return Err(Error::Bounds);
    }
    let expected = digest(&receipt)?;
    tx.execute(
        "INSERT OR IGNORE INTO profile_import_receipts_v1 VALUES(?1,?2,?3)",
        params![receipt.operation.to_string(), encoded, expected],
    )
    .map_err(|_| Error::Storage)?;
    if read(tx, receipt.operation)?
        .as_ref()
        .map(digest)
        .transpose()?
        != Some(expected)
    {
        return Err(Error::Conflict);
    }
    plan.verify_root()?;
    check_connection(tx, &plan.receipt.candidate)?;
    Ok(receipt)
}
impl CandidatePlan {
    fn verify_root(&self) -> Result<()> {
        #[cfg(unix)]
        {
            self.root.verify()
        }
        #[cfg(not(unix))]
        {
            Err(Error::Unsupported)
        }
    }
}
/// Native FrozenActivation implementations use this on the actual candidate
/// ReaderStore connection. It never opens a database or accepts uncommitted SQL.
pub fn verify_committed(connection: &Connection, expected: &CandidateReceipt) -> Result<()> {
    expected.validate()?;
    if !connection.is_autocommit() {
        return Err(Error::Conflict);
    }
    schema(connection, false)?;
    check_root(&expected.candidate)?;
    check_connection(connection, &expected.candidate)?;
    if read(connection, expected.operation)?
        .as_ref()
        .map(digest)
        .transpose()?
        != Some(digest(expected)?)
    {
        return Err(Error::Conflict);
    }
    check_root(&expected.candidate)?;
    check_connection(connection, &expected.candidate)
}
fn check_root(binding: &RootBinding) -> Result<()> {
    #[cfg(unix)]
    {
        let root = crate::platform::Root::private(
            &binding.locator.restore().map_err(|_| Error::Invalid)?,
        )?;
        if &root.binding != binding {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = binding;
        Err(Error::Unsupported)
    }
}
fn read(connection: &Connection, id: Uuid) -> Result<Option<CandidateReceipt>> {
    let oversized: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM profile_import_receipts_v1 \
             WHERE operation=?1 AND length(CAST(receipt AS BLOB))>262144)",
            [id.to_string()],
            |r| r.get(0),
        )
        .map_err(|_| Error::Storage)?;
    if oversized {
        return Err(Error::Bounds);
    }
    let row = connection
        .query_row(
            "SELECT receipt,digest FROM profile_import_receipts_v1 WHERE operation=?1",
            [id.to_string()],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|_| Error::Storage)?;
    row.map(|(raw, hash)| {
        let value: CandidateReceipt = serde_json::from_str(&raw).map_err(|_| Error::Invalid)?;
        if value.operation != id || digest(&value)? != hash {
            return Err(Error::Integrity);
        }
        value.validate()?;
        Ok(value)
    })
    .transpose()
}
fn schema(connection: &Connection, create: bool) -> Result<()> {
    let existing: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='profile_import_schema_v1'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| Error::Storage)?;
    if existing.is_none() {
        let found: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema \
                 WHERE name='profile_import_receipts_v1')",
                [],
                |r| r.get(0),
            )
            .map_err(|_| Error::Storage)?;
        if found || !create {
            return Err(Error::Invalid);
        }
        connection.execute_batch(SQL).map_err(|_| Error::Storage)?;
    }
    let version: Option<u64> = connection
        .query_row(
            "SELECT version FROM profile_import_schema_v1 WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| Error::Invalid)?;
    if version != Some(1) {
        return Err(Error::Invalid);
    }
    for name in ["profile_import_schema_v1", "profile_import_receipts_v1"] {
        let actual: String = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
                [name],
                |r| r.get(0),
            )
            .map_err(|_| Error::Invalid)?;
        let expected = SQL
            .split(';')
            .find(|s| s.trim_start().starts_with(&format!("CREATE TABLE {name}(")))
            .ok_or(Error::Invalid)?
            .trim();
        if actual != expected {
            return Err(Error::Invalid);
        }
    }
    Ok(())
}
