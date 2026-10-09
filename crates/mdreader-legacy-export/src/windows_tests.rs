use super::*;
use mdreader_capture::SelectedDirectory;
use std::io::Write;

#[test]
fn private_ntfs_archive_export_copy_reviews_and_profile_preserve_original_bytes() {
    let temporary = tempfile::tempdir().unwrap();
    let selected = SelectedDirectory::open(temporary.path()).unwrap();
    let private = selected.create_private_child(Path::new("private")).unwrap();
    let root = temporary.path().join("private");
    let input = b"{\"future\":9223372036854775807,\"settings\":{\"theme\":\"dark\"}}";
    let mut file = private
        .create_database_file(Path::new("settings.json"))
        .unwrap();
    file.write_all(input).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let inputs = [Input {
        role: Role::Settings,
        path: root.join("settings.json"),
    }];
    let limits = Limits::default();
    let archive = root.join("archive");
    export(&inputs, &archive, &limits).unwrap();
    let identity = migration::archive::identity(&archive, &limits).unwrap();
    assert_eq!(std::fs::read(archive.join("0000.raw")).unwrap(), input);
    assert_eq!(std::fs::read(&inputs[0].path).unwrap(), input);
    assert!(matches!(
        export(&inputs, &archive, &limits),
        Err(Error::Conflict)
    ));
    migration::archive::copy(&archive, &identity, &root.join("copy"), &limits).unwrap();
    assert_eq!(
        migration::archive::identity(&root.join("copy"), &limits).unwrap(),
        identity
    );
    let spec = SnapshotSpec::from_inputs(&inputs).unwrap();
    let mut file = private
        .create_database_file(Path::new("spec.json"))
        .unwrap();
    file.write_all(&serde_json::to_vec(&spec).unwrap()).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let prepared =
        migration::prepare(&root.join("spec.json"), &root.join("profile"), &limits).unwrap();
    assert_eq!(prepared.version, 1);
    assert_eq!(
        std::fs::read(root.join("profile/settings.json")).unwrap(),
        input
    );
    migration::verify(&root.join("profile"), &limits).unwrap();
    let reviews = reviews::ReviewArchive::open(
        &root.join("profile/archived-reviews"),
        &prepared.archive,
        &limits,
    )
    .unwrap();
    assert!(reviews.list(0, 10).unwrap().records.is_empty());
    drop(reviews);
    let materialized = migration::archive::prepare(
        &archive,
        &identity,
        &root.join("materialized"),
        &limits,
        |_, _, destination| {
            assert!(destination.join("legacy-archive/0000.raw").is_file());
            Ok(0)
        },
    )
    .unwrap();
    assert_eq!(materialized.materialized_records, Some(0));
    assert!(!root.join("materialized/settings.json").exists());
    migration::verify(&root.join("materialized"), &limits).unwrap();
    private.verify().unwrap();
    selected.verify().unwrap();
}
