//! Export only through the retained original capability to a new private archive.
use super::*;
pub(super) fn execute(app: &AppHandle, active: &mut Pending) -> Result<MigrationReply> {
    let operation = active.id;

    if let Some(owner) = active.windows.legacy.as_mut() {
        let Some(parent) = selected_parent(app, owner.path())? else {
            return Ok(reply("cancelled", Some(operation)));
        };
        let directory = workspace::private_directory(&parent.path, "MDReader-archive-")?;
        let inventory = owner.inventory()?;
        let snapshot = mdreader_profile::stage(
            owner,
            inventory,
            &directory.join("original"),
            &Default::default(),
        )?;
        snapshot.revalidate(&Default::default())?;
        owner.revalidate()?;
        parent.verify().map_err(|_| Error::Conflict)?;
        let mut result = reply("exported", Some(operation));
        result.origin = Some("legacy_catdb");
        return Ok(result);
    }
    let owner = active.windows.original.as_mut().ok_or(Error::Conflict)?;
    let Some(parent) = selected_parent(app, owner.path())? else {
        return Ok(reply("cancelled", Some(operation)));
    };
    let name = format!("MDReader-archive-{}", Uuid::new_v4());
    let selected =
        mdreader_capture::SelectedDirectory::open(&parent.path).map_err(|_| Error::Conflict)?;
    let private = selected
        .create_private_child(Path::new(&name))
        .map_err(|_| Error::Storage)?;
    let limits = mdreader_profile::Limits::default();
    let choices = crate::profile_transition::backup_inventory::selections(owner.path(), &limits)?;
    let inventory = mdreader_profile::Inventory::capture(owner.path(), &choices, &limits)?;
    let snapshot = mdreader_profile::stage(
        &mut crate::profile_transition::BorrowedFreeze { frozen: owner },
        inventory,
        &parent.path.join(name).join("original"),
        &limits,
    )?;
    snapshot.revalidate(&limits)?;
    private.verify().map_err(|_| Error::Conflict)?;
    selected.verify().map_err(|_| Error::Conflict)?;
    parent.verify().map_err(|_| Error::Conflict)?;
    let mut result = reply("exported", Some(operation));
    result.origin = Some("reader");
    Ok(result)
}
