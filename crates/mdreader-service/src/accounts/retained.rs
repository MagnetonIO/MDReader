//! Only opaque references/public hints live here. Prior private custody stays in the vault.
use super::*;
use rusqlite::OptionalExtension;
use storage::Binding;
fn exact(actual: Option<Binding>, expected: &Binding) -> Result<()> {
    match actual {
        Some(b) if b.library == expected.library && b.reference == expected.reference => Ok(()),
        _ => Err(AccountError::Conflict),
    }
}
fn hint_in(tx: &rusqlite::Transaction<'_>, expected: &Binding) -> Result<Option<Uuid>> {
    exact(storage::read(tx)?, expected)?;
    let row: Option<(String,String)> = tx.query_row(
        "SELECT credential_reference,remote_library FROM native_sync_existing_library WHERE singleton=1",
        [], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    row.map(|(reference, value)| {
        let id = Uuid::parse_str(&value).map_err(|_| AccountError::Schema)?;
        if reference != expected.reference.opaque_reference()
            || id.get_version_num() != 4
            || id.to_string() != value
        {
            return Err(AccountError::Schema);
        }
        Ok(id)
    })
    .transpose()
}
pub(super) fn hint(owner: &mut ReaderStore, expected: &Binding) -> Result<Option<Uuid>> {
    owner.with_library_connection(|db| {
        let tx = db.transaction()?;
        storage::schema(&tx)?;
        let result = hint_in(&tx, expected)?;
        tx.commit()?;
        Ok(result)
    })
}
pub(super) fn observe(owner: &mut ReaderStore, expected: &Binding, library: Uuid) -> Result<()> {
    owner.with_library_connection(|db| {
        let tx = db.transaction()?;
        storage::schema(&tx)?;
        match hint_in(&tx, expected)? {
            Some(old) if old != library => return Err(AccountError::Conflict),
            Some(_) => {}
            None => {
                tx.execute(
                    "INSERT INTO native_sync_existing_library VALUES(1,?1,?2)",
                    rusqlite::params![expected.reference.opaque_reference(), library.to_string()],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    })
}
pub(super) fn switch(
    owner: &mut ReaderStore,
    expected: &Binding,
    library: Uuid,
    reference: &SyncCredentialReference,
) -> Result<Binding> {
    if library == expected.library
        || library.get_version_num() != 4
        || *reference == expected.reference
    {
        return Err(AccountError::Conflict);
    }
    owner.with_library_connection(|db| {
        let tx=db.transaction()?;
        storage::schema(&tx)?;
        if hint_in(&tx, expected)? != Some(library) {return Err(AccountError::Conflict);}
        let count:u64=tx.query_row("SELECT count(*) FROM native_sync_retained_bindings",[],|r|r.get(0))?;
        if count>=8 {return Err(AccountError::Limit);}
        tx.execute("INSERT INTO native_sync_retained_bindings VALUES(?1,?2,?3,?4)",
            rusqlite::params![count+1,expected.library.to_string(),expected.reference.opaque_reference(),library.to_string()])?;
        if tx.execute("UPDATE native_sync_account_binding SET remote_library=?1,credential_reference=?2 WHERE singleton=1 AND remote_library=?3 AND credential_reference=?4",
            rusqlite::params![library.to_string(),reference.opaque_reference(),expected.library.to_string(),expected.reference.opaque_reference()])? != 1 {
            return Err(AccountError::Conflict);
        }
        tx.execute("DELETE FROM native_sync_existing_library WHERE singleton=1",[])?;
        tx.commit()?;
        Ok(Binding{library,reference:reference.clone()})
    })
}
