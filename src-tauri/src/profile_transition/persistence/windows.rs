//! Windows ordinary state composition. This does not issue Unix-root freeze,
//! migration/default-selection or coordinated profile backup capabilities.
use super::{commands::PersistenceState, JsonOwner, PersistenceError};
use mdreader_core::SourceRoot;
use std::{path::Path, sync::Arc};
use tauri::Manager;

pub(crate) fn configure(app: &tauri::AppHandle, directory: &Path) -> Result<(), PersistenceError> {
    let root = SourceRoot::capture(directory.to_owned()).map_err(|_| PersistenceError::Conflict)?;
    let owner = Arc::new(JsonOwner::new(root)?);
    if !app.manage(PersistenceState {
        owner,
        transition: None,
        flush: None,
        editor: None,
    }) {
        return Err(PersistenceError::Conflict);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile_transition::persistence::JsonKind;

    #[test]
    fn windows_native_state_reopens_exact_bytes_rejects_stale_epoch_and_keeps_legacy_originals() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("private-profile");
        mdreader_capture::PrivateDirectory::create(&directory).unwrap();
        let root = SourceRoot::capture(directory.canonicalize().unwrap()).unwrap();
        let owner = JsonOwner::new(root.clone()).unwrap();
        assert!(matches!(
            JsonOwner::new(root.clone()),
            Err(PersistenceError::Unavailable)
        ));
        let identity = owner.load("main").unwrap().identity;
        let session =
            r#"{"readerHistory":{"version":1,"activeTabId":null,"tabs":[],"recent":[],"mru":[]}}"#;
        let receipt = owner
            .write("main", &identity, JsonKind::Session, 1, session)
            .unwrap();
        assert_eq!(
            owner
                .write("main", &identity, JsonKind::Session, 1, session)
                .unwrap()
                .sha256,
            receipt.sha256
        );
        assert!(matches!(
            owner.write("main", &identity, JsonKind::Session, 1, r#"{"root":null}"#),
            Err(PersistenceError::Conflict)
        ));
        owner
            .write(
                "main",
                &identity,
                JsonKind::Settings,
                1,
                r#"{"theme":"dark"}"#,
            )
            .unwrap();
        let connection =
            rusqlite::Connection::open(directory.join("reader-json-v1.sqlite")).unwrap();
        connection
            .pragma_update(None, "journal_mode", "PERSIST")
            .unwrap();
        let raw =
            br#"{"settings":{"theme":"dark","future":1.230000000000000000001e999},"other":true}"#;
        connection
            .execute(
                "UPDATE reader_json SET body=?1 WHERE name='settings.json'",
                [raw.as_slice()],
            )
            .unwrap();
        owner
            .write(
                "main",
                &identity,
                JsonKind::Settings,
                2,
                r#"{"theme":"light"}"#,
            )
            .unwrap();
        let exact = owner.read_current("main", &identity).unwrap();
        assert!(exact
            .settings_json
            .as_ref()
            .unwrap()
            .contains("1.230000000000000000001e999"));
        assert!(exact
            .settings_json
            .as_ref()
            .unwrap()
            .contains("\"other\":true"));
        assert!(!directory.join("session.json").exists());
        assert!(!directory.join("settings.json").exists());
        drop(connection);
        drop(owner);
        let owner = JsonOwner::new(root.clone()).unwrap();
        let loaded = owner.load("main").unwrap();
        assert_eq!(loaded.session_json, exact.session_json);
        assert_eq!(loaded.settings_json, exact.settings_json);
        assert!(matches!(
            owner.write(
                "main",
                &identity,
                JsonKind::Settings,
                3,
                r#"{"theme":"dark"}"#
            ),
            Err(PersistenceError::Conflict)
        ));
        drop(owner);
        let connection =
            rusqlite::Connection::open(directory.join("reader-json-v1.sqlite")).unwrap();
        connection
            .pragma_update(None, "journal_mode", "PERSIST")
            .unwrap();
        connection
            .execute("UPDATE reader_json_meta SET version=2", [])
            .unwrap();
        drop(connection);
        assert!(matches!(
            JsonOwner::new(root),
            Err(PersistenceError::UnsupportedSchema)
        ));

        let old = temporary.path().join("legacy-profile");
        mdreader_capture::PrivateDirectory::create(&old).unwrap();
        let original = br#"{"settings":{"theme":"dark"}}"#;
        std::fs::write(old.join("settings.json"), original).unwrap();
        let root = SourceRoot::capture(old.canonicalize().unwrap()).unwrap();
        assert!(matches!(
            JsonOwner::new(root),
            Err(PersistenceError::UnsupportedSchema)
        ));
        assert_eq!(std::fs::read(old.join("settings.json")).unwrap(), original);
    }
}
