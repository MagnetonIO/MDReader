//! Bounded PNG/JPEG structural admission matching the browser's selected asset subset.
//! This is not a pixel decoder. Rendering must still use the native bounded decoder.
use crate::{Result, SyncError};
fn require(value: bool) -> Result<()> {
    if value {
        Ok(())
    } else {
        Err(SyncError::Invalid)
    }
}
fn be32(bytes: &[u8], offset: usize) -> Result<u32> {
    let bytes = bytes.get(offset..offset + 4).ok_or(SyncError::Invalid)?;
    Ok(u32::from_be_bytes(
        bytes.try_into().map_err(|_| SyncError::Invalid)?,
    ))
}
fn be16(bytes: &[u8], offset: usize) -> Result<u16> {
    let bytes = bytes.get(offset..offset + 2).ok_or(SyncError::Invalid)?;
    Ok(u16::from_be_bytes(
        bytes.try_into().map_err(|_| SyncError::Invalid)?,
    ))
}
pub(super) fn validate(bytes: &[u8], mime: &str) -> Result<()> {
    require(!bytes.is_empty() && bytes.len() <= 4 * 1024 * 1024)?;
    let (width, height) = match mime {
        "image/png" => png(bytes)?,
        "image/jpeg" => jpeg(bytes)?,
        _ => return Err(SyncError::Invalid),
    };
    require(
        width > 0
            && height > 0
            && width <= 4096
            && height <= 4096
            && u64::from(width) * u64::from(height) <= 4 * 1024 * 1024,
    )
}
fn png(bytes: &[u8]) -> Result<(u32, u32)> {
    require(
        bytes.len() >= 45
            && bytes.starts_with(b"\x89PNG\r\n\x1a\n")
            && be32(bytes, 8)? == 13
            && bytes.get(12..16) == Some(b"IHDR"),
    )?;
    let dimensions = (be32(bytes, 16)?, be32(bytes, 20)?);
    let (mut offset, mut count, mut idat) = (8usize, 0, false);
    while offset + 12 <= bytes.len() {
        count += 1;
        require(count <= 4096)?;
        let length = be32(bytes, offset)? as usize;
        let kind = &bytes[offset + 4..offset + 8];
        let end = offset
            .checked_add(12)
            .and_then(|n| n.checked_add(length))
            .ok_or(SyncError::Invalid)?;
        require(
            end <= bytes.len()
                && ![b"acTL", b"fcTL", b"fdAT", b"zTXt", b"iTXt", b"iCCP"]
                    .iter()
                    .any(|tag| tag.as_slice() == kind)
                && (kind != b"IHDR" || offset == 8),
        )?;
        idat |= kind == b"IDAT";
        offset = end;
        if kind == b"IEND" {
            require(length == 0 && offset == bytes.len() && idat)?;
            return Ok(dimensions);
        }
    }
    Err(SyncError::Invalid)
}
fn jpeg(bytes: &[u8]) -> Result<(u32, u32)> {
    require(bytes.len() >= 12 && bytes.starts_with(&[255, 216]) && bytes.ends_with(&[255, 217]))?;
    let (mut offset, mut count, mut dimensions) = (2usize, 0, None);
    while offset + 4 <= bytes.len() {
        count += 1;
        require(count <= 4096 && bytes[offset] == 255)?;
        while bytes.get(offset) == Some(&255) {
            offset += 1;
        }
        let marker = *bytes.get(offset).ok_or(SyncError::Invalid)?;
        offset += 1;
        let length = usize::from(be16(bytes, offset)?);
        require(length >= 2 && offset + length <= bytes.len())?;
        if marker == 218 {
            return dimensions.ok_or(SyncError::Invalid);
        }
        if (192..=207).contains(&marker) && ![196, 200, 204].contains(&marker) {
            require(marker == 192 || marker == 194)?;
            require(length >= 8 && dimensions.is_none())?;
            dimensions = Some((
                u32::from(be16(bytes, offset + 5)?),
                u32::from(be16(bytes, offset + 3)?),
            ));
        }
        require(![0, 1, 216, 217].contains(&marker) && !(208..=215).contains(&marker))?;
        offset += length;
    }
    Err(SyncError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};

    #[test]
    fn bounded_raster_structure_rejects_animation_truncation_and_large_dimensions() {
        let png = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j5WQAAAAASUVORK5CYII=").unwrap();
        validate(&png, "image/png").unwrap();
        let mut huge = png.clone();
        huge[16..20].copy_from_slice(&4097u32.to_be_bytes());
        assert!(validate(&huge, "image/png").is_err());
        let mut animation = png.clone();
        animation[37..41].copy_from_slice(b"acTL");
        assert!(validate(&animation, "image/png").is_err());
        for end in 0..png.len() {
            assert!(validate(&png[..end], "image/png").is_err());
        }
        assert!(validate(&png, "image/jpeg").is_err());
        // Structural JPEG skeleton only, not a renderable decoder fixture.
        let jpeg = [
            255, 216, 255, 192, 0, 8, 8, 0, 1, 0, 1, 1, 255, 218, 0, 2, 255, 217,
        ];
        validate(&jpeg, "image/jpeg").unwrap();
        let mut unsupported_sof = jpeg;
        unsupported_sof[3] = 193;
        assert!(validate(&unsupported_sof, "image/jpeg").is_err());
        for end in 0..jpeg.len() {
            assert!(validate(&jpeg[..end], "image/jpeg").is_err());
        }
    }
}
