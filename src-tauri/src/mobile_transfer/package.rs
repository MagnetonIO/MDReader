//! Preserve exact Markdown bytes and selected ready image paths. No fetching,
//! rewriting, SQL schema initialization or attachment inference.
use super::{snapshot, staging, types::Reference};
use crate::{mobile_assets::AssetRuntime, reader::ReaderBackend};
use std::{io::Write, path::Path};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

pub(super) struct PreparedCopy {
    pub path: String,
    pub name: String,
    pub mime: String,
    pub bytes: u32,
    pub sha256: String,
}
pub(super) fn prepare(
    backend: &ReaderBackend,
    runtime: &AssetRuntime,
    profile: &Path,
    cache: &Path,
    reference: &Reference,
) -> Result<PreparedCopy, String> {
    let documents = profile.join("device-markdown-v1");
    let images = profile.join("device-assets-v1");
    let snapshot = snapshot::prepare(backend, runtime, &documents, &images, reference)?;
    let assets = snapshot
        .manifest
        .as_ref()
        .map(|m| m.assets.as_slice())
        .unwrap_or_default();
    let mut names = std::collections::HashSet::new();
    for asset in assets {
        snapshot::target(&asset.target)?;
        let key = asset.target.to_lowercase();
        if !names.insert(key.clone())
            || assets.iter().any(|other| {
                other
                    .target
                    .to_lowercase()
                    .starts_with(&(key.clone() + "/"))
            })
        {
            return Err("unavailable".into());
        }
    }
    let stem: String = snapshot
        .name
        .chars()
        .scan(0, |bytes, c| {
            *bytes += c.len_utf8();
            (*bytes <= 100).then_some(c)
        })
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let stem = stem.trim().trim_end_matches('.').trim_end_matches(".md");
    let stem = if stem.is_empty() { "document" } else { stem };
    let name = format!("{stem}.{}", if assets.is_empty() { "md" } else { "zip" });
    let mut copy = staging::create(cache, &name)?;
    if assets.is_empty() {
        copy.file
            .write_all(snapshot.markdown.as_bytes())
            .map_err(|_| "transfer_unconfirmed")?;
    } else {
        let mut markdown_name = format!("{stem}.md");
        while names.iter().any(|key| {
            *key == markdown_name.to_lowercase()
                || key.starts_with(&(markdown_name.to_lowercase() + "/"))
        }) {
            markdown_name = format!("document-{}.md", uuid::Uuid::new_v4());
        }
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o600);
        let mut zip = ZipWriter::new(&mut copy.file);
        zip.start_file(markdown_name, options)
            .map_err(|_| "transfer_unconfirmed")?;
        zip.write_all(snapshot.markdown.as_bytes())
            .map_err(|_| "transfer_unconfirmed")?;
        for asset in assets {
            zip.start_file(&asset.target, options)
                .map_err(|_| "transfer_unconfirmed")?;
            snapshot::copy_asset(
                backend,
                runtime,
                &snapshot,
                reference,
                &asset.asset_id,
                &mut zip,
            )?;
        }
        zip.finish().map_err(|_| "transfer_unconfirmed")?;
    }
    snapshot::revalidate(backend, runtime, &documents, &images, reference, &snapshot)?;
    let (path, bytes, sha256) = copy.finish()?;
    Ok(PreparedCopy {
        path: path.to_str().ok_or("unavailable")?.into(),
        name,
        mime: if assets.is_empty() {
            "text/markdown"
        } else {
            "application/zip"
        }
        .into(),
        bytes,
        sha256,
    })
}
