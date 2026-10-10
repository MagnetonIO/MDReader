//! Immutable mixed-profile device copies. Never carries or recreates old grants.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use mdreader_core::{
    device::{self, assets::AssetRuntime},
    ReaderStore, SourceRoot,
};
use mdreader_legacy_export::{self as legacy, Input, Role};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
mod captures;
mod drafts;
pub(super) mod history;
mod owned;
mod provider_cache;
mod providers;

const MAX_BYTES: usize = 64 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    version: u32,
    pub documents: Vec<Document>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Document {
    pub id: String,
    pub source_id: String,
    pub name: String,
    pub revision: String,
    pub markdown: String,
    pub pinned: bool,
    pub line: u32,
    assets: Vec<Asset>,
    #[serde(default)]
    lineage: Option<Lineage>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Lineage {
    kind: String,
    source_id: String,
    path: String,
    revision: String,
    document_id: Option<String>,
    operation_id: Option<String>,
    state: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Asset {
    target: String,
    bytes: u32,
    base64: String,
}
pub(super) fn capture(root: &SourceRoot, output: &SourceRoot) -> Result<Option<Input>> {
    if !root
        .path
        .join("reader-v1/library.sqlite")
        .try_exists()
        .map_err(|_| Error::Storage)?
    {
        return Ok(None);
    }
    let mut recovered =
        super::closed_database::Snapshot::capture(root, Path::new("reader-v1/library.sqlite"))?;
    let mut owner = recovered.store()?;
    let runtime = AssetRuntime::default();
    let assets = runtime
        .existing_assets(&mut owner, &root.path.join("reader-v1/device-assets-v1"))
        .map_err(|_| Error::Integrity)?;
    let mut documents = Vec::new();
    let mut total = 0usize;
    for observed in device::catalog::archive_metadata(&owner).map_err(|_| Error::Integrity)? {
        let document = observed.document;
        super::conversion::owned_document(&mut owner, &root.path, &document)?;
        let source = owned::source_root(
            &mut owner,
            &document.source_id,
            &root
                .path
                .join("reader-v1/device-markdown-v1")
                .join(&document.id),
        )?;
        let bytes = owned::read(&source, Path::new("document.md"), 1024 * 1024)?;
        if bytes.len() != document.bytes as usize
            || blake3::hash(&bytes).to_hex().as_str() != document.revision
        {
            return Err(Error::Conflict);
        }
        let markdown = String::from_utf8(bytes).map_err(|_| Error::Integrity)?;
        let mut copied = Vec::new();
        if let Some(assets) = &assets {
            let archived =
                device::publication::prepare_archive(&mut owner, &document.id, assets, &runtime)
                    .and_then(|plan| plan.read())
                    .map_err(|_| Error::Integrity)?;
            if archived.markdown() != markdown.as_bytes() {
                return Err(Error::Conflict);
            }
            for asset in archived.assets().iter().filter(|a| a.state == "ready") {
                let bytes = asset.bytes().ok_or(Error::Integrity)?;
                if bytes.len() != asset.expected_bytes as usize
                    || asset.received != asset.expected_bytes
                {
                    return Err(Error::Integrity);
                }
                let encoded = STANDARD.encode(bytes);
                total = total.checked_add(encoded.len()).ok_or(Error::Bounds)?;
                if total > MAX_BYTES {
                    return Err(Error::Bounds);
                }
                copied.push(Asset {
                    target: asset.target.clone(),
                    bytes: asset.expected_bytes,
                    base64: encoded,
                });
            }
            archived
                .revalidate(&mut owner)
                .map_err(|_| Error::Conflict)?;
        }
        total = total.checked_add(markdown.len()).ok_or(Error::Bounds)?;
        if total > MAX_BYTES {
            return Err(Error::Bounds);
        }
        let lineage = Lineage {
            kind: "device".into(),
            source_id: document.source_id.clone(),
            path: "document.md".into(),
            revision: document.revision.clone(),
            document_id: Some(document.id.clone()),
            operation_id: None,
            state: Some(
                if observed.retired {
                    "retired"
                } else if !observed.enabled {
                    "disabled"
                } else {
                    "active"
                }
                .into(),
            ),
        };
        documents.push(Document {
            id: document.id,
            source_id: document.source_id,
            name: owned::name(&if observed.retired || !observed.enabled {
                format!("Recovered {}", document.name)
            } else {
                document.name
            }),
            revision: document.revision,
            markdown,
            pinned: document.pinned,
            line: document.line,
            assets: copied,
            lineage: Some(lineage),
        });
    }
    captures::capture(&mut owner, root, &mut documents)?;
    providers::capture(&mut owner, root, &mut documents)?;
    provider_cache::capture(&mut owner, &mut documents)?;
    drafts::capture(&mut owner, &mut documents)?;
    history::positions(root, &mut documents)?;
    recovered.verify()?;
    let bytes = serde_json::to_vec(&Snapshot {
        version: 2,
        documents,
    })
    .map_err(|_| Error::Storage)?;
    if bytes.len() > MAX_BYTES {
        return Err(Error::Bounds);
    }
    let name = Path::new("native-device-copies.json");
    let mut file = output
        .create_new_relative(name)
        .map_err(|_| Error::Storage)?;
    file.write_all(&bytes).map_err(|_| Error::Storage)?;
    file.sync_all().map_err(|_| Error::Storage)?;
    Ok(Some(Input {
        role: Role::ReaderDocuments,
        path: output.path.join(name),
    }))
}
pub(super) fn read(archive: &Path) -> Result<Option<(usize, Snapshot)>> {
    let manifest =
        legacy::verify(archive, &Default::default()).map_err(super::archive_import::failure)?;
    let mut native = manifest
        .artifacts
        .iter()
        .enumerate()
        .filter(|(_, a)| a.role == Role::ReaderDocuments);
    let Some((index, artifact)) = native.next() else {
        return Ok(None);
    };
    if native.next().is_some() {
        return Err(Error::Integrity);
    }
    let member = manifest
        .members
        .iter()
        .find(|m| m.name == artifact.raw)
        .ok_or(Error::Integrity)?;
    if member.bytes > MAX_BYTES as u64 {
        return Err(Error::Bounds);
    }
    let root = SourceRoot::capture(archive.to_owned()).map_err(|_| Error::Conflict)?;
    let mut file = root
        .open_optional_file_relative(Path::new(&artifact.raw))
        .map_err(|_| Error::Conflict)?
        .ok_or(Error::Integrity)?;
    let stamp = mdreader_core::source_root::file_stamp::single_link_stamp(&file)
        .map_err(|_| Error::Conflict)?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Storage)?;
    if bytes.len() as u64 != member.bytes
        || format!("{:x}", Sha256::digest(&bytes)) != member.sha256
        || stamp
            != mdreader_core::source_root::file_stamp::single_link_stamp(&file)
                .map_err(|_| Error::Conflict)?
    {
        return Err(Error::Integrity);
    }
    let snapshot: Snapshot = serde_json::from_slice(&bytes).map_err(|_| Error::Integrity)?;
    if !matches!(snapshot.version, 1 | 2) || snapshot.documents.len() > 1000 {
        return Err(Error::Unsupported);
    }
    let mut ids = std::collections::HashSet::new();
    for document in &snapshot.documents {
        if !uuid::Uuid::parse_str(&document.id)
            .is_ok_and(|id| matches!(id.get_version_num(), 4 | 5))
            || !ids.insert(&document.id)
            || document.markdown.len() > 1024 * 1024
            || blake3::hash(document.markdown.as_bytes()).to_hex().as_str() != document.revision
            || document.name.is_empty()
            || document.name.len() > 224
            || document.line == 0
            || document.line as usize > document.markdown.lines().count().max(1)
            || document.assets.len() > 64
            || document.lineage.as_ref().is_some_and(|l| {
                l.kind.len() > 64
                    || l.source_id.len() > 256
                    || l.path.len() > 4096
                    || l.revision.len() != 64
                    || l.operation_id.as_ref().is_some_and(|id| id.len() > 64)
            })
        {
            return Err(Error::Integrity);
        }
    }
    let named = root
        .open_optional_file_relative(Path::new(&artifact.raw))
        .map_err(|_| Error::Conflict)?
        .ok_or(Error::Conflict)?;
    if stamp
        != mdreader_core::source_root::file_stamp::single_link_stamp(&named)
            .map_err(|_| Error::Conflict)?
    {
        return Err(Error::Conflict);
    }
    root.verify().map_err(|_| Error::Conflict)?;
    Ok(Some((index, snapshot)))
}
pub(super) fn restore(
    owner: &mut ReaderStore,
    candidate: &Path,
    document: &Document,
) -> Result<device::DeviceDocument> {
    let copied = device::import(
        owner,
        &candidate.join("reader-v1/device-markdown-v1"),
        document.name.clone(),
        document.markdown.clone(),
    )
    .map_err(|_| Error::Storage)?;
    if copied.revision != document.revision {
        return Err(Error::Integrity);
    }
    let runtime = AssetRuntime::default();
    let assets = runtime
        .assets(owner, &candidate.join("reader-v1/device-assets-v1"))
        .map_err(|_| Error::Storage)?;
    for asset in &document.assets {
        if asset.base64.len() > 5_592_408 {
            return Err(Error::Bounds);
        }
        let bytes = STANDARD
            .decode(&asset.base64)
            .map_err(|_| Error::Integrity)?;
        if bytes.len() != asset.bytes as usize {
            return Err(Error::Integrity);
        }
        let manifest = assets
            .manifest(owner, &copied.id, &copied.revision)
            .map_err(|_| Error::Integrity)?;
        let id = assets
            .begin(
                owner,
                &copied.id,
                &copied.revision,
                &asset.target,
                asset.bytes,
                &manifest.manifest,
            )
            .map_err(|_| Error::Integrity)?;
        for (index, chunk) in bytes.chunks(65536).enumerate() {
            assets
                .write(owner, &id, (index * 65536) as u32, &STANDARD.encode(chunk))
                .map_err(|_| Error::Storage)?;
        }
        let permit = runtime.validation_permit().map_err(|_| Error::Storage)?;
        let validated = assets
            .prepare_commit(owner, &id, permit)
            .and_then(|prepared| prepared.validate())
            .map_err(|_| Error::Integrity)?;
        assets
            .publish(owner, validated)
            .map_err(|_| Error::Integrity)?;
    }
    device::pin(owner, &copied.id, document.pinned).map_err(|_| Error::Storage)?;
    device::position(owner, &copied.id, &copied.revision, document.line)
        .map_err(|_| Error::Integrity)?;
    Ok(copied)
}
