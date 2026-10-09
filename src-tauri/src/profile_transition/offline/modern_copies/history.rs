//! Exact closed-source canonical receipts may map to selected recovered bytes.
//! Legacy block/pixel anchors and tab pins remain archival, never source grants.
use super::*;
fn selected(raw: &serde_json::Value) -> &serde_json::Value {
    raw.get("entries")
        .and_then(|v| v.as_array())
        .and_then(|entries| {
            raw.get("index")
                .and_then(|v| v.as_u64())
                .and_then(|i| entries.get(i as usize))
        })
        .unwrap_or(raw)
}
fn matches(lineage: &Lineage, value: &serde_json::Value) -> bool {
    if !matches!(lineage.kind.as_str(), "device" | "capture" | "provider") {
        return false;
    }
    let Some(reference) = value.get("reference") else {
        return false;
    };
    reference.get("sourceId").and_then(|v| v.as_str()) == Some(&lineage.source_id)
        && reference.get("revision").and_then(|v| v.as_str()) == Some(&lineage.revision)
        && if lineage.kind == "device" {
            reference.get("documentId").and_then(|v| v.as_str()) == lineage.document_id.as_deref()
        } else {
            reference.get("path").and_then(|v| v.as_str()) == Some(&lineage.path)
        }
}
pub(super) fn positions(profile: &SourceRoot, documents: &mut [Document]) -> Result<()> {
    if !profile
        .path
        .join("session.json")
        .try_exists()
        .map_err(|_| Error::Storage)?
    {
        return Ok(());
    }
    let bytes = owned::read(profile, Path::new("session.json"), 16 * 1024 * 1024)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| Error::Integrity)?;
    let session = value.get("session").unwrap_or(&value);
    let Some(history) = session.get("readerHistory") else {
        return Ok(());
    };
    let mut entries = Vec::new();
    if let Some(recent) = history.get("recent").and_then(|v| v.as_array()) {
        entries.extend(recent.iter());
    }
    if let Some(tabs) = history.get("tabs").and_then(|v| v.as_array()) {
        entries.extend(tabs.iter().map(selected));
    }
    if entries.len() > 1000 {
        return Err(Error::Bounds);
    }
    for document in documents {
        let Some(lineage) = &document.lineage else {
            continue;
        };
        // Native bookmark state on device documents already has its durable
        // meaning. Only provider/capture canonical line positions need mapping.
        if lineage.kind == "device" {
            continue;
        }
        for entry in &entries {
            if !matches(lineage, entry) {
                continue;
            }
            let Some(position) = entry
                .get("position")
                .filter(|p| p.get("revision").and_then(|v| v.as_str()) == Some(&lineage.revision))
            else {
                continue;
            };
            let Some(line) = position.get("line").and_then(|v| v.as_u64()) else {
                continue;
            };
            if line == 0 || line > document.markdown.lines().count().max(1) as u64 {
                return Err(Error::Integrity);
            }
            document.line = line as u32;
            break;
        }
    }
    Ok(())
}
pub(in crate::profile_transition::offline) fn bind(
    owner: &mut ReaderStore,
    archive: &Path,
    identity: &legacy::reviews::ArchiveIdentity,
    copies: &[(&Document, device::DeviceDocument)],
) -> Result<()> {
    // Verify/load the complete archive once per preparation, never once per
    // recovered copy. The enclosing preparation retains its final archive fence.
    let entries = legacy::history::entries(archive, identity, &Default::default())
        .map_err(super::super::archive_import::failure)?;
    for (document, copied) in copies {
        let Some(lineage) = &document.lineage else {
            continue;
        };
        for entry in &entries {
            if !matches(lineage, selected(&entry.raw)) {
                continue;
            }
            let current =
                device::catalog::document(owner, &copied.id).map_err(|_| Error::Conflict)?;
            if device::catalog::receipt(&current) != device::catalog::receipt(copied) {
                return Err(Error::Conflict);
            }
            let bindings = device::archive::history::bindings(
                owner,
                &identity.export_id.to_string(),
                &identity.manifest_sha256,
            )
            .map_err(|_| Error::Integrity)?;
            let digest =
                device::archive::history::digest(&bindings).map_err(|_| Error::Integrity)?;
            device::archive::history::bind(
                owner,
                &identity.export_id.to_string(),
                &identity.manifest_sha256,
                &entry.entry_id,
                &digest,
                &device::archive::history::Reference {
                    document_id: Some(copied.id.clone()),
                    source_id: copied.source_id.clone(),
                    path: "document.md".into(),
                    revision: copied.revision.clone(),
                },
            )
            .map_err(|_| Error::Conflict)?;
        }
    }
    Ok(())
}
