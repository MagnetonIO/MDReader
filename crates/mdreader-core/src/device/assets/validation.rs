use super::*;

pub struct PreparedAsset {
    pub(super) _permit: ValidationPermit,
    pub(super) bytes: Vec<u8>,
    pub(super) item: Item,
    pub(super) root: SourceRoot,
    pub(super) owner: Arc<()>,
}
pub struct ValidatedAsset {
    pub(super) _permit: ValidationPermit,
    pub(super) item: Item,
    pub(super) root: SourceRoot,
    pub(super) owner: Arc<()>,
    pub(super) hash: String,
    pub(super) mime: String,
    pub(super) width: u32,
    pub(super) height: u32,
}
impl PreparedAsset {
    /// Full PNG/JPEG decode is mandatory. Dimensions/output bytes are strict;
    /// the image crate's decoder allocation budget excludes allocator overhead.
    pub fn validate(self) -> Result<ValidatedAsset> {
        let format = image::guess_format(&self.bytes).map_err(|_| Error::InvalidImage)?;
        let mime = match format {
            image::ImageFormat::Png => "image/png",
            image::ImageFormat::Jpeg => "image/jpeg",
            _ => return Err(Error::UnsupportedImage),
        };
        // Reject APNG before a browser can interpret multiple unvalidated frames.
        if format == image::ImageFormat::Png {
            let mut offset = 8usize;
            while offset + 12 <= self.bytes.len() {
                let length = u32::from_be_bytes(
                    self.bytes[offset..offset + 4]
                        .try_into()
                        .map_err(|_| unavailable())?,
                ) as usize;
                let end = offset
                    .checked_add(12)
                    .and_then(|start| start.checked_add(length))
                    .filter(|end| *end <= self.bytes.len())
                    .ok_or_else(unavailable)?;
                if &self.bytes[offset + 4..offset + 8] == b"acTL" {
                    return Err(Error::AnimatedPng);
                }
                offset = end;
            }
        }
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(24 * 1024 * 1024);
        let mut reader = image::ImageReader::with_format(std::io::Cursor::new(&self.bytes), format);
        reader.limits(limits.clone());
        let (width, height) = reader.into_dimensions().map_err(|_| unavailable())?;
        if width == 0
            || height == 0
            || width > 4096
            || height > 4096
            || u64::from(width) * u64::from(height) > 4_000_000
        {
            return Err(Error::ImageBounds);
        }
        let mut reader = image::ImageReader::with_format(std::io::Cursor::new(&self.bytes), format);
        reader.limits(limits);
        let decoded = reader.decode().map_err(|_| Error::DecodeFailed)?;
        if decoded.width() != width
            || decoded.height() != height
            || decoded.as_bytes().len() > 16_000_000
        {
            return Err(unavailable());
        }
        self.root.verify().map_err(|_| unavailable())?;
        Ok(ValidatedAsset {
            _permit: self._permit,
            item: self.item,
            root: self.root,
            owner: self.owner,
            hash: digest(&self.bytes),
            mime: mime.into(),
            width,
            height,
        })
    }
}
