use super::*;
use crate::library::LibraryRegistry;

fn record(id: &str) -> String {
    format!(
        r#"{{"kind":"connected-catdb","id":"{id}","unknownNumber":1.25000001e+999,"unknownInteger":900719925474099312345,"credentials":"private-sentinel","future":{{"contract":[true,null]}}}}"#
    )
}

fn registry(version: u32, rows: &str, active: &str) -> String {
    format!(
        r#"{{"version":{version},"active_source_id":{active},"sources":[{rows}],"cleanup_pending":[]}}"#
    )
}

#[test]
fn old_registries_archive_exact_raw_rows_and_never_activate_them() {
    let id = "a".repeat(64);
    let row = record(&id);
    for version in [7, 8, 9] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.json");
        let original = registry(version, &row, &format!("\"{id}\""));
        fs::write(&path, &original).unwrap();
        let loaded = LibraryRegistry::load_from(&path).unwrap();
        assert!(loaded.sources.is_empty());
        assert_eq!(loaded.restore_source_id(), None);
        assert_eq!(loaded.retired_sources.len(), 1);
        assert!(!format!("{loaded:?}").contains("private-sentinel"));
        let snapshot = loaded.snapshot(dir.path(), None).unwrap();
        assert!(snapshot.sources.is_empty());
        assert!(!serde_json::to_string(&snapshot)
            .unwrap()
            .contains("private-sentinel"));
        assert!(matches!(
            loaded.activation_root(&id),
            Err(LibraryError::SourceNotFound)
        ));
        let reopened = LibraryRegistry::load_from(&path).unwrap();
        assert_eq!(reopened.retired_sources[0].raw.get(), row);
        let backup = dir.path().join(format!(
            "library.pre-v10-{:x}.json",
            Sha256::digest(original.as_bytes())
        ));
        assert_eq!(fs::read(backup).unwrap(), original.as_bytes());
        reopened.save_to(&path).unwrap();
        assert_eq!(
            LibraryRegistry::load_from(&path).unwrap().retired_sources[0]
                .raw
                .get(),
            row
        );
    }
}

#[test]
fn retired_ids_cannot_collide_with_live_membership_or_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.json");
    let notes = dir.path().join("notes");
    fs::create_dir(&notes).unwrap();
    let mut live = LibraryRegistry::empty();
    let source = live.add_root(&notes).unwrap();
    let live_row = serde_json::to_string(&live.sources[0]).unwrap();
    let duplicate = registry(9, &format!("{live_row},{}", record(&source.id)), "null");
    fs::write(&path, &duplicate).unwrap();
    assert!(matches!(
        LibraryRegistry::load_from(&path),
        Err(LibraryError::CorruptRegistry)
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), duplicate);
    let id = "b".repeat(64);
    let original = registry(9, &record(&id), "null").replace(
        "\"cleanup_pending\":[]",
        &format!(
            r#""cleanup_pending":[{{"source_id":"{id}","tombstone":"{}"}}]"#,
            "c".repeat(64)
        ),
    );
    fs::write(&path, &original).unwrap();
    assert!(matches!(
        LibraryRegistry::load_from(&path),
        Err(LibraryError::CorruptRegistry)
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn archiving_preserves_live_local_authority_and_never_repairs_unknown_live_kinds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.json");
    let notes = dir.path().join("notes");
    fs::create_dir(&notes).unwrap();
    let mut live = LibraryRegistry::empty();
    let source = live.add_root(&notes).unwrap();
    let row = serde_json::to_string(&live.sources[0]).unwrap();
    let original = registry(
        9,
        &format!("{row},{}", record(&"a".repeat(64))),
        &format!("\"{}\"", source.id),
    );
    fs::write(&path, &original).unwrap();
    let loaded = LibraryRegistry::load_from(&path).unwrap();
    assert_eq!(loaded.sources().len(), 1);
    assert_eq!(loaded.restore_source_id(), Some(source.id.as_str()));
    assert_eq!(
        loaded.activation_root(&source.id).unwrap().path,
        fs::canonicalize(notes).unwrap()
    );
    assert_eq!(loaded.retired_sources.len(), 1);
    let unknown = original.replace("local-folder", "unrecognized-live-provider");
    fs::write(&path, &unknown).unwrap();
    assert!(matches!(
        LibraryRegistry::load_from(&path),
        Err(LibraryError::CorruptRegistry)
    ));
    assert_eq!(fs::read_to_string(path).unwrap(), unknown);
}

#[test]
fn opaque_bounds_and_duplicate_identity_fields_are_checked_without_payload_errors() {
    let id = "a".repeat(64);
    let oversized = record(&id).replace("private-sentinel", &"x".repeat(MAX_RECORD_BYTES));
    for invalid in [
        oversized,
        record("wrong"),
        format!(r#"{{"kind":"connected-catdb","id":"{id}","id":"{id}"}}"#),
    ] {
        let error = serde_json::from_str::<RetiredMetadata>(&invalid).unwrap_err();
        assert!(!error.to_string().contains("private-sentinel"));
    }
    let repeated = (0..=MAX_LIBRARY_SOURCES)
        .map(|_| RawValue::from_string(record(&id)).unwrap())
        .collect();
    assert!(matches!(
        partition(repeated),
        Err(LibraryError::CorruptRegistry)
    ));
}

#[test]
fn backup_conflict_blocks_upgrade_without_changing_the_registry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.json");
    let original = registry(9, &record(&"a".repeat(64)), "null");
    fs::write(&path, &original).unwrap();
    let backup = dir.path().join(format!(
        "library.pre-v10-{:x}.json",
        Sha256::digest(original.as_bytes())
    ));
    fs::write(&backup, b"unrelated retained snapshot").unwrap();
    assert!(matches!(
        LibraryRegistry::load_from(&path),
        Err(LibraryError::PersistFailed)
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    assert_eq!(fs::read(backup).unwrap(), b"unrelated retained snapshot");
}
