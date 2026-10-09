use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{fs, io::Read};

#[test]
fn transfer_copy_preserves_original_bytes_selected_image_and_revocation() {
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("profile");
    fs::create_dir(&profile).unwrap();
    let backend = crate::reader::ReaderBackend::embedded(&temp.path().join("owner")).unwrap();
    let runtime = crate::mobile_assets::AssetRuntime::default();
    let original =
        "\u{feff}# Original\r\n\r\n![Selected](images/blue.png)\r\n![Unselected](absent.jpg)\r\n";
    let document = backend
        .with_mobile_store(|store| {
            mdreader_core::device::import(
                store,
                &profile.join("device-markdown-v1"),
                "original.md".into(),
                original.into(),
            )
            .map_err(|e| e.to_string())
        })
        .unwrap();
    let reference = Reference {
        document_id: document.id.clone(),
        source_id: document.source_id.clone(),
        path: "document.md".into(),
        revision: document.revision.clone(),
    };
    let cache = temp.path().join("cache");
    let raw = package::prepare(&backend, &runtime, &profile, &cache, &reference).unwrap();
    assert_eq!(fs::read(&raw.path).unwrap(), original.as_bytes());
    assert_eq!(raw.mime, "text/markdown");
    assert_eq!(
        std::path::Path::new(&raw.path).file_name().unwrap(),
        "original.md"
    );
    assert_eq!(raw.name, "original.md");
    assert!(
        !profile.join("device-assets-v1").exists(),
        "export must not initialize asset metadata or files"
    );

    let png = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAADCAIAAAA2iEnWAAAAD0lEQVR4nGNgYPgPRsgUADjcBftBwbf8AAAAAElFTkSuQmCC";
    let bytes = STANDARD.decode(png).unwrap();
    let prepared = backend
        .with_mobile_store(|store| {
            let assets = runtime
                .assets(store, &profile.join("device-assets-v1"))
                .map_err(|e| e.to_string())?;
            let manifest = assets
                .manifest(store, &document.id, &document.revision)
                .map_err(|e| e.to_string())?;
            let id = assets
                .begin(
                    store,
                    &document.id,
                    &document.revision,
                    "images/blue.png",
                    bytes.len() as u32,
                    &manifest.manifest,
                )
                .map_err(|e| e.to_string())?;
            assets
                .write(store, &id, 0, png)
                .map_err(|e| e.to_string())?;
            assets
                .prepare_commit(store, &id, runtime.validation_permit().unwrap())
                .map_err(|e| e.to_string())
        })
        .unwrap();
    let validated = prepared.validate().unwrap();
    backend
        .with_mobile_store(|store| {
            runtime
                .assets(store, &profile.join("device-assets-v1"))
                .and_then(|a| a.publish(store, validated))
                .map_err(|e| e.to_string())
        })
        .unwrap();
    let bundle = package::prepare(&backend, &runtime, &profile, &cache, &reference).unwrap();
    assert_eq!(bundle.mime, "application/zip");
    assert_eq!(
        std::path::Path::new(&bundle.path).file_name().unwrap(),
        "original.zip"
    );
    let mut archive = zip::ZipArchive::new(fs::File::open(bundle.path).unwrap()).unwrap();
    assert_eq!(
        archive.len(),
        2,
        "only the explicit ready image is included"
    );
    let mut body = Vec::new();
    archive
        .by_name("original.md")
        .unwrap()
        .read_to_end(&mut body)
        .unwrap();
    assert_eq!(body, original.as_bytes());
    body.clear();
    archive
        .by_name("images/blue.png")
        .unwrap()
        .read_to_end(&mut body)
        .unwrap();
    assert_eq!(body, bytes);
    let selected = snapshot::prepare(
        &backend,
        &runtime,
        &profile.join("device-markdown-v1"),
        &profile.join("device-assets-v1"),
        &reference,
    )
    .unwrap();
    backend
        .with_mobile_store(|store| {
            store
                .set_source_enabled(&reference.source_id, false)
                .map_err(|e| e.to_string())
        })
        .unwrap();
    assert!(snapshot::revalidate(
        &backend,
        &runtime,
        &profile.join("device-markdown-v1"),
        &profile.join("device-assets-v1"),
        &reference,
        &selected
    )
    .is_err());
    assert!(package::prepare(&backend, &runtime, &profile, &cache, &reference).is_err());
    assert_eq!(
        fs::read(
            profile
                .join("device-markdown-v1")
                .join(&document.id)
                .join("document.md")
        )
        .unwrap(),
        original.as_bytes()
    );
}

