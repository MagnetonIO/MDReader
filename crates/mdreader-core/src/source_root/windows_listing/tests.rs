//! Actual Windows handles/NTFS names, not synthetic directory-record output.
use super::*;
use std::collections::BTreeSet;

fn private_root() -> (tempfile::TempDir, SourceRoot) {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().canonicalize().unwrap().join("selected");
    mdreader_capture::PrivateDirectory::create(&path).unwrap();
    (temporary, SourceRoot::capture(path).unwrap())
}
fn names(entries: Vec<SourceEntry>) -> BTreeSet<OsString> {
    entries.into_iter().map(|entry| entry.name).collect()
}
#[test]
fn retained_listing_restarts_root_and_nested_cursors_with_exact_names() {
    let (_temporary, root) = private_root();
    fs::write(root.path.join("猫 % #.md"), b"original").unwrap();
    fs::create_dir(root.path.join("nested notes")).unwrap();
    fs::write(root.path.join("nested notes/child.md"), b"child").unwrap();
    fs::create_dir(root.path.join("nested notes/empty")).unwrap();
    let expected = BTreeSet::from([OsString::from("猫 % #.md"), OsString::from("nested notes")]);
    for _ in 0..3 {
        assert_eq!(
            names(root.read_dir_relative_bounded(Path::new(""), 2).unwrap()),
            expected
        );
        let children = root
            .read_dir_relative_bounded(Path::new("nested notes"), 2)
            .unwrap();
        assert_eq!(
            names(children),
            BTreeSet::from([OsString::from("child.md"), OsString::from("empty")])
        );
        assert!(root
            .read_dir_relative_bounded(Path::new("nested notes/empty"), 0)
            .unwrap()
            .is_empty());
    }
    root.verify().unwrap();
    assert_eq!(fs::read(root.path.join("猫 % #.md")).unwrap(), b"original");
}
#[test]
fn retained_listing_refuses_overflow_escape_and_directory_replacement() {
    let (temporary, root) = private_root();
    fs::write(root.path.join("a.md"), b"a").unwrap();
    fs::write(root.path.join("b.md"), b"b").unwrap();
    assert!(root.read_dir_relative_bounded(Path::new(""), 0).is_err());
    assert!(root.read_dir_relative_bounded(Path::new(""), 1).is_err());
    for target in ["..", "../outside", "C:\\", "\\\\server\\share"] {
        assert!(root
            .read_dir_relative_bounded(Path::new(target), 3)
            .is_err());
    }
    let replacement = temporary.path().join("replacement");
    fs::create_dir(&replacement).unwrap();
    assert!(fs::rename(&root.path, temporary.path().join("moved")).is_err());
    assert_eq!(
        names(root.read_dir_relative_bounded(Path::new(""), 2).unwrap()),
        BTreeSet::from([OsString::from("a.md"), OsString::from("b.md")])
    );
    root.verify().unwrap();
    assert_eq!(fs::read(root.path.join("a.md")).unwrap(), b"a");
}
