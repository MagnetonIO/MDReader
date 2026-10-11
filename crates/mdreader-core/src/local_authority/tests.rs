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
