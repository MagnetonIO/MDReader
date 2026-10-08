//! Native-only authenticated endpoint discovery. Tokens never cross broad DACLs.
use crate::{protocol_error, Result, ENDPOINT};
use std::path::Path;

pub(crate) fn prepare(directory: &Path) -> Result<()> {
    #[cfg(windows)]
    mdreader_capture::PrivateDirectory::create(directory)
        .map_err(|_| protocol_error("reader service private directory is unavailable"))?;
    #[cfg(not(windows))]
    std::fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
pub(crate) fn publish(directory: &Path, bytes: &[u8]) -> Result<()> {
    #[cfg(windows)]
    mdreader_capture::PrivateDirectory::open(directory)
        .and_then(|private| private.publish(Path::new(ENDPOINT), bytes))
        .map_err(|_| protocol_error("reader service private endpoint is unavailable"))?;
    #[cfg(not(windows))]
    {
        use std::io::Write;
        let temporary = directory.join(format!(".endpoint-{}", uuid::Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, directory.join(ENDPOINT))?;
    }
    Ok(())
}
pub(crate) fn read(directory: &Path) -> Result<Vec<u8>> {
    #[cfg(windows)]
    let bytes = mdreader_capture::PrivateDirectory::open(directory)
        .and_then(|root| root.read(Path::new(ENDPOINT), 4096))
        .map_err(|_| protocol_error("reader service private endpoint is unavailable"))?;
    #[cfg(not(windows))]
    let bytes = {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(directory.join(ENDPOINT))?
            .take(4097)
            .read_to_end(&mut bytes)?;
        bytes
    };
    if bytes.len() > 4096 {
        return Err(protocol_error("invalid service endpoint"));
    }
    Ok(bytes)
}
