//! Public, disposable desktop Save-a-copy fixture. No app/service launch or cleanup.
//! Requires device-assets; uses the real device import and exact read APIs.
#[cfg(all(unix, feature = "device-assets"))]
mod fixture {
    use mdreader_core::{device, ReaderStore, SourceRoot};
    use serde::{Deserialize, Serialize};
    use sha2::{Digest, Sha256};
    use std::{
        fs,
        io::{Read, Write},
        path::{Path, PathBuf},
    };

    const ORIGINAL: &str = "# Public save-copy check\n\nThis synthetic original stays unchanged.\n";
    const DRAFT: &str = "# Public save-copy check\n\nUnsavedcopyneedle2026 public draft.\n";
    const COLLISION: &[u8] = b"Public existing destination; never overwrite.\n";
    const MARKER: &[u8] = b"mdreader-native-e2e-profile-v1";
    const SOURCE_ID: &str = "00000000-0000-0000-0000-00000000e2e0";

    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Witness {
        version: u8,
        document_id: String,
        original_sha256: String,
        draft_sha256: String,
    }
    fn hash(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }
    fn write(
        root: &SourceRoot,
        path: &str,
        bytes: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut file = root.create_new_relative(Path::new(path))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }
    fn read(root: &SourceRoot, path: &Path) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
        let Some(file) = root.open_optional_file_relative(path)? else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        file.take(4097).read_to_end(&mut bytes)?;
        if bytes.len() > 4096 {
            return Err("public fixture file exceeded its bound".into());
        }
        Ok(Some(bytes))
    }
    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let args = std::env::args_os().skip(1).collect::<Vec<_>>();
        if args.len() == 1 && args[0] == "--create-disposable-public-fixture" {
            let temporary = tempfile::Builder::new()
                .prefix("mdreader-e2e-save-copy-")
                .tempdir()?;
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700))?;
            let root = SourceRoot::capture(temporary.path().canonicalize()?)?;
            let corpus = root.create_or_capture_directory_relative(Path::new("corpus"))?;
            let copies = root.create_or_capture_directory_relative(Path::new("copies"))?;
            write(&root, ".mdreader-e2e-profile-v1", MARKER)?;
            write(&corpus, "public-save-copy.md", ORIGINAL.as_bytes())?;
            write(&root, "expected-draft.md", DRAFT.as_bytes())?;
            write(&copies, "collision.md", COLLISION)?;
            let mut store = ReaderStore::open(&root.path.join("reader-v1"))?;
            let document = device::import(
                &mut store,
                &root.path.join("device-markdown-v1"),
                "public-save-copy.md".into(),
                ORIGINAL.into(),
            )?;
            let actual = device::read(&mut store, &document.id, Some(&document.revision))?;
            if actual.markdown != ORIGINAL
                || !document.indexed
                || device::search(&mut store, "synthetic")?.len() != 1
            {
                return Err("real device import/read/search did not match public fixture".into());
            }
            drop(store); // Release the only writer before any GUI/service starts.
            let witness = Witness {
                version: 1,
                document_id: document.id.clone(),
                original_sha256: hash(ORIGINAL.as_bytes()),
                draft_sha256: hash(DRAFT.as_bytes()),
            };
            write(
                &root,
                "save-copy-witness.json",
                &serde_json::to_vec(&witness)?,
            )?;
            root.try_clone_directory_handle()?.sync_all()?;
            root.verify()?;
            let kept = temporary.keep();
            println!(
                "{}",
                serde_json::json!({"kind":"public-disposable-save-copy-fixture",
                "profileRoot": kept.canonicalize()?, "corpusRoot": corpus.path,
                "sourceId": SOURCE_ID, "documentId": document.id,
                "originalSha256": witness.original_sha256, "draftSha256": witness.draft_sha256,
                "copyDestination": copies.path.join("saved-copy.md"),
                "collisionDestination": copies.path.join("collision.md"),
                "expectedDraft": root.path.join("expected-draft.md"),
                "launchesProcesses":false,"acceptsNativeDialog":false})
            );
            return Ok(());
        }
        if args.len() != 3 || args[0] != "--verify" {
            return Err("usage: --create-disposable-public-fixture | --verify ABS_PROFILE cancelled|collision|saved".into());
        }
        let path = PathBuf::from(&args[1]);
        let metadata = fs::symlink_metadata(&path)?;
        use std::os::unix::fs::PermissionsExt;
        if !path.is_absolute()
            || !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err("invalid disposable fixture root".into());
        }
        let path = path.canonicalize()?;
        if path.parent() != Some(std::env::temp_dir().canonicalize()?.as_path())
            || !path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with("mdreader-e2e-save-copy-"))
        {
            return Err("fixture must be an immediate owned temporary root".into());
        }
        let root = SourceRoot::capture(path)?;
        if read(&root, Path::new(".mdreader-e2e-profile-v1"))?.as_deref() != Some(MARKER) {
            return Err("fixture marker mismatch".into());
        }
        let witness: Witness = serde_json::from_slice(
            &read(&root, Path::new("save-copy-witness.json"))?.ok_or("missing fixture witness")?,
        )?;
        if witness.version != 1
            || uuid::Uuid::parse_str(&witness.document_id)?.to_string() != witness.document_id
            || witness.original_sha256 != hash(ORIGINAL.as_bytes())
            || witness.draft_sha256 != hash(DRAFT.as_bytes())
        {
            return Err("fixture witness mismatch".into());
        }
        for name in [
            PathBuf::from("corpus/public-save-copy.md"),
            PathBuf::from(format!(
                "device-markdown-v1/{}/document.md",
                witness.document_id
            )),
        ] {
            if read(&root, &name)?.as_deref() != Some(ORIGINAL.as_bytes()) {
                return Err("original bytes changed".into());
            }
        }
        if read(&root, Path::new("copies/collision.md"))?.as_deref() != Some(COLLISION) {
            return Err("existing destination changed".into());
        }
        let saved = read(&root, Path::new("copies/saved-copy.md"))?;
        match args[2].to_str() {
            Some("cancelled" | "collision") if saved.is_none() => {}
            Some("saved") if saved.as_deref() == Some(DRAFT.as_bytes()) => {}
            _ => return Err("unexpected destination outcome".into()),
        }
        root.verify()?;
        println!(
            "{}",
            serde_json::json!({"kind":"public-save-copy-file-verification",
            "outcome":args[2].to_str(),"originalUnchanged":true,"ownedCopyUnchanged":true,
            "collisionUnchanged":true,"savedCopyMatchesDraft":saved.is_some(),
            "originalSha256":witness.original_sha256,"draftSha256":witness.draft_sha256,
            "acceptsBufferRetention":false,"acceptsNativeDialog":false})
        );
        Ok(())
    }
}
#[cfg(all(unix, feature = "device-assets"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    fixture::run()
}
#[cfg(not(all(unix, feature = "device-assets")))]
fn main() {
    eprintln!("This disposable desktop fixture requires Unix and device-assets.");
    std::process::exit(2);
}
