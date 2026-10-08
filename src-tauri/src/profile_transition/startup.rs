//! Bind the ordinary JSON writer before the first renderer invoke. This does
//! not inventory/export a profile or make the other writer participants ready.
use super::{
    DatabaseKind, DatabaseRegistration, NativeProfileTransition,
    persistence::{
        JsonOwner, PersistenceError,
        bridge::{FlushBridge, FlushChannel},
        commands::{PersistenceState, TauriFlushChannel},
    },
    service::ServiceParticipant,
};
use mdreader_core::{SourceRoot, SourceRootIdentity};
use mdreader_profile::{RootBinding, RootLocator};
use std::{path::Path, sync::Arc};
use tauri::Manager;

pub(crate) fn configure(app: &tauri::AppHandle, directory: &Path) -> Result<(), PersistenceError> {
    let providers = app
        .try_state::<Arc<crate::native_providers::NativeProviders>>()
        .map(|state| state.inner().clone());
    let state = compose(
        directory,
        Arc::new(TauriFlushChannel(app.clone())),
        providers,
    )?;
    if !app.manage(state) {
        return Err(PersistenceError::Conflict);
    }
    Ok(())
}

fn compose(
    directory: &Path,
    channel: Arc<dyn FlushChannel>,
    providers: Option<Arc<crate::native_providers::NativeProviders>>,
) -> Result<PersistenceState, PersistenceError> {
    let root = SourceRoot::capture(directory.to_owned()).map_err(|_| PersistenceError::Conflict)?;
    let SourceRootIdentity::Unix { device, inode } = &root.identity else {
        return Err(PersistenceError::Unavailable);
    };
    let binding = RootBinding {
        locator: RootLocator::capture(&root.path).map_err(|_| PersistenceError::Conflict)?,
        device: *device,
        inode: *inode,
    };
    let owner = Arc::new(JsonOwner::new(root)?);
    let mut transition = NativeProfileTransition::for_root(binding.clone())
        .map_err(|_| PersistenceError::Conflict)?;
    let flush = FlushBridge::new(owner.clone(), binding.clone(), channel)
        .map_err(|_| PersistenceError::Conflict)?;
    for participant in flush.participants() {
        transition
            .register(participant, Vec::new())
            .map_err(|_| PersistenceError::Conflict)?;
    }
    let service = ServiceParticipant::new(binding.clone(), &directory.join("reader-v1"))
        .map_err(|_| PersistenceError::Conflict)?;
    for participant in service.participants() {
        let databases = if participant.participant() == mdreader_profile::Participant::ReaderDaemon
        {
            vec![
                DatabaseRegistration {
                    relative: "reader-v1/library.sqlite".into(),
                    kind: DatabaseKind::Durable,
                },
                DatabaseRegistration {
                    relative: "reader-v1/index.sqlite".into(),
                    kind: DatabaseKind::Rebuildable,
                },
            ]
        } else {
            Vec::new()
        };
        transition
            .register(participant, databases)
            .map_err(|_| PersistenceError::Conflict)?;
    }
    if let Some(providers) = providers {
        transition
            .register(
                Arc::new(
                    crate::native_providers::preservation::ParticipantOwner::new(
                        binding.clone(),
                        providers,
                    ),
                ),
                Vec::new(),
            )
            .map_err(|_| PersistenceError::Conflict)?;
    }
    Ok(PersistenceState {
        owner,
        transition: Arc::new(transition),
        flush,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdreader_profile::{Error, Participant};

    struct NoFreeze;
    impl FlushChannel for NoFreeze {
        fn request(
            &self,
            _: &str,
            _: &super::super::persistence::bridge::FlushRequest,
        ) -> mdreader_profile::Result<()> {
            panic!("missing real participants must prevent any renderer freeze")
        }
        fn released(
            &self,
            _: &str,
            _: &super::super::persistence::bridge::FlushRequest,
        ) -> mdreader_profile::Result<()> {
            panic!("no installed preservation was started")
        }
    }

    #[test]
    fn startup_binds_one_json_owner_without_inventorying_or_granting_profile_preservation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        // Unclassified content must neither prevent ordinary reading nor be
        // traversed by a startup-time profile inventory.
        std::fs::write(root.join("unclassified.bin"), b"retained original").unwrap();
        let original = br#"{"settings":{"theme":"dark","unknown":1e999999}}"#;
        std::fs::write(root.join("settings.json"), original).unwrap();
        let state = compose(&root, Arc::new(NoFreeze), None).unwrap();
        assert_eq!(state.transition.missing_participants().len(), 3);
        assert!(matches!(
            compose(&root, Arc::new(NoFreeze), None),
            Err(PersistenceError::Unavailable)
        ));
        let loaded = state.owner.load("main").unwrap();
        assert_eq!(
            loaded.settings_json.as_deref(),
            std::str::from_utf8(original).ok()
        );
        let _lease = state
            .transition
            .effect(Participant::RendererSettings)
            .unwrap();
        state
            .owner
            .write(
                "main",
                &loaded.identity,
                super::super::persistence::JsonKind::Settings,
                1,
                r#"{"theme":"light"}"#,
            )
            .unwrap();
        assert!(
            std::fs::read_to_string(root.join("settings.json"))
                .unwrap()
                .contains("1e999999")
        );
        assert_eq!(
            std::fs::read(root.join("unclassified.bin")).unwrap(),
            b"retained original"
        );
        assert!(matches!(
            state.transition.preserve_bundle(
                &[],
                &root.join("must-not-publish"),
                &mdreader_profile::Limits::default(),
                std::time::Duration::from_millis(10)
            ),
            Err(Error::NotQuiescent)
        ));
        assert!(!root.join("must-not-publish").exists());
    }
}
