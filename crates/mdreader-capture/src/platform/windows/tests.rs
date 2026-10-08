use super::*;
use fs2::FileExt;

#[test]
fn unsupported_spool_durability_never_creates_a_namespace() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("not-created");
    assert!(matches!(
        Spool::open(&path, Limits::default()),
        Err(CaptureError::UnsupportedPlatform)
    ));
    assert!(!path.exists());
    assert!(matches!(
        sync_directory(temp.path()),
        Err(CaptureError::UnsupportedPlatform)
    ));
}

#[test]
fn private_handles_reject_hardlinks_and_lock_contention_is_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let private = temp.path().join("private");
    private_directory(&private).unwrap();
    // An inherited DACL is rejected instead of silently tightening an existing object.
    let inherited = private.join("inherited");
    fs::create_dir(&inherited).unwrap();
    assert!(matches!(
        private_directory(&inherited),
        Err(CaptureError::UnsafePath)
    ));
    let original = private.join("original");
    let mut file = private_open(&original, true).unwrap();
    file.write_all(b"synthetic visible output").unwrap();
    file.sync_all().unwrap();
    drop(file);
    fs::hard_link(&original, private.join("alias")).unwrap();
    assert!(matches!(
        read_bounded(&original, 64),
        Err(CaptureError::UnsafePath)
    ));

    let lock = private.join("queue.lock");
    let first = private_open(&lock, false).unwrap();
    let second = private_open(&lock, false).unwrap();
    first.try_lock_exclusive().unwrap();
    let error = second.try_lock_exclusive().unwrap_err();
    assert!(lock_would_block(&error));
    FileExt::unlock(&first).unwrap();
    second.try_lock_exclusive().unwrap();
    FileExt::unlock(&second).unwrap();
}

#[test]
fn pinned_namespace_and_handle_relative_replacement_preserve_exact_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let private = temp.path().join("private");
    private_directory(&private).unwrap();
    let chain = authority::Chain::open(&private, false, true).unwrap();
    assert!(fs::rename(&private, temp.path().join("replacement")).is_err());
    chain.verify().unwrap();
    let temporary = private.join("temporary");
    let destination = private.join("published");
    let mut file = private_open(&temporary, true).unwrap();
    file.write_all(b"# Exact synthetic body\n").unwrap();
    replace(&file, &temporary, &destination).unwrap();
    drop(file);
    assert!(!temporary.exists());
    assert_eq!(
        read_bounded(&destination, 64).unwrap(),
        b"# Exact synthetic body\n"
    );
    assert!(matches!(
        read_bounded(&destination, 2),
        Err(CaptureError::TooLarge)
    ));
}
