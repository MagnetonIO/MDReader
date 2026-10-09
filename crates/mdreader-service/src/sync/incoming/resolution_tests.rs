//! Real SQLite/files and production preservation primitives; not OIDC evidence.
use super::*;
use crate::sync::incoming::resolution::{self, Preserved, ResolutionChoice, ResolutionRequest};

fn request(view: &resolution::View) -> ResolutionRequest {
    ResolutionRequest {
        sequence: view.entry.sequence,
        revision_id: view.entry.revision_id.clone(),
        resolution_token: view.entry.resolution_token.clone(),
        choice: ResolutionChoice::KeepBoth,
    }
}
fn resolve(
    f: &mut Fixture,
    input: &PreparedIncoming,
    request: &ResolutionRequest,
) -> Result<Receipt> {
    let identity = identity::Identity::from_prepared(input)?;
    let origin = identity::origin(
        &f.scope,
        input,
        f.origin.session_epoch,
        f.origin.selection_epoch,
    )?;
    let observed = resolution::view(&mut f.owner, input, &origin)?;
    observed.require(request)?;
    let head = observed.head.ok_or(Error::Conflict)?;
    let assets = f
        .runtime
        .assets(&mut f.owner, &f.temp.path().join("assets"))
        .unwrap();
    let current =
        publication::prepare_current(&mut f.owner, &head.document, &assets, &origin)?.verify()?;
    let archive =
        publication::prepare_archive(&mut f.owner, &head.document, &assets, &f.runtime)?.read()?;
    f.owner.with_library_connection(|db| -> Result<()> {
        let tx = db.transaction()?;
        ledger::reserve(&tx, &identity::scope_key(&f.scope)?, &identity, &origin)?;
        tx.commit()?;
        Ok(())
    })?;
    let plan = publication::reserve_new(
        &mut f.owner,
        &f.temp.path().join("documents"),
        &assets,
        &origin,
        &identity::document(input)?,
    )?;
    f.owner.with_library_connection(|db| -> Result<()> {
        let tx = db.transaction()?;
        ledger::associate(
            &tx,
            &identity::scope_key(&f.scope)?,
            &identity,
            &origin,
            plan.document_id(),
        )?;
        tx.commit()?;
        Ok(())
    })?;
    let ready = plan
        .write(identity::document(input)?)?
        .validate(f.runtime.validation_permit().unwrap())?;
    resolution::commit_preserved(
        &mut f.owner,
        &f.scope,
        &identity,
        &origin,
        Preserved {
            head: &head,
            current: &current,
            archive: &archive,
            ready: &ready,
        },
        |_| Ok(()),
    )
}
fn original(f: &mut Fixture, id: &str) -> (Vec<u8>, Vec<u8>, bool, u32) {
    let assets = f
        .runtime
        .assets(&mut f.owner, &f.temp.path().join("assets"))
        .unwrap();
    let archived = publication::prepare_archive(&mut f.owner, id, &assets, &f.runtime)
        .unwrap()
        .read()
        .unwrap();
    assert_eq!(archived.assets().len(), 1);
    (
        archived.markdown().to_vec(),
        archived.assets()[0].bytes().unwrap().to_vec(),
        archived.state().document.pinned,
        archived.state().document.line,
    )
}
#[test]
fn keep_both_publishes_peer_copy_and_preserves_original_body_assets_and_state() {
    let mut f = Fixture::new();
    let first = f.prepared(2, 1);
    let old = receipt(apply(&mut f, &first).unwrap());
    device::pin(&mut f.owner, &old.document.id, true).unwrap();
    device::position(&mut f.owner, &old.document.id, &old.document.revision, 3).unwrap();
    let before = original(&mut f, &old.document.id);
    assert!(before.0.starts_with(&[0xef, 0xbb, 0xbf]));
    assert!(before.0.windows(2).any(|pair| pair == b"\r\n"));
    let next = replacement_from(&mut f, 2, None, true);
    let origin = identity::origin(
        &f.scope,
        &next,
        f.origin.session_epoch,
        f.origin.selection_epoch,
    )
    .unwrap();
    let observed = resolution::view(&mut f.owner, &next, &origin).unwrap();
    assert!(observed.entry.reason == PendingReason::HeadConflict);
    assert!(observed.entry.choices == [ResolutionChoice::KeepBoth]);
    let accepted = resolve(&mut f, &next, &request(&observed)).unwrap();
    assert_ne!(accepted.document.id, old.document.id);
    assert_eq!(f.cursor().applied, 2);
    assert_eq!(f.count("device_documents_v1"), 2);
    assert_eq!(original(&mut f, &old.document.id), before);
    assert_eq!(
        device::read(&mut f.owner, &accepted.document.id, None)
            .unwrap()
            .markdown,
        "# Peer version\n\n![Pixel](pixel.png)\n"
    );
    f.reopen();
    assert_eq!(original(&mut f, &old.document.id), before);
    assert_eq!(f.cursor().applied, 2);
    assert_eq!(f.count("device_documents_v1"), 2);
    assert!(resolve(&mut f, &next, &request(&observed)).is_err());
    assert_eq!(f.count("device_documents_v1"), 2);
    assert_eq!(f.cursor().applied, 2);
}
#[test]
fn changed_local_state_or_activation_invalidates_exact_resolution_token_without_writes() {
    let mut f = Fixture::new();
    let first = f.prepared(2, 1);
    let old = receipt(apply(&mut f, &first).unwrap());
    let next = replacement_from(&mut f, 2, None, true);
    let origin = identity::origin(
        &f.scope,
        &next,
        f.origin.session_epoch,
        f.origin.selection_epoch,
    )
    .unwrap();
    let observed = resolution::view(&mut f.owner, &next, &origin).unwrap();
    let stale = request(&observed);
    device::pin(&mut f.owner, &old.document.id, true).unwrap();
    assert!(matches!(
        resolve(&mut f, &next, &stale),
        Err(Error::Conflict)
    ));
    assert_eq!(f.count("native_incoming_items_v1"), 1);
    assert_eq!(f.count("device_documents_v1"), 1);
    assert_eq!(f.cursor().applied, 1);
    let current = resolution::view(&mut f.owner, &next, &origin).unwrap();
    let fresh = request(&current);
    let mut replacement = origin.clone();
    replacement.session_epoch = Uuid::new_v4();
    replacement.selection_epoch = Uuid::new_v4();
    assert!(resolution::view(&mut f.owner, &next, &replacement)
        .unwrap()
        .require(&fresh)
        .is_err());
    assert_eq!(f.count("native_incoming_items_v1"), 1);
    assert_eq!(f.cursor().applied, 1);
}
