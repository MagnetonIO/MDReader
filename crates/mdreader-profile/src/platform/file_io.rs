use crate::{Error, Result};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};

pub(crate) fn hash_file(file: &mut File, bound: u64) -> Result<(u64, String)> {
    use sha2::{Digest, Sha256};
    file.seek(SeekFrom::Start(0)).map_err(|_| Error::Storage)?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(|_| Error::Storage)?;
        if n == 0 {
            break;
        }
        total = total.checked_add(n as u64).ok_or(Error::Bounds)?;
        if total > bound {
            return Err(Error::Bounds);
        }
        hash.update(&buffer[..n]);
    }
    Ok((total, crate::model::hex(&hash.finalize())))
}
pub(crate) fn copy_file(source: &mut File, dest: &mut File, bound: u64) -> Result<(u64, String)> {
    source
        .seek(SeekFrom::Start(0))
        .map_err(|_| Error::Storage)?;
    let copied = std::io::copy(&mut source.take(bound.saturating_add(1)), dest)
        .map_err(|_| Error::Storage)?;
    if copied > bound {
        return Err(Error::Bounds);
    }
    dest.sync_all().map_err(|_| Error::Storage)?;
    hash_file(dest, bound)
}
