//! Immutable demo deployment identity.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use catdb_embed::HashEmbedder;
use catdb_engine::{EmbeddedCatDb, EngineConfig, EngineResult};
use rusqlite::{Connection, OptionalExtension, MAIN_DB};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

/// Versioned identity written into the immutable demo image.
pub const DEMO_DEPLOYMENT_FORMAT: &str = "catdb-demo-deployment/v1";
const MAX_DEMO_SNAPSHOT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_DEMO_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_CORPUS_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_CORPUS_BYTES: usize = 64 * 1024 * 1024;

/// Identity bound to one immutable corpus and query-serving SQLite snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemoDeploymentManifest {
    /// Manifest format discriminator.
    pub format: String,
    /// Exact Git commit compiled into the runtime binary.
    pub source_commit: String,
    /// Authoritative catalog revision read from the opened engine.
    pub catalog_revision: Uuid,
    /// Stable digest of sorted source-relative corpus paths and bytes.
    pub corpus_sha256: String,
    /// Stable digest of normalized query-serving workspace state.
    pub logical_workspace_blake3: String,
    /// Digest of the closed SQLite snapshot bytes.
    pub sqlite_sha256: String,
    /// Digest binding all deployment identity fields above.
    pub deployment_fingerprint: String,
}

/// Image preparation failed before an immutable deployment identity was made.
#[derive(Debug, Error)]
pub enum DemoDeploymentError {
    /// Filesystem access failed.
    #[error("{context}: {source}")]
    Io {
        /// Operation that failed.
        context: String,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// CatDB could not open or inspect the workspace.
    #[error("opening authoritative CatDB workspace: {0}")]
    Engine(#[from] catdb_engine::EngineError),
    /// An identity input was malformed or unsafe.
    #[error("{0}")]
    InvalidIdentity(String),
    /// Manifest serialization failed.
    #[error("serializing deployment manifest: {0}")]
    Serialization(#[from] serde_json::Error),
    /// The immutable SQLite snapshot was malformed or inconsistent.
    #[error("invalid SQLite snapshot: {0}")]
    Snapshot(String),
}

fn io(context: impl Into<String>, source: std::io::Error) -> DemoDeploymentError {
    DemoDeploymentError::Io {
        context: context.into(),
        source,
    }
}

fn validate_source_commit(source_commit: &str) -> Result<(), DemoDeploymentError> {
    if source_commit.len() != 40
        || !source_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(DemoDeploymentError::InvalidIdentity(
            "source commit must be an exact lowercase 40-hex Git commit".to_string(),
        ));
    }
    Ok(())
}

fn sqlite_sidecar(database: &Path, suffix: &str) -> PathBuf {
    let mut value: OsString = database.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

fn require_closed_sqlite_snapshot(database: &Path) -> Result<(), DemoDeploymentError> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = sqlite_sidecar(database, suffix);
        if sidecar.exists() {
            return Err(DemoDeploymentError::InvalidIdentity(format!(
                "SQLite snapshot has an adjacent {suffix} sidecar"
            )));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn open_regular_read_only(path: &Path) -> Result<fs::File, DemoDeploymentError> {
    use rustix::fs::{open, Mode, OFlags};

    let descriptor = open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| {
        DemoDeploymentError::InvalidIdentity(format!(
            "image input must be a no-follow regular file: {error}"
        ))
    })?;
    let file = fs::File::from(descriptor);
    if !file
        .metadata()
        .map_err(|error| io("inspecting retained image input", error))?
        .is_file()
    {
        return Err(DemoDeploymentError::InvalidIdentity(
            "image input must be a regular file".to_string(),
        ));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_regular_read_only(path: &Path) -> Result<fs::File, DemoDeploymentError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| io(format!("inspecting {}", path.display()), error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(DemoDeploymentError::InvalidIdentity(
            "image input must be a no-follow regular file".to_string(),
        ));
    }
    let file =
        fs::File::open(path).map_err(|error| io(format!("opening {}", path.display()), error))?;
    if !file
        .metadata()
        .map_err(|error| io("inspecting retained image input", error))?
        .is_file()
    {
        return Err(DemoDeploymentError::InvalidIdentity(
            "image input must be a regular file".to_string(),
        ));
    }
    Ok(file)
}

fn sha256_reader(file: &mut fs::File) -> Result<String, DemoDeploymentError> {
    file.rewind()
        .map_err(|error| io("rewinding retained image input", error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| io("reading retained image input", error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn sha256_file(path: &Path) -> Result<String, DemoDeploymentError> {
    sha256_reader(&mut open_regular_read_only(path)?)
}

fn read_bounded_regular_file(
    path: &Path,
    maximum: u64,
    kind: &str,
) -> Result<Vec<u8>, DemoDeploymentError> {
    let mut file = open_regular_read_only(path)?;
    let length = file
        .metadata()
        .map_err(|error| io(format!("inspecting retained {kind}"), error))?
        .len();
    if length > maximum {
        return Err(DemoDeploymentError::InvalidIdentity(format!(
            "{kind} exceeds its {maximum}-byte limit"
        )));
    }
    let capacity = usize::try_from(length).map_err(|_| {
        DemoDeploymentError::InvalidIdentity(format!("{kind} does not fit in memory"))
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes)
        .map_err(|error| io(format!("reading retained {kind}"), error))?;
    Ok(bytes)
}

fn collect_corpus_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), DemoDeploymentError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| {
            io(
                format!("reading corpus directory {}", directory.display()),
                error,
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            io(
                format!("reading corpus directory {}", directory.display()),
                error,
            )
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry
            .file_type()
            .map_err(|error| io(format!("inspecting {}", entry.path().display()), error))?;
        let path = entry.path();
        if file_type.is_symlink() {
            return Err(DemoDeploymentError::InvalidIdentity(
                "corpus contains a symbolic link".to_string(),
            ));
        }
        if file_type.is_dir() {
            collect_corpus_files(root, &path, files)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(DemoDeploymentError::InvalidIdentity(
                "corpus contains a non-file entry".to_string(),
            ));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| {
                DemoDeploymentError::InvalidIdentity(
                    "corpus traversal escaped its root".to_string(),
                )
            })?
            .to_str()
            .ok_or_else(|| {
                DemoDeploymentError::InvalidIdentity("corpus path is not valid UTF-8".to_string())
            })?
            .replace(std::path::MAIN_SEPARATOR, "/");
        let bytes = read_bounded_regular_file(&path, MAX_CORPUS_FILE_BYTES, "corpus file")?;
        files.push((relative, bytes));
    }
    Ok(())
}

fn corpus_files(root: &Path) -> Result<Vec<(String, Vec<u8>)>, DemoDeploymentError> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| io(format!("inspecting corpus {}", root.display()), error))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(DemoDeploymentError::InvalidIdentity(
            "corpus root must be a real directory".to_string(),
        ));
    }
    let mut files = Vec::new();
    collect_corpus_files(root, root, &mut files)?;
    if files.is_empty() {
        return Err(DemoDeploymentError::InvalidIdentity(
            "corpus must contain at least one file".to_string(),
        ));
    }
    let total_bytes = files
        .iter()
        .try_fold(0_usize, |total, (_, bytes)| total.checked_add(bytes.len()))
        .ok_or_else(|| {
            DemoDeploymentError::InvalidIdentity("corpus byte count overflowed".to_string())
        })?;
    if total_bytes > MAX_CORPUS_BYTES {
        return Err(DemoDeploymentError::InvalidIdentity(format!(
            "corpus exceeds its {MAX_CORPUS_BYTES}-byte limit"
        )));
    }
    Ok(files)
}

fn corpus_sha256(files: &[(String, Vec<u8>)]) -> String {
    let mut ordered = files.to_vec();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    hasher.update(b"catdb-demo-corpus/v1\0");
    for (path, bytes) in ordered {
        hasher.update((path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn deployment_fingerprint(
    source_commit: &str,
    catalog_revision: Uuid,
    corpus_sha256: &str,
    logical_workspace_blake3: &str,
    sqlite_sha256: &str,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"catdb-demo-deployment/v1\0");
    for value in [
        source_commit,
        &catalog_revision.to_string(),
        corpus_sha256,
        logical_workspace_blake3,
        sqlite_sha256,
    ] {
        hash_part(&mut hasher, value.as_bytes());
    }
    format!("blake3:{}", hasher.finalize().to_hex())
}

fn validate_digest(value: &str, prefix: &str, field: &str) -> Result<(), DemoDeploymentError> {
    if value.len() != prefix.len() + 64
        || !value.starts_with(prefix)
        || !value[prefix.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(DemoDeploymentError::InvalidIdentity(format!(
            "{field} must be a lowercase {prefix} digest"
        )));
    }
    Ok(())
}

/// Verified private runtime copy and the exact engine opened from it.
///
/// Keeping this value alive keeps the private directory alive for the server
/// lifetime.
pub struct PreparedDemoRuntime {
    runtime_directory: tempfile::TempDir,
    engine: Arc<EmbeddedCatDb>,
    identity: DemoDeploymentManifest,
}

impl PreparedDemoRuntime {
    /// Query engine opened from the verified private copy.
    pub fn engine(&self) -> Arc<EmbeddedCatDb> {
        self.engine.clone()
    }

    /// Immutable deployment identity verified during startup.
    pub fn identity(&self) -> &DemoDeploymentManifest {
        &self.identity
    }

    /// Private SQLite copy used by the engine.
    pub fn database_path(&self) -> PathBuf {
        self.runtime_directory.path().join("catdb.sqlite")
    }
}

fn read_manifest(path: &Path) -> Result<DemoDeploymentManifest, DemoDeploymentError> {
    let bytes = read_bounded_regular_file(path, MAX_DEMO_MANIFEST_BYTES, "deployment manifest")?;
    serde_json::from_slice(&bytes).map_err(DemoDeploymentError::Serialization)
}

fn verify_manifest_shape(
    identity: &DemoDeploymentManifest,
    expected_source_commit: &str,
    deployed_source_commit: Option<&str>,
) -> Result<(), DemoDeploymentError> {
    validate_source_commit(expected_source_commit)?;
    if identity.format != DEMO_DEPLOYMENT_FORMAT {
        return Err(DemoDeploymentError::InvalidIdentity(
            "deployment manifest format mismatch".to_string(),
        ));
    }
    validate_source_commit(&identity.source_commit)?;
    if identity.source_commit != expected_source_commit {
        return Err(DemoDeploymentError::InvalidIdentity(
            "deployment source commit does not match the compiled runtime".to_string(),
        ));
    }
    if let Some(deployed) = deployed_source_commit {
        validate_source_commit(deployed)?;
        if deployed != expected_source_commit {
            return Err(DemoDeploymentError::InvalidIdentity(
                "platform deployment commit does not match the compiled runtime".to_string(),
            ));
        }
    }
    validate_digest(&identity.corpus_sha256, "sha256:", "corpus digest")?;
    validate_digest(
        &identity.logical_workspace_blake3,
        "blake3:",
        "logical workspace digest",
    )?;
    validate_digest(&identity.sqlite_sha256, "sha256:", "SQLite snapshot digest")?;
    validate_digest(
        &identity.deployment_fingerprint,
        "blake3:",
        "deployment fingerprint",
    )?;
    Ok(())
}

fn validate_runtime_root(root: &Path) -> Result<(), DemoDeploymentError> {
    let metadata =
        fs::symlink_metadata(root).map_err(|error| io("inspecting private runtime root", error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(DemoDeploymentError::InvalidIdentity(
            "runtime root must be a real directory".to_string(),
        ));
    }
    Ok(())
}

fn create_private_snapshot_copy(
    runtime_root: &Path,
    authority: &mut fs::File,
    expected_digest: &str,
) -> Result<tempfile::TempDir, DemoDeploymentError> {
    validate_runtime_root(runtime_root)?;
    let runtime_directory = tempfile::Builder::new()
        .prefix("catdb-demo-")
        .tempdir_in(runtime_root)
        .map_err(|error| io("creating private runtime directory", error))?;
    let destination = runtime_directory.path().join("catdb.sqlite");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options
        .open(&destination)
        .map_err(|error| io("creating private runtime snapshot", error))?;
    authority
        .rewind()
        .map_err(|error| io("rewinding retained SQLite authority for copy", error))?;
    std::io::copy(authority, &mut output)
        .map_err(|error| io("copying retained SQLite authority", error))?;
    output
        .flush()
        .map_err(|error| io("flushing private runtime snapshot", error))?;
    output
        .sync_all()
        .map_err(|error| io("syncing private runtime snapshot", error))?;

    if sha256_reader(authority)? != expected_digest {
        return Err(DemoDeploymentError::InvalidIdentity(
            "SQLite authority changed while it was copied".to_string(),
        ));
    }
    if sha256_file(&destination)? != expected_digest {
        return Err(DemoDeploymentError::InvalidIdentity(
            "private SQLite snapshot digest mismatch".to_string(),
        ));
    }
    Ok(runtime_directory)
}

/// Verify every immutable image identity before opening a private runtime copy.
pub async fn prepare_demo_runtime(
    database: &Path,
    corpus: &Path,
    manifest_path: &Path,
    runtime_root: &Path,
    expected_source_commit: &str,
    deployed_source_commit: Option<&str>,
) -> Result<PreparedDemoRuntime, DemoDeploymentError> {
    let identity = read_manifest(manifest_path)?;
    verify_manifest_shape(&identity, expected_source_commit, deployed_source_commit)?;
    require_closed_sqlite_snapshot(database)?;
    let files = corpus_files(corpus)?;
    if corpus_sha256(&files) != identity.corpus_sha256 {
        return Err(DemoDeploymentError::InvalidIdentity(
            "corpus digest mismatch".to_string(),
        ));
    }

    let expected_fingerprint = deployment_fingerprint(
        &identity.source_commit,
        identity.catalog_revision,
        &identity.corpus_sha256,
        &identity.logical_workspace_blake3,
        &identity.sqlite_sha256,
    );
    if expected_fingerprint != identity.deployment_fingerprint {
        return Err(DemoDeploymentError::InvalidIdentity(
            "deployment fingerprint mismatch".to_string(),
        ));
    }

    let mut retained_authority = open_regular_read_only(database)?;
    let (catalog_revision, logical_workspace_blake3) =
        inspect_demo_snapshot_reader(&mut retained_authority)?;
    if catalog_revision != identity.catalog_revision {
        return Err(DemoDeploymentError::InvalidIdentity(
            "authoritative catalog revision mismatch".to_string(),
        ));
    }
    if logical_workspace_blake3 != identity.logical_workspace_blake3 {
        return Err(DemoDeploymentError::InvalidIdentity(
            "logical workspace digest mismatch".to_string(),
        ));
    }
    if sha256_reader(&mut retained_authority)? != identity.sqlite_sha256 {
        return Err(DemoDeploymentError::InvalidIdentity(
            "SQLite snapshot digest mismatch".to_string(),
        ));
    }
    require_closed_sqlite_snapshot(database)?;

    let runtime_directory = create_private_snapshot_copy(
        runtime_root,
        &mut retained_authority,
        &identity.sqlite_sha256,
    )?;
    let runtime_database = runtime_directory.path().join("catdb.sqlite");
    let engine = EmbeddedCatDb::open(
        EngineConfig::new(&runtime_database, Arc::new(HashEmbedder::new()))
            .with_domain(crate::registration()),
    )?;
    if engine.pinned_catalog_revision().revision_id != identity.catalog_revision {
        return Err(DemoDeploymentError::InvalidIdentity(
            "runtime engine catalog revision mismatch".to_string(),
        ));
    }
    let runtime_logical = format!("blake3:{}", logical_workspace_fingerprint(&engine).await?);
    if runtime_logical != identity.logical_workspace_blake3 {
        return Err(DemoDeploymentError::InvalidIdentity(
            "runtime engine logical workspace digest mismatch".to_string(),
        ));
    }
    Ok(PreparedDemoRuntime {
        runtime_directory,
        engine: Arc::new(engine),
        identity,
    })
}

/// Open the built workspace, derive its authoritative identity, and write the
/// manifest used by runtime startup verification.
pub async fn prepare_demo_image(
    database: &Path,
    corpus: &Path,
    manifest_path: &Path,
    source_commit: &str,
) -> Result<DemoDeploymentManifest, DemoDeploymentError> {
    validate_source_commit(source_commit)?;
    require_closed_sqlite_snapshot(database)?;
    let files = corpus_files(corpus)?;
    let engine = EmbeddedCatDb::open(
        EngineConfig::new(database, Arc::new(HashEmbedder::new()))
            .with_domain(crate::registration()),
    )?;
    let engine_catalog_revision = engine.pinned_catalog_revision().revision_id;
    let engine_logical_workspace =
        format!("blake3:{}", logical_workspace_fingerprint(&engine).await?);
    drop(engine);
    require_closed_sqlite_snapshot(database)?;
    let (catalog_revision, logical_workspace_blake3) = inspect_demo_snapshot(database)?;
    if catalog_revision != engine_catalog_revision
        || logical_workspace_blake3 != engine_logical_workspace
    {
        return Err(DemoDeploymentError::Snapshot(
            "read-only snapshot identity disagrees with the opened engine".to_string(),
        ));
    }

    let corpus_sha256 = corpus_sha256(&files);
    let sqlite_sha256 = sha256_file(database)?;
    let deployment_fingerprint = deployment_fingerprint(
        source_commit,
        catalog_revision,
        &corpus_sha256,
        &logical_workspace_blake3,
        &sqlite_sha256,
    );
    let manifest = DemoDeploymentManifest {
        format: DEMO_DEPLOYMENT_FORMAT.to_string(),
        source_commit: source_commit.to_string(),
        catalog_revision,
        corpus_sha256,
        logical_workspace_blake3,
        sqlite_sha256,
        deployment_fingerprint,
    };

    let parent = manifest_path.parent().ok_or_else(|| {
        DemoDeploymentError::InvalidIdentity("manifest path has no parent directory".to_string())
    })?;
    fs::create_dir_all(parent)
        .map_err(|error| io(format!("creating {}", parent.display()), error))?;
    let temporary = parent.join(format!(".deployment-{}.tmp", Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    fs::write(&temporary, &bytes)
        .map_err(|error| io(format!("writing {}", temporary.display()), error))?;
    fs::rename(&temporary, manifest_path)
        .map_err(|error| io(format!("publishing {}", manifest_path.display()), error))?;
    Ok(manifest)
}

/// Compute the immutable workspace fingerprint.
///
/// `files` are sorted by source-relative path before hashing, so filesystem
/// traversal order cannot change the identity.
pub fn workspace_fingerprint(
    build_commit: &str,
    catalog_revision: &str,
    files: &[(String, Vec<u8>)],
) -> String {
    let mut ordered = files.to_vec();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"catdb-demo-workspace/v1\0");
    hash_part(&mut hasher, build_commit.as_bytes());
    hash_part(&mut hasher, catalog_revision.as_bytes());
    for (path, bytes) in ordered {
        hash_part(&mut hasher, path.as_bytes());
        hash_part(&mut hasher, &bytes);
    }
    hasher.finalize().to_hex().to_string()
}

fn hash_part(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value);
}

/// Hash the normalized, query-serving state of a built workspace.
///
/// Database bytes are recorded separately by the image build. This digest
/// deliberately excludes generated UUIDs, timestamps, source root paths, and
/// SQLite page layout so two independent imports of the same corpus can prove
/// that they produced the same document/section/chunk/link semantics.
pub async fn logical_workspace_fingerprint(engine: &EmbeddedCatDb) -> EngineResult<String> {
    let status = engine.status().await?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"catdb-demo-logical-workspace/v1\0");

    hash_part(&mut hasher, &status.schema_version.to_le_bytes());
    if let Some(index) = status.active_index {
        hash_part(&mut hasher, index.embedding_model.as_bytes());
        hash_part(&mut hasher, &index.dim.to_le_bytes());
        hash_part(&mut hasher, index.chunker_version.as_bytes());
        hash_part(&mut hasher, b"active");
    } else {
        hash_part(&mut hasher, b"no-active-index");
    }

    let mut sources = status.sources;
    sources.sort_by(|left, right| {
        left.include_globs
            .cmp(&right.include_globs)
            .then_with(|| left.exclude_globs.cmp(&right.exclude_globs))
            .then_with(|| left.max_file_size.cmp(&right.max_file_size))
    });
    for source in sources {
        hash_part(
            &mut hasher,
            &serde_json::to_vec(&(
                source.enabled,
                source.watch,
                source.include_globs,
                source.exclude_globs,
                source.max_file_size,
                source.respect_gitignore,
                source.embed_policy,
            ))
            .expect("source deployment state serializes"),
        );
    }

    let mut documents = engine.store().list_documents()?;
    documents.sort_by(|left, right| left.file_path.cmp(&right.file_path));
    let document_paths = documents
        .iter()
        .map(|document| (document.id, document.file_path.clone()))
        .collect::<HashMap<_, _>>();

    for document in documents {
        hash_part(&mut hasher, document.file_path.as_bytes());
        hash_part(
            &mut hasher,
            document.repo.as_deref().unwrap_or("").as_bytes(),
        );
        hash_part(&mut hasher, document.content_hash.as_bytes());

        let sections = engine.store().sections_for_document(document.id)?;
        let section_keys = sections
            .iter()
            .map(|section| {
                (
                    section.id,
                    format!(
                        "{}:{}-{}",
                        section.heading_path.join("\u{1f}"),
                        section.start_line,
                        section.end_line
                    ),
                )
            })
            .collect::<HashMap<_, _>>();
        for section in &sections {
            hash_part(
                &mut hasher,
                section_keys
                    .get(&section.id)
                    .expect("section key was built")
                    .as_bytes(),
            );
            hash_part(&mut hasher, section.content_hash.as_bytes());
            hash_part(
                &mut hasher,
                section
                    .parent_id
                    .and_then(|id| section_keys.get(&id))
                    .map_or(b"" as &[u8], |key| key.as_bytes()),
            );
        }

        let mut chunks = engine.store().chunks_for_document(document.id)?;
        chunks.sort_by(|left, right| {
            left.heading_path
                .cmp(&right.heading_path)
                .then_with(|| left.part.cmp(&right.part))
                .then_with(|| left.content_hash.cmp(&right.content_hash))
        });
        for chunk in chunks {
            hash_part(&mut hasher, chunk.heading_path.join("\u{1f}").as_bytes());
            hash_part(&mut hasher, &chunk.part.to_le_bytes());
            hash_part(&mut hasher, chunk.text.as_bytes());
            hash_part(&mut hasher, chunk.embedding_text.as_bytes());
            hash_part(&mut hasher, chunk.content_hash.as_bytes());
            hash_part(&mut hasher, &chunk.token_count.to_le_bytes());
        }

        let section_ids = sections
            .iter()
            .map(|section| section.id)
            .collect::<Vec<_>>();
        let mut links = engine.store().links_for_sections(&section_ids)?;
        links.sort_by(|left, right| {
            section_keys
                .get(&left.source_section)
                .cmp(&section_keys.get(&right.source_section))
                .then_with(|| left.raw_target.cmp(&right.raw_target))
                .then_with(|| left.kind.cmp(&right.kind))
        });
        for link in links {
            hash_part(
                &mut hasher,
                section_keys
                    .get(&link.source_section)
                    .expect("link source section belongs to document")
                    .as_bytes(),
            );
            hash_part(&mut hasher, link.raw_target.as_bytes());
            hash_part(&mut hasher, link.kind.as_bytes());
            hash_part(
                &mut hasher,
                link.resolved_document
                    .and_then(|id| document_paths.get(&id))
                    .map_or(b"" as &[u8], |path| path.as_bytes()),
            );
        }
    }

    Ok(hasher.finalize().to_hex().to_string())
}

fn sqlite_snapshot_error(error: rusqlite::Error) -> DemoDeploymentError {
    DemoDeploymentError::Snapshot(error.to_string())
}

fn checked_u32(value: i64, field: &str) -> Result<u32, DemoDeploymentError> {
    u32::try_from(value).map_err(|_| {
        DemoDeploymentError::Snapshot(format!("{field} is outside the supported u32 range"))
    })
}

fn query_count(
    connection: &Connection,
    table: &str,
    revision: &str,
) -> Result<i64, DemoDeploymentError> {
    connection
        .query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE revision_id=?1"),
            [revision],
            |row| row.get(0),
        )
        .map_err(sqlite_snapshot_error)
}

fn verify_catalog_seal(connection: &Connection, revision: &str) -> Result<(), DemoDeploymentError> {
    let sealed = connection
        .query_row(
            "SELECT schema_count, mapping_count, source_count, policy_count,
                    provenance_rule_count
               FROM catalog_revision_seals WHERE revision_id=?1",
            [revision],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_snapshot_error)?
        .ok_or_else(|| {
            DemoDeploymentError::Snapshot(
                "authoritative catalog head does not have a membership seal".to_string(),
            )
        })?;
    let actual = (
        query_count(connection, "catalog_revision_schema_versions", revision)?,
        query_count(connection, "catalog_revision_mapping_versions", revision)?,
        query_count(connection, "catalog_revision_source_versions", revision)?,
        query_count(connection, "catalog_revision_policy_versions", revision)?,
        query_count(
            connection,
            "catalog_revision_provenance_rule_versions",
            revision,
        )?,
    );
    if actual != sealed {
        return Err(DemoDeploymentError::Snapshot(
            "authoritative catalog membership does not match its immutable seal".to_string(),
        ));
    }
    Ok(())
}

/// Read the catalog head and normalized query-serving state without allowing
/// SQLite to migrate, create a journal, or modify the immutable image.
///
/// The returned revision comes from `catalog_head` inside the database, not
/// from deployment metadata stored beside it.
pub fn inspect_demo_snapshot(database: &Path) -> Result<(Uuid, String), DemoDeploymentError> {
    inspect_demo_snapshot_reader(&mut open_regular_read_only(database)?)
}

fn inspect_demo_snapshot_reader(
    retained_authority: &mut fs::File,
) -> Result<(Uuid, String), DemoDeploymentError> {
    let snapshot_bytes = retained_authority
        .metadata()
        .map_err(|error| io("inspecting retained SQLite authority", error))?
        .len();
    if snapshot_bytes == 0 || snapshot_bytes > MAX_DEMO_SNAPSHOT_BYTES {
        return Err(DemoDeploymentError::InvalidIdentity(format!(
            "SQLite snapshot size must be between 1 byte and {MAX_DEMO_SNAPSHOT_BYTES} bytes"
        )));
    }
    let snapshot_bytes = usize::try_from(snapshot_bytes).map_err(|_| {
        DemoDeploymentError::InvalidIdentity(
            "SQLite snapshot does not fit in the runtime address space".to_string(),
        )
    })?;
    retained_authority
        .rewind()
        .map_err(|error| io("rewinding retained SQLite authority", error))?;
    let mut snapshot = vec![0_u8; snapshot_bytes];
    retained_authority
        .read_exact(&mut snapshot)
        .map_err(|error| io("reading retained SQLite authority", error))?;
    if snapshot.get(..16) != Some(b"SQLite format 3\0") {
        return Err(DemoDeploymentError::Snapshot(
            "retained authority does not have a SQLite format 3 header".to_string(),
        ));
    }
    // A cleanly checkpointed SQLite file can retain WAL read/write version
    // bytes even when no WAL exists. Deserialized databases have no VFS from
    // which to open a WAL, so normalize only those two header bytes in this
    // private inspection copy. The retained authority is never modified and
    // its digest/copy continue to use the exact original bytes.
    if !matches!(snapshot[18], 1 | 2) || !matches!(snapshot[19], 1 | 2) {
        return Err(DemoDeploymentError::Snapshot(
            "retained authority has invalid SQLite journal version bytes".to_string(),
        ));
    }
    snapshot[18] = 1;
    snapshot[19] = 1;
    let mut connection = Connection::open_in_memory().map_err(sqlite_snapshot_error)?;
    connection
        .deserialize_read_exact(MAIN_DB, Cursor::new(snapshot), snapshot_bytes, true)
        .map_err(|error| {
            DemoDeploymentError::Snapshot(format!(
                "deserializing retained SQLite authority: {error}"
            ))
        })?;
    connection
        .execute_batch("PRAGMA query_only=ON;")
        .map_err(|error| {
            DemoDeploymentError::Snapshot(format!(
                "locking deserialized SQLite authority read-only: {error}"
            ))
        })?;
    let quick_check: String = connection
        .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
        .map_err(|error| {
            DemoDeploymentError::Snapshot(format!(
                "checking deserialized SQLite authority: {error}"
            ))
        })?;
    if quick_check != "ok" {
        return Err(DemoDeploymentError::Snapshot(
            "SQLite quick_check did not return ok".to_string(),
        ));
    }
    let schema_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(sqlite_snapshot_error)?;
    let catalog_revision_text: String = connection
        .query_row(
            "SELECT revision_id FROM catalog_head WHERE singleton=0",
            [],
            |row| row.get(0),
        )
        .map_err(sqlite_snapshot_error)?;
    let catalog_revision = Uuid::parse_str(&catalog_revision_text).map_err(|_| {
        DemoDeploymentError::Snapshot("authoritative catalog head is not a valid UUID".to_string())
    })?;
    verify_catalog_seal(&connection, &catalog_revision_text)?;

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"catdb-demo-logical-workspace/v1\0");
    hash_part(&mut hasher, &schema_version.to_le_bytes());

    let active_index = connection
        .query_row(
            "SELECT embedding_model, dim, chunker_version
               FROM index_versions WHERE state='active'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_snapshot_error)?;
    if let Some((embedding_model, dim, chunker_version)) = active_index {
        hash_part(&mut hasher, embedding_model.as_bytes());
        hash_part(
            &mut hasher,
            &checked_u32(dim, "index dimension")?.to_le_bytes(),
        );
        hash_part(&mut hasher, chunker_version.as_bytes());
        hash_part(&mut hasher, b"active");
    } else {
        hash_part(&mut hasher, b"no-active-index");
    }

    type SourceIdentity = (bool, bool, Vec<String>, Vec<String>, u64, bool, String);
    let mut source_statement = connection
        .prepare(
            "SELECT enabled, watch, include_globs, exclude_globs, max_file_size,
                    respect_gitignore, embed_policy
               FROM sources",
        )
        .map_err(sqlite_snapshot_error)?;
    let mut sources = source_statement
        .query_map([], |row| {
            let include: String = row.get(2)?;
            let exclude: String = row.get(3)?;
            Ok((
                row.get::<_, bool>(0)?,
                row.get::<_, bool>(1)?,
                include,
                exclude,
                row.get::<_, i64>(4)?,
                row.get::<_, bool>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(sqlite_snapshot_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sqlite_snapshot_error)?
        .into_iter()
        .map(
            |(enabled, watch, include, exclude, max_file_size, respect, policy)| {
                Ok::<SourceIdentity, DemoDeploymentError>((
                    enabled,
                    watch,
                    serde_json::from_str(&include)?,
                    serde_json::from_str(&exclude)?,
                    u64::try_from(max_file_size).map_err(|_| {
                        DemoDeploymentError::Snapshot(
                            "source maximum file size is negative".to_string(),
                        )
                    })?,
                    respect,
                    policy,
                ))
            },
        )
        .collect::<Result<Vec<_>, _>>()?;
    sources.sort_by(|left, right| {
        left.2
            .cmp(&right.2)
            .then_with(|| left.3.cmp(&right.3))
            .then_with(|| left.4.cmp(&right.4))
    });
    for source in sources {
        hash_part(
            &mut hasher,
            &serde_json::to_vec(&source).expect("normalized source deployment state serializes"),
        );
    }

    let mut document_statement = connection
        .prepare(
            "SELECT id, file_path, repo, content_hash
               FROM documents ORDER BY file_path, id",
        )
        .map_err(sqlite_snapshot_error)?;
    let documents = document_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(sqlite_snapshot_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sqlite_snapshot_error)?;
    let document_paths = documents
        .iter()
        .map(|(id, path, _, _)| (id.clone(), path.clone()))
        .collect::<HashMap<_, _>>();

    for (document_id, file_path, repo, content_hash) in documents {
        hash_part(&mut hasher, file_path.as_bytes());
        hash_part(&mut hasher, repo.as_deref().unwrap_or("").as_bytes());
        hash_part(&mut hasher, content_hash.as_bytes());

        let mut section_statement = connection
            .prepare(
                "SELECT id, heading_path, start_line, end_line, content_hash, parent_id
                   FROM sections WHERE document_id=?1 ORDER BY start_line, id",
            )
            .map_err(sqlite_snapshot_error)?;
        let sections = section_statement
            .query_map([&document_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .map_err(sqlite_snapshot_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_snapshot_error)?
            .into_iter()
            .map(|(id, heading, start, end, content_hash, parent)| {
                let heading: Vec<String> = serde_json::from_str(&heading)?;
                let start = checked_u32(start, "section start line")?;
                let end = checked_u32(end, "section end line")?;
                let key = format!("{}:{start}-{end}", heading.join("\u{1f}"));
                Ok::<_, DemoDeploymentError>((id, key, content_hash, parent))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let section_keys = sections
            .iter()
            .map(|(id, key, _, _)| (id.clone(), key.clone()))
            .collect::<HashMap<_, _>>();
        for (_, key, section_hash, parent) in &sections {
            hash_part(&mut hasher, key.as_bytes());
            hash_part(&mut hasher, section_hash.as_bytes());
            hash_part(
                &mut hasher,
                parent
                    .as_ref()
                    .and_then(|id| section_keys.get(id))
                    .map_or(b"" as &[u8], |key| key.as_bytes()),
            );
        }

        let mut chunk_statement = connection
            .prepare(
                "SELECT heading_path, part, text, embedding_text, content_hash, token_count
                   FROM chunks WHERE document_id=?1",
            )
            .map_err(sqlite_snapshot_error)?;
        let mut chunks = chunk_statement
            .query_map([&document_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(sqlite_snapshot_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_snapshot_error)?
            .into_iter()
            .map(
                |(heading, part, text, embedding, content_hash, token_count)| {
                    Ok::<_, DemoDeploymentError>((
                        serde_json::from_str::<Vec<String>>(&heading)?,
                        checked_u32(part, "chunk part")?,
                        text,
                        embedding,
                        content_hash,
                        checked_u32(token_count, "chunk token count")?,
                    ))
                },
            )
            .collect::<Result<Vec<_>, _>>()?;
        chunks.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.4.cmp(&right.4))
        });
        for (heading, part, text, embedding, chunk_hash, token_count) in chunks {
            hash_part(&mut hasher, heading.join("\u{1f}").as_bytes());
            hash_part(&mut hasher, &part.to_le_bytes());
            hash_part(&mut hasher, text.as_bytes());
            hash_part(&mut hasher, embedding.as_bytes());
            hash_part(&mut hasher, chunk_hash.as_bytes());
            hash_part(&mut hasher, &token_count.to_le_bytes());
        }

        let mut link_statement = connection
            .prepare(
                "SELECT l.source_section, l.raw_target, l.kind, l.resolved_document
                   FROM links l
                   JOIN sections s ON s.id=l.source_section
                  WHERE s.document_id=?1",
            )
            .map_err(sqlite_snapshot_error)?;
        let mut links = link_statement
            .query_map([&document_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(sqlite_snapshot_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_snapshot_error)?;
        links.sort_by(|left, right| {
            section_keys
                .get(&left.0)
                .cmp(&section_keys.get(&right.0))
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
        });
        for (source_section, raw_target, kind, resolved_document) in links {
            let source_key = section_keys.get(&source_section).ok_or_else(|| {
                DemoDeploymentError::Snapshot(
                    "link source section is outside its document".to_string(),
                )
            })?;
            hash_part(&mut hasher, source_key.as_bytes());
            hash_part(&mut hasher, raw_target.as_bytes());
            hash_part(&mut hasher, kind.as_bytes());
            hash_part(
                &mut hasher,
                resolved_document
                    .as_ref()
                    .and_then(|id| document_paths.get(id))
                    .map_or(b"" as &[u8], |path| path.as_bytes()),
            );
        }
    }

    Ok((
        catalog_revision,
        format!("blake3:{}", hasher.finalize().to_hex()),
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use catdb_embed::HashEmbedder;
    use catdb_engine::{EngineConfig, ImportOptions};
    use tempfile::tempdir;

    use super::*;

    fn files() -> Vec<(String, Vec<u8>)> {
        vec![
            ("notes/b.md".into(), b"# B\n".to_vec()),
            ("notes/a.md".into(), b"# A\n".to_vec()),
        ]
    }

    #[test]
    fn fingerprint_is_order_independent_but_binds_every_pinned_input() {
        let baseline = workspace_fingerprint("a".repeat(40).as_str(), "catalog-1", &files());
        let mut reversed = files();
        reversed.reverse();
        assert_eq!(
            baseline,
            workspace_fingerprint("a".repeat(40).as_str(), "catalog-1", &reversed)
        );

        let mut changed_bytes = files();
        changed_bytes[0].1.push(b'!');
        assert_ne!(
            baseline,
            workspace_fingerprint("a".repeat(40).as_str(), "catalog-1", &changed_bytes)
        );
        assert_ne!(
            baseline,
            workspace_fingerprint("a".repeat(40).as_str(), "catalog-2", &files())
        );
        assert_ne!(
            baseline,
            workspace_fingerprint("b".repeat(40).as_str(), "catalog-1", &files())
        );
    }

    #[tokio::test]
    async fn independent_builds_have_identical_logical_state_and_content_changes_break_it() {
        let temp = tempdir().unwrap();
        let corpus = temp.path().join("corpus");
        fs::create_dir_all(corpus.join("notes")).unwrap();
        fs::write(
            corpus.join("README.md"),
            "# Demo\n\nRead [the note](notes/provenance.md).\n",
        )
        .unwrap();
        fs::write(
            corpus.join("notes/provenance.md"),
            "# Provenance\n\nEvery result retains its source.\n",
        )
        .unwrap();

        let build = |name: &str| {
            EmbeddedCatDb::open(
                EngineConfig::new(temp.path().join(name), Arc::new(HashEmbedder::new()))
                    .with_domain(crate::registration()),
            )
            .unwrap()
        };
        let first = build("first.sqlite");
        first
            .import(&corpus, ImportOptions::default())
            .await
            .unwrap();
        let second = build("second.sqlite");
        second
            .import(&corpus, ImportOptions::default())
            .await
            .unwrap();

        let first_fingerprint = logical_workspace_fingerprint(&first).await.unwrap();
        assert_eq!(
            first_fingerprint,
            logical_workspace_fingerprint(&second).await.unwrap()
        );

        fs::write(
            corpus.join("notes/provenance.md"),
            "# Provenance\n\nA materially changed result retains its source.\n",
        )
        .unwrap();
        let changed = build("changed.sqlite");
        changed
            .import(&corpus, ImportOptions::default())
            .await
            .unwrap();
        assert_ne!(
            first_fingerprint,
            logical_workspace_fingerprint(&changed).await.unwrap()
        );
    }

    #[tokio::test]
    async fn snapshot_inspection_uses_the_retained_descriptor_after_path_replacement() {
        let temp = tempdir().unwrap();
        let corpus = temp.path().join("corpus");
        fs::create_dir_all(&corpus).unwrap();
        fs::write(corpus.join("README.md"), "# Retained authority\n").unwrap();
        let database = temp.path().join("catdb.sqlite");
        let engine = EmbeddedCatDb::open(
            EngineConfig::new(&database, Arc::new(HashEmbedder::new()))
                .with_domain(crate::registration()),
        )
        .unwrap();
        engine
            .import(&corpus, ImportOptions::default())
            .await
            .unwrap();
        drop(engine);

        let expected = inspect_demo_snapshot(&database).unwrap();
        let mut retained = open_regular_read_only(&database).unwrap();
        fs::rename(&database, temp.path().join("retained.sqlite")).unwrap();
        fs::write(&database, b"not a SQLite database").unwrap();

        assert_eq!(
            inspect_demo_snapshot_reader(&mut retained).unwrap(),
            expected
        );
    }
}
