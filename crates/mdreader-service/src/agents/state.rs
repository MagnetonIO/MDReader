//! Installed receipt reconciliation and visible source capabilities.
use super::*;
pub(super) fn state(
    store: &mut ReaderStore,
    directory: &Path,
    selection: &Selection,
) -> Result<State> {
    selection.checked()?;
    let id = selection.id()?;
    let record = grants::record(store, &id)?;
    let installed = match record {
        Some(mut record) if matches!(record.phase.as_str(), "installed" | "installing") => {
            let req = request(directory, &record)?;
            let installed = assets::present(selection, record.owns_skill()).unwrap_or(false)
                && installer(directory)?
                    .verify_installed(&req, &policy(selection)?)
                    .unwrap_or(false);
            if installed && record.phase == "installing" {
                record.phase = "installed".into();
                grants::save_install(store, &id, &record)?;
            }
            installed
        }
        _ => false,
    };
    let (enabled, source_ids, _) = grants::grant(store, &id)?;
    let permissions = permissions::get(store, &id)?;
    let sources: Vec<_> = store
        .list_sources()?
        .into_iter()
        .filter(|s| s.enabled)
        .collect();
    let editable_source_ids = sources
        .iter()
        .filter_map(|s| {
            documents::editable(store, &s.id)
                .ok()
                .filter(|v| *v)
                .map(|_| s.id.clone())
        })
        .collect();
    Ok(State {
        installed,
        enabled,
        source_ids,
        sources,
        reporting_project_ids: permissions.projects,
        edit_source_ids: permissions.edits,
        editable_source_ids,
        connected: installed && enabled && permissions.last_seen_ms.is_some(),
        last_seen_ms: permissions.last_seen_ms,
    })
}
