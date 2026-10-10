//! Native-verified metadata capability upgrade; binding and grant stay exact.
use super::*;
use rusqlite::params;

pub(super) fn retain_verified_changes(c: &mut Connection, opened: &SourceDescriptor) -> Result<()> {
    if !opened.capabilities().changes {
        return Ok(());
    }
    let binding = opened.binding();
    revalidate(c, binding)?;
    let current = registry::descriptor_at(c, binding, true)?;
    if current.capabilities().changes {
        return Ok(());
    }
    if current.kind() != ProviderKind::GoogleDrive
        || opened.kind() != current.kind()
        || opened.scope() != current.scope()
        || !matches!(
            current.scope(),
            SelectedScope::DriveFolder { .. } | SelectedScope::ScopedRoot { .. }
        )
    {
        return Err(ProviderError::IdentityMismatch);
    }
    let mut expected = current.capabilities();
    expected.changes = true;
    if opened.capabilities() != expected {
        return Err(ProviderError::IdentityMismatch);
    }
    let next = SourceDescriptor::new(
        binding.clone(),
        current.kind(),
        current.name().clone(),
        current.scope().clone(),
        expected,
        current.health(),
        current.coverage().clone(),
    )?;
    let before = registry::encode(&current, 64 * 1024)?;
    let text = registry::encode(&next, 64 * 1024)?;
    let tx = registry::db(c.transaction())?;
    if registry::db(tx.execute(
        "UPDATE mdreader_provider_registry_sources SET descriptor=?1,digest=?2 WHERE identity=?3 AND binding=?4 AND enabled=1 AND descriptor=?5 AND digest=?6",
        params![text, body_hash(text.as_bytes()).as_slice(), registry::identity(binding), binding.digest().as_slice(), before, body_hash(before.as_bytes()).as_slice()],
    ))? != 1 { return Err(ProviderError::Conflict); }
    // Pre-delta staging has no start checkpoint, so it cannot certify changes
    // that raced its old enumeration. Retain published metadata and bodies.
    for table in ["rows", "seen"] {
        registry::db(tx.execute(&format!("DELETE FROM mdreader_provider_registry_{table} WHERE snapshot IN (SELECT id FROM mdreader_provider_registry_snapshots WHERE source=?1 AND binding=?2 AND phase!='published' AND change_cursor IS NULL)"), params![registry::identity(binding), binding.digest().as_slice()]))?;
    }
    registry::db(tx.execute("DELETE FROM mdreader_provider_registry_snapshots WHERE source=?1 AND binding=?2 AND phase!='published' AND change_cursor IS NULL", params![registry::identity(binding), binding.digest().as_slice()]))?;
    registry::db(tx.commit())
}