#[test]
#[cfg(unix)]
fn transfer_staging_releases_finished_copies_and_expires_legacy_without_following_links() {
    use std::{
        fs::FileTimes,
        path::Path,
        time::{Duration, SystemTime},
    };
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let root = cache.join("mdreader-transfers-v1");
    for _ in 0..5 {
        let copy = staging::create(&cache, "Notes.md").unwrap();
        let (path, _, _) = copy.finish().unwrap();
        assert_eq!(path.file_name().unwrap(), "Notes.md");
        staging::release(&cache, &path).unwrap();
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    }
    assert!(staging::create(&cache, "../outside.md").is_err());
    let authority = mdreader_core::SourceRoot::capture(root.clone()).unwrap();
    let legacy = format!("{}-Old.md", uuid::Uuid::new_v4());
    let file = authority.create_new_relative(Path::new(&legacy)).unwrap();
    file.set_times(FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(3601)))
        .unwrap();
    let copy = staging::create(&cache, "Current.md").unwrap();
    assert!(!root.join(legacy).exists());
    let (path, _, _) = copy.finish().unwrap();
    fs::write(path.parent().unwrap().join("unrelated.txt"), "keep").unwrap();
    assert!(
        staging::release(&cache, &path).is_err(),
        "never remove a nonempty directory"
    );
    assert_eq!(
        fs::read(path.parent().unwrap().join("unrelated.txt")).unwrap(),
        b"keep"
    );
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("Keep.md"), "unchanged").unwrap();
    let link = uuid::Uuid::new_v4().to_string();
    std::os::unix::fs::symlink(&outside, root.join(&link)).unwrap();
    assert!(authority
        .remove_empty_directory_relative(Path::new(&link))
        .is_err());
    assert!(staging::create(&cache, "Refused.md").is_err());
    assert_eq!(fs::read(outside.join("Keep.md")).unwrap(), b"unchanged");
}

#[cfg(unix)]
#[test]
fn provider_transfer_preserves_exact_cached_bytes_and_refuses_revoked_or_changed_revision() {
    use mdreader_core::ProviderProjection;
    use mdreader_provider::{
        AccountBinding, DisplayText, DocumentBody, DocumentVersion, LibraryId, MarkdownBytes,
        Metadata, OpaqueValue, RecordKey, SourceBinding, SourceId,
    };
    let temp = tempfile::tempdir().unwrap();
    let backend = crate::reader::ReaderBackend::embedded(&temp.path().join("owner")).unwrap();
    let library = LibraryId::new("public-test-library").unwrap();
    let projection =
        ProviderProjection::open(&temp.path().join("provider-cache"), library.clone()).unwrap();
    let binding = SourceBinding::new(
        library,
        SourceId::new("public-drive").unwrap(),
        AccountBinding::new("public-account").unwrap(),
        uuid::Uuid::new_v4(),
        "1".repeat(64),
    )
    .unwrap();
    let original =
        "\u{feff}# Exact provider\r\n\r\nUTF-8: λ.\r\n![Not selected](unfetched.png)\r\n";
    let key = RecordKey::new("opaque/remote-key").unwrap();
    let version = DocumentVersion {
        provider_revision: OpaqueValue::new("revision-1").unwrap(),
        content_hash: None,
        conditional_write_token: None,
    };
    let metadata = Metadata {
        key: key.clone(),
        name: DisplayText::new("../../not-a-native-path.md").unwrap(),
        display_path: DisplayText::new("remote display only").unwrap(),
        media_type: None,
        modified_at_ms: None,
        size_bytes: Some(original.len() as u64),
        version: version.clone(),
    };
    let document = DocumentBody {
        key,
        version,
        body: MarkdownBytes::new(original.as_bytes().to_vec()).unwrap(),
        observed_at_ms: 1,
    };
    let reference = backend
        .with_mobile_store(|store| {
            store.with_library_connection(|db| {
                let tx = db.transaction().unwrap();
                let value = projection
                    .admit(&tx, &binding, &metadata, &document)
                    .unwrap();
                tx.commit().unwrap();
                Ok(crate::mobile_provider_state::ProviderReadingReference {
                    source_id: value.reader_source_id,
                    path: value.path,
                    revision: value.revision,
                })
            })
        })
        .unwrap();
    let cache = temp.path().join("cache");
    let copy = provider::prepare(&backend, &cache, &reference).unwrap();
    assert_eq!(fs::read(&copy.path).unwrap(), original.as_bytes());
    assert_eq!(
        (copy.name.as_str(), copy.mime.as_str()),
        ("provider-document.md", "text/markdown")
    );
    assert!(std::path::Path::new(&copy.path).starts_with(cache.canonicalize().unwrap()));
    assert!(!temp.path().join("owner/device-markdown-v1").exists());
    assert!(!temp.path().join("owner/device-assets-v1").exists());
    let changed = crate::mobile_provider_state::ProviderReadingReference {
        revision: "0".repeat(64),
        ..reference.clone()
    };
    assert_eq!(
        provider::prepare(&backend, &cache, &changed)
            .err()
            .as_deref(),
        Some("stale")
    );
    backend
        .with_mobile_store(|store| {
            store.with_library_connection(|db| {
                db.execute("UPDATE core_provider_slots SET enabled=0", [])
                    .unwrap()
            });
            Ok(())
        })
        .unwrap();
    assert_eq!(
        provider::prepare(&backend, &cache, &reference)
            .err()
            .as_deref(),
        Some("stale")
    );
}
