use super::*;
use std::fs;
#[test]
fn broad_parent_excludes_private_originals_from_reads_search_catalog_and_mutations() {
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("owned-private");
    let notes = root.path().join("public");
    fs::create_dir(&notes).unwrap();
    let mut store = ReaderStore::open(&private).unwrap();
    fs::write(private.join("secret.md"), "# Private\nprivatecanary").unwrap();
    fs::write(notes.join("public.md"), "# Public\npubliccanary").unwrap();
    let source = store
        .add_local_source(
            "Selected parent",
            SourceRoot::capture(root.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    store.index_source(&source.id).unwrap();
    assert_eq!(
        store
            .search("publiccanary", std::slice::from_ref(&source.id), 10)
            .unwrap()
            .len(),
        1
    );
    assert!(store
        .search("privatecanary", std::slice::from_ref(&source.id), 10)
        .unwrap()
        .is_empty());
    assert!(store
        .read_document(&source.id, "owned-private/secret.md", None)
        .is_err());
    let authority = store.stored_root(&source.id, true).unwrap();
    assert!(authority
        .open_optional_file_relative(Path::new("owned-private/secret.md"))
        .is_err());
    assert!(authority
        .create_new_relative(Path::new("owned-private/new.md"))
        .is_err());
    assert!(authority
        .rename_relative(
            Path::new("public/public.md"),
            Path::new("owned-private/moved.md")
        )
        .is_err());
    assert!(store
        .add_local_source(
            "Private",
            SourceRoot::capture(private.canonicalize().unwrap()).unwrap()
        )
        .is_err());
    let page = store.catalog_page(&source.id, None, None, 100).unwrap();
    assert_eq!(page.documents.len(), 1);
    assert_eq!(page.documents[0].path, "public/public.md");
    assert_eq!(
        fs::read_to_string(private.join("secret.md")).unwrap(),
        "# Private\nprivatecanary"
    );
}
#[test]
fn legacy_indexed_private_row_is_unavailable_before_any_rebuild() {
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("owned-private");
    let mut store = ReaderStore::open(&private).unwrap();
    let source = store
        .add_local_source(
            "Parent",
            SourceRoot::capture(root.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    let tx = store.index.transaction().unwrap();
    crate::store::index_markdown(
        &tx,
        &source.id,
        "OwNeD-PrIvAtE/retained.md",
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "# Old\noldprivatecanary",
        0,
        "legacy",
    )
    .unwrap();
    crate::store::catalog::schema::publish(&tx, &source.id).unwrap();
    tx.commit().unwrap();
    assert!(store
        .search("oldprivatecanary", std::slice::from_ref(&source.id), 10)
        .unwrap()
        .is_empty());
    assert!(store
        .catalog_page(&source.id, None, None, 100)
        .unwrap()
        .documents
        .is_empty());
    drop(store);
    let store = ReaderStore::open_readonly(&private).unwrap();
    assert!(store
        .search("oldprivatecanary", &[source.id], 10)
        .unwrap()
        .is_empty());
}

#[cfg(target_os = "macos")]
#[test]
fn case_insensitive_volume_private_aliases_cannot_read_or_mutate() {
    let parent = tempfile::tempdir().unwrap();
    let private = parent.path().join("owned-private");
    let mut store = ReaderStore::open(&private).unwrap();
    fs::write(private.join("secret.md"), "# Private\naliascanary").unwrap();
    let alias = parent.path().join("OwNeD-PrIvAtE");
    if !alias.exists() {
        return;
    } // Actual temp volume demonstrably case-sensitive.
    use std::os::unix::fs::MetadataExt;
    assert_eq!(
        fs::metadata(&alias).unwrap().ino(),
        fs::metadata(&private).unwrap().ino()
    );
    let source = store
        .add_local_source(
            "Parent",
            SourceRoot::capture(parent.path().canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    assert!(store
        .read_document(&source.id, "OwNeD-PrIvAtE/secret.md", None)
        .is_err());
    let root = store.stored_root(&source.id, true).unwrap();
    assert!(root
        .create_new_relative(Path::new("OwNeD-PrIvAtE/new.md"))
        .is_err());
    assert!(root
        .open_file_relative(Path::new("OwNeD-PrIvAtE/secret.md"))
        .is_err());
    assert_eq!(
        fs::read_to_string(private.join("secret.md")).unwrap(),
        "# Private\naliascanary"
    );
}

#[test]
fn unicode_private_aliases_mask_legacy_rows_and_preserve_unrelated_folders() {
    let parent = tempfile::tempdir().unwrap();
    let home = parent.path().join("hôme");
    fs::create_dir(&home).unwrap();
    let private = home.join("privé");
    let notes = home.join("public-notes");
    fs::create_dir(&notes).unwrap();
    let mut store = ReaderStore::open(&private).unwrap();
    fs::write(private.join("secret.md"), "# Private\nunicodeprivatecanary").unwrap();
    fs::write(home.join("public.md"), "# Public\nunicodepubliccanary").unwrap();
    fs::write(notes.join("note.md"), "# Selected\nselectedunicodecanary").unwrap();
    let source = store
        .add_local_source(
            "Unicode home",
            SourceRoot::capture(home.canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    let selected = store
        .add_local_source(
            "Selected sibling",
            SourceRoot::capture(notes.canonicalize().unwrap()).unwrap(),
        )
        .unwrap();
    // Also cover a future private name whose uppercase-table spelling is ASCII.
    store
        .library
        .execute(
            "INSERT OR IGNORE INTO reader_local_private_v1 VALUES(?1)",
            [encode_path(&home.join("prıvate"))],
        )
        .unwrap();
    refresh(&store.library).unwrap();
    store.index_source(&source.id).unwrap();
    store.index_source(&selected.id).unwrap();
    assert_eq!(
        store
            .search("unicodepubliccanary", std::slice::from_ref(&source.id), 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .search(
                "selectedunicodecanary",
                std::slice::from_ref(&selected.id),
                10
            )
            .unwrap()
            .len(),
        1
    );
    let tx = store.index.transaction().unwrap();
    for path in [
        "PRIVÉ/secret.md",
        "PRIVE\u{301}/secret.md",
        "PRIVATE/secret.md",
    ] {
        crate::store::index_markdown(
            &tx,
            &source.id,
            path,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "# Private\nunicodeprivatecanary",
            0,
            "legacy",
        )
        .unwrap();
    }
    crate::store::catalog::schema::publish(&tx, &source.id).unwrap();
    tx.commit().unwrap();
    assert!(store
        .search("unicodeprivatecanary", std::slice::from_ref(&source.id), 10)
        .unwrap()
        .is_empty());
    assert!(!store
        .catalog_page(&source.id, None, None, 100)
        .unwrap()
        .documents
        .iter()
        .any(|d| d.path.starts_with("PRIV")));
    assert!(store
        .read_document(&source.id, "PRIVÉ/secret.md", None)
        .is_err());
    assert!(store
        .read_document(&source.id, "PRIVATE/secret.md", None)
        .is_err());
    assert!(store
        .stored_root(&source.id, true)
        .unwrap()
        .create_new_relative(Path::new("PRIVE\u{301}/new.md"))
        .is_err());
    drop(store);
    let store = ReaderStore::open_readonly(&private).unwrap();
    assert!(store
        .search("unicodeprivatecanary", std::slice::from_ref(&source.id), 10)
        .unwrap()
        .is_empty());
    assert_eq!(
        store
            .search("unicodepubliccanary", std::slice::from_ref(&source.id), 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        fs::read_to_string(private.join("secret.md")).unwrap(),
        "# Private\nunicodeprivatecanary"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn actual_unicode_case_alias_refuses_before_private_file_open() {
    let parent = tempfile::tempdir().unwrap();
    let private = parent.path().join("privé");
    fs::create_dir(&private).unwrap();
    fs::write(private.join("secret.md"), "# Private\nunicodealiascanary").unwrap();
    let alias = parent.path().join("PRIVÉ");
    if !alias.exists() {
        return;
    } // Demonstrably case-sensitive temp volume.
    use std::os::unix::fs::MetadataExt;
    assert_eq!(
        fs::metadata(&alias).unwrap().ino(),
        fs::metadata(&private).unwrap().ino()
    );
    let root = SourceRoot::capture(parent.path().canonicalize().unwrap())
        .unwrap()
        .excluding(vec![private.canonicalize().unwrap()])
        .unwrap();
    for path in ["PRIVÉ/secret.md", "PRIVE\u{301}/secret.md"] {
        assert!(root.open_file_relative(Path::new(path)).is_err());
    }
    assert!(root.create_new_relative(Path::new("PRIVÉ/new.md")).is_err());
    fs::write(parent.path().join("public.md"), "# Public").unwrap();
    assert!(root.open_file_relative(Path::new("public.md")).is_ok());
    assert_eq!(
        fs::read_to_string(private.join("secret.md")).unwrap(),
        "# Private\nunicodealiascanary"
    );
}

#[test]
fn bounded_private_predicate_preserves_components_and_refuses_malformed_evidence() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    sqlite_match::register(&db).unwrap();
    for (path, prefix, denied) in [
        ("PRIVÉ/n.md", "privé", true),
        ("PRIVE\u{301}/n.md", "privé", true),
        ("PRIVATE/n.md", "prıvate", true),
        ("private-other/n.md", "private", false),
        ("Public/n.md", "private", false),
        ("../private/n.md", "private", true),
        ("/private/n.md", "private", true),
    ] {
        assert_eq!(
            db.query_row(
                "SELECT mdreader_private_prefix_v1(?1,?2)",
                params![path, prefix],
                |r| r.get::<_, bool>(0)
            )
            .unwrap(),
            denied
        );
    }
    assert!(db
        .query_row(
            "SELECT mdreader_private_prefix_v1(?1,'private')",
            ["a/".repeat(513)],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    assert!(db
        .query_row(
            "SELECT mdreader_private_prefix_v1(?1,'private')",
            ["a".repeat(32769)],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    assert!(db
        .query_row(
            "SELECT mdreader_private_prefix_v1(NULL,'private')",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    assert!(crate::source_root::short_name_component(
        std::ffi::OsStr::new("PRIVE~1")
    ));
    assert!(!crate::source_root::short_name_component(
        std::ffi::OsStr::new("public~draft.md")
    ));
    #[cfg(windows)]
    assert!(db
        .query_row(
            "SELECT mdreader_private_prefix_v1('PRIVE~1/n.md','privé')",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
}
