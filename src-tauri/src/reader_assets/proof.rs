//! Bounded no-follow file observations shared by catalog and workspace assets.
use super::{
    path, AssetContent, AssetFailure as F, AssetPreview, AssetReadError, AssetReferenceKind,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use mdreader_core::{
    source_root::file_stamp::{single_link_stamp, SourceFileStamp},
    SourceRoot,
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

const TEXT_BYTES: u64 = 1024 * 1024;
const RASTER_BYTES: u64 = 4 * 1024 * 1024;
static READING: AtomicUsize = AtomicUsize::new(0);
pub(crate) struct Permit;
impl Permit {
    pub(crate) fn acquire() -> Result<Self, AssetReadError> {
        READING
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < 2).then_some(count + 1)
            })
            .map_err(|_| F::Busy.error())?;
        Ok(Self)
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        READING.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) struct BoundedFile {
    file: File,
    stamp: SourceFileStamp,
    path: String,
    pub(crate) bytes: Vec<u8>,
}
impl BoundedFile {
    pub(crate) fn read(
        root: &SourceRoot,
        path: &str,
        maximum: u64,
    ) -> Result<Self, AssetReadError> {
        path::origin(path)?;
        root.verify().map_err(|_| F::Stale.error())?;
        let mut file = open(root, path)?;
        let stamp = single_link_stamp(&file).map_err(|_| F::Unauthorized.error())?;
        let length = file.metadata().map_err(|_| F::Unavailable.error())?.len();
        if length > maximum {
            return Err(F::TooLarge.error());
        }
        let mut bytes = Vec::with_capacity(length as usize + 1);
        Read::by_ref(&mut file)
            .take(maximum + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| F::Unavailable.error())?;
        if bytes.len() as u64 != length {
            return Err(F::Stale.error());
        }
        let observation = Self {
            file,
            stamp,
            path: path.into(),
            bytes,
        };
        observation.revalidate(root)?;
        Ok(observation)
    }
    pub(crate) fn revalidate(&self, root: &SourceRoot) -> Result<(), AssetReadError> {
        root.verify().map_err(|_| F::Stale.error())?;
        if single_link_stamp(&self.file).map_err(|_| F::Stale.error())? != self.stamp
            || single_link_stamp(&open(root, &self.path)?).map_err(|_| F::Stale.error())?
                != self.stamp
        {
            return Err(F::Stale.error());
        }
        Ok(())
    }
    pub(crate) fn revision(&self) -> String {
        format!("{:x}", Sha256::digest(&self.bytes))
    }
}
fn open(root: &SourceRoot, path: &str) -> Result<File, AssetReadError> {
    root.open_optional_file_relative(Path::new(path))
        .map_err(|_| F::Unauthorized.error())?
        .ok_or_else(|| F::Unavailable.error())
}
fn text_format(path: &str) -> Option<&'static str> {
    match Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase()
        .as_str()
    {
        "bicep" => Some("bicep"),
        "yml" | "yaml" => Some("yaml"),
        "sql" => Some("sql"),
        "drawio" => Some("xml"),
        "txt" => Some("text"),
        "json" => Some("json"),
        "toml" => Some("toml"),
        "config" | "conf" => Some("config"),
        _ => None,
    }
}
pub(crate) struct AssetObservation {
    file: BoundedFile,
    pub(crate) preview: AssetPreview,
}
pub(super) fn referenced(
    markdown: &str,
    origin: &str,
    target: &path::Target,
    kind: AssetReferenceKind,
) -> bool {
    let ast_kind = match kind {
        AssetReferenceKind::Image => "image",
        AssetReferenceKind::Attachment => "markdown",
    };
    mdreader_core::parse_markdown(markdown, origin)
        .sections
        .iter()
        .flat_map(|section| &section.links)
        .any(|link| {
            link.kind == ast_kind
                && path::resolve(origin, &link.raw_target)
                    .is_ok_and(|reference| reference == *target)
        })
}
impl AssetObservation {
    pub(crate) fn read(
        root: &SourceRoot,
        origin: &str,
        markdown: &str,
        raw: &str,
        kind: AssetReferenceKind,
    ) -> Result<Self, AssetReadError> {
        let target = path::resolve(origin, raw)?;
        if matches!(kind, AssetReferenceKind::Image) && target.fragment.is_some() {
            return Err(F::Invalid.error());
        }
        if !referenced(markdown, origin, &target, kind) {
            return Err(F::Unauthorized.error());
        }
        let lower = target.path.to_ascii_lowercase();
        let raster = lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg");
        let format = (!matches!(kind, AssetReferenceKind::Image))
            .then(|| text_format(&target.path))
            .flatten();
        if !raster && format.is_none() {
            return Err(F::Unsupported.error());
        }
        let file = BoundedFile::read(
            root,
            &target.path,
            if raster { RASTER_BYTES } else { TEXT_BYTES },
        )?;
        let content = if raster {
            let image = mdreader_core::device::assets::validate_raster(&file.bytes)
                .map_err(|_| F::Unsupported.error())?;
            AssetContent::Raster {
                mime: image.mime,
                width: image.width,
                height: image.height,
                base64: STANDARD.encode(&file.bytes),
            }
        } else {
            AssetContent::Text {
                mime: "text/plain".into(),
                format: format.ok_or_else(|| F::Unsupported.error())?.into(),
                text: std::str::from_utf8(&file.bytes)
                    .map_err(|_| F::Unsupported.error())?
                    .into(),
            }
        };
        let preview = AssetPreview {
            path: target.path,
            revision: file.revision(),
            fragment: target.fragment,
            content,
        };
        file.revalidate(root)?;
        Ok(Self { file, preview })
    }
    pub(crate) fn revalidate(&self, root: &SourceRoot) -> Result<(), AssetReadError> {
        self.file.revalidate(root)
    }
}
