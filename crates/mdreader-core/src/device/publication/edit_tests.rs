use super::tests::{commit, Fixture};
use super::*;
use crate::device::{catalog, state};

fn staged(
    f: &mut Fixture,
    old: &crate::device::DeviceDocument,
    operation: uuid::Uuid,
) -> (
    EditSelection,
    VerifiedCurrent,
    ArchivedDocument,
    ReadyIncoming,
) {
    let body = b"# Edited\n![Image](figure.png)\n";
    let selection = EditSelection::new(catalog::receipt(old), operation, body).unwrap();
    let origin = selection.origin();
    let current = prepare_current(&mut f.store, &old.id, &f.assets, &origin)
        .unwrap()
        .verify()
        .unwrap();
    let archive = prepare_archive(&mut f.store, &old.id, &f.assets, &f.runtime)
        .unwrap()
        .read()
        .unwrap();
    let input = IncomingDocument::new(
        old.name.clone(),
        body.to_vec(),
        archive
            .assets()
            .iter()
            .map(|asset| {
                IncomingAsset::new(
                    asset.target.clone(),
                    asset.mime.clone().unwrap(),
                    asset.bytes().unwrap().to_vec(),
                )
                .unwrap()
            })
            .collect(),
    )
    .unwrap();
    let ready = reserve_new(&mut f.store, &f.documents, &f.assets, &origin, &input)
        .unwrap()
        .write(input)
        .unwrap()
        .validate(f.runtime.validation_permit().unwrap())
        .unwrap();
    (selection, current, archive, ready)
}

#[test]
fn save_preserves_original_assets_and_pin_and_replays_after_owner_reopen() {
    let mut f = Fixture::new();
    let ready = f.ready();
    let old = commit(&mut f, &ready).document;
    drop(ready);
    crate::device::pin(&mut f.store, &old.id, true).unwrap();
    let (selection, current, archive, ready) = staged(&mut f, &old, uuid::Uuid::new_v4());
    let saved = commit_edit(&mut f.store, &ready, &current, &archive, &selection).unwrap();
    assert!(saved.pinned);
    assert_eq!(saved.line, 1);
    assert_ne!(saved.id, old.id);
    assert_eq!(
        replay_edit(&mut f.store, &selection).unwrap().unwrap().id,
        saved.id
    );
    assert!(state::current(&mut f.store, &old.id).is_err());
    let preserved = prepare_archive(&mut f.store, &old.id, &f.assets, &f.runtime)
        .unwrap()
        .read()
        .unwrap();
    assert_eq!(preserved.markdown(), archive.markdown());
    assert_eq!(preserved.assets()[0].bytes(), archive.assets()[0].bytes());
    drop(preserved);
    let result = prepare_archive(&mut f.store, &saved.id, &f.assets, &f.runtime)
        .unwrap()
        .read()
        .unwrap();
    assert_eq!(result.assets()[0].bytes(), archive.assets()[0].bytes());
    drop((ready, current, archive, result));
    drop(f.store);
    let mut reopened = crate::ReaderStore::open(&f.temp.path().join("data")).unwrap();
    assert_eq!(
        replay_edit(&mut reopened, &selection).unwrap().unwrap().id,
        saved.id
    );
}

#[test]
fn mismatched_replay_and_later_revocation_never_republish() {
    let mut f = Fixture::new();
    let ready = f.ready();
    let old = commit(&mut f, &ready).document;
    drop(ready);
    let operation = uuid::Uuid::new_v4();
    let (selection, current, archive, ready) = staged(&mut f, &old, operation);
    let saved = commit_edit(&mut f.store, &ready, &current, &archive, &selection).unwrap();
    let changed = EditSelection::new(catalog::receipt(&old), operation, b"changed").unwrap();
    assert!(matches!(
        replay_edit(&mut f.store, &changed),
        Err(PublicationError::Conflict)
    ));
    f.store.set_source_enabled(&saved.source_id, false).unwrap();
    assert!(replay_edit(&mut f.store, &selection).is_err());
    assert_eq!(f.count("device_documents_v1"), 2);
}

#[test]
fn changed_original_asset_aborts_save_without_retiring_original() {
    let mut f = Fixture::new();
    let ready = f.ready();
    let old = commit(&mut f, &ready).document;
    drop(ready);
    let (selection, current, archive, ready) = staged(&mut f, &old, uuid::Uuid::new_v4());
    let asset = &archive.assets()[0];
    std::fs::write(
        f.temp
            .path()
            .join("assets")
            .join(format!("{}.bin", asset.asset_id)),
        b"changed",
    )
    .unwrap();
    assert!(commit_edit(&mut f.store, &ready, &current, &archive, &selection).is_err());
    assert!(state::current(&mut f.store, &old.id).is_ok());
    assert_eq!(f.count("device_documents_v1"), 1);
}
