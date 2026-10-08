//! Pinned local directory chain. No reparse ancestors or delete sharing.
use super::*;
use std::{
    os::windows::{
        ffi::OsStrExt,
        fs::MetadataExt,
        io::{AsRawHandle, FromRawHandle},
    },
    ptr,
};
use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE, System::WindowsProgramming::DRIVE_FIXED,
};

#[derive(PartialEq, Eq)]
pub(super) struct Identity(pub(super) u64, [u8; 16]);
#[allow(unsafe_code)]
pub(super) fn identity(file: &File) -> Result<Identity> {
    let mut info = FILE_ID_INFO::default();
    // SAFETY: live file HANDLE and correctly sized output structure.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle().cast(),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(CaptureError::UnsafePath);
    }
    Ok(Identity(info.VolumeSerialNumber, info.FileId.Identifier))
}
pub(super) fn inspect(file: &File, directory: bool, private: bool) -> Result<()> {
    let metadata = file.metadata()?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || metadata.is_dir() != directory
        || (!directory && !metadata.is_file())
    {
        return Err(CaptureError::UnsafePath);
    }
    if !directory {
        single_link(file)?;
    }
    if private {
        security::verify(file)?;
    }
    Ok(())
}
#[allow(unsafe_code)]
fn single_link(file: &File) -> Result<()> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: live handle and fixed ABI output; hard-linked files are never used.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), &mut info) } == 0
        || info.nNumberOfLinks != 1
    {
        return Err(CaptureError::UnsafePath);
    }
    Ok(())
}
#[allow(unsafe_code)]
pub(super) fn ntfs(file: &File) -> Result<()> {
    let mut name = [0u16; 32];
    // SAFETY: bounded filesystem-name output buffer; all other outputs optional.
    if unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle().cast(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    } == 0
    {
        return Err(CaptureError::UnsafePath);
    }
    if name[..5] != [b'N' as u16, b'T' as u16, b'F' as u16, b'S' as u16, 0] {
        return Err(CaptureError::UnsupportedPlatform);
    }
    Ok(())
}
#[allow(unsafe_code)]
pub(super) fn directory(path: &Path, create: bool, private: bool) -> Result<File> {
    let wide = wide(path)?;
    if create {
        let descriptor = security::Descriptor::private()?;
        let attributes = descriptor.attributes();
        // SAFETY: live terminated path and descriptor. Existing directories are never repaired.
        if unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) } == 0
            && std::io::Error::last_os_error().raw_os_error() != Some(183)
        {
            return Err(CaptureError::UnsafePath);
        }
    }
    // Pin against rename/replacement and write-based reparse mutation for this chain's lifetime.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(CaptureError::UnsafePath);
    }
    // SAFETY: unique successful CreateFile handle ownership.
    let file = unsafe { File::from_raw_handle(handle.cast()) };
    inspect(&file, true, private)?;
    Ok(file)
}
pub(super) struct Chain {
    pub(super) handles: Vec<(PathBuf, File, Identity)>,
}
impl Chain {
    pub(super) fn open(path: &Path, create: bool, private_final: bool) -> Result<Self> {
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        let mut current = PathBuf::new();
        let mut chain = Self {
            handles: Vec::new(),
        };
        let components: Vec<_> = path.components().collect();
        if components.len() > 128
            || components.len() < 2
            || !matches!(components[0], Component::Prefix(prefix) if matches!(prefix.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)))
            || !matches!(components[1], Component::RootDir)
            || components[2..]
                .iter()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(CaptureError::UnsafePath);
        }
        let volume_root: PathBuf = components[..2]
            .iter()
            .map(|component| component.as_os_str())
            .collect();
        local_fixed_volume(&volume_root)?;
        for (index, component) in components.iter().enumerate() {
            match component {
                Component::Prefix(prefix)
                    if matches!(
                        prefix.kind(),
                        std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
                    ) =>
                {
                    current.push(prefix.as_os_str())
                }
                Component::RootDir => current.push(component.as_os_str()),
                Component::Normal(name) => current.push(name),
                _ => return Err(CaptureError::UnsafePath),
            }
            if matches!(component, Component::Prefix(_)) {
                continue;
            }
            let private = index + 1 == components.len() && private_final;
            let file = directory(
                &current,
                create && matches!(component, Component::Normal(_)),
                private,
            )?;
            let id = identity(&file)?;
            chain.handles.push((current.clone(), file, id));
        }
        if chain.handles.is_empty() {
            return Err(CaptureError::UnsafePath);
        }
        ntfs(&chain.handles.last().ok_or(CaptureError::UnsafePath)?.1)?;
        Ok(chain)
    }
    pub(super) fn verify(&self) -> Result<()> {
        for (path, file, id) in &self.handles {
            inspect(file, true, false)?;
            let reopened = directory(path, false, false)?;
            if &identity(&reopened)? != id {
                return Err(CaptureError::UnsafePath);
            }
        }
        Ok(())
    }
}
pub(super) fn wide(path: &Path) -> Result<Vec<u16>> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if wide.len() > 32768 || wide[..wide.len() - 1].contains(&0) {
        return Err(CaptureError::UnsafePath);
    }
    Ok(wide)
}
pub(crate) struct DirectoryAuthority {
    chains: Vec<Chain>,
}
impl DirectoryAuthority {
    pub(crate) fn capture(root: &Path) -> Result<Self> {
        let mut chains = vec![Chain::open(root, false, true)?];
        for child in ["pending", "markdown", "receipts", "temporary"] {
            chains.push(Chain::open(&root.join(child), false, true)?);
        }
        Ok(Self { chains })
    }
    pub(crate) fn verify(&self, _root: &Path) -> Result<()> {
        for chain in &self.chains {
            chain.verify()?;
            security::verify(&chain.handles.last().ok_or(CaptureError::UnsafePath)?.1)?;
        }
        Ok(())
    }
}

#[allow(unsafe_code)]
fn local_fixed_volume(root: &Path) -> Result<()> {
    let root = wide(root)?;
    // SAFETY: terminated drive-root path; remote and removable volumes are rejected.
    if unsafe { GetDriveTypeW(root.as_ptr()) } != DRIVE_FIXED {
        return Err(CaptureError::UnsupportedPlatform);
    }
    Ok(())
}
