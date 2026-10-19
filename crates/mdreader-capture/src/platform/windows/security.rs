//! Handle-based private DACL validation. No inherited or broad allow entries.
use crate::{CaptureError, Result};
use std::{
    fs::File,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
};
use windows_sys::Win32::{
    Foundation::{LocalFree, HANDLE},
    Security::Authorization::*,
    Security::*,
    Storage::FileSystem::FILE_ALL_ACCESS,
    System::{
        SystemServices::ACCESS_ALLOWED_ACE_TYPE,
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

pub(super) struct Descriptor(PSECURITY_DESCRIPTOR);
impl Descriptor {
    #[allow(unsafe_code)]
    pub(super) fn private() -> Result<Self> {
        let user = User::current()?;
        let mut text = ptr::null_mut();
        // SAFETY: user retains its validated token buffer; Windows owns text.
        if unsafe { ConvertSidToStringSidW(user.sid(), &mut text) } == 0 {
            return Err(CaptureError::UnsafePath);
        }
        let allocation = Allocation(text.cast());
        let mut length = 0;
        // Token SID strings are bounded by the maximum Windows SID length.
        while length < 256 && unsafe { *text.add(length) } != 0 {
            length += 1;
        }
        if length == 256 {
            return Err(CaptureError::UnsafePath);
        }
        let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
            .map_err(|_| CaptureError::UnsafePath)?;
        drop(allocation);
        let sddl: Vec<u16> = format!("O:{sid}D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = ptr::null_mut();
        // SAFETY: NUL-terminated live SDDL and valid output pointer.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(CaptureError::UnsafePath);
        }
        Ok(Self(descriptor))
    }
    pub(super) fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}
#[allow(unsafe_code)]
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
struct Allocation(*mut std::ffi::c_void);
#[allow(unsafe_code)]
impl Drop for Allocation {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
struct User {
    words: Vec<usize>,
}
impl User {
    #[allow(unsafe_code)]
    fn current() -> Result<Self> {
        let mut token: HANDLE = ptr::null_mut();
        // SAFETY: valid process pseudo-handle and one output HANDLE.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(CaptureError::UnsafePath);
        }
        // SAFETY: API success transferred one non-null token HANDLE.
        let token = unsafe { OwnedHandle::from_raw_handle(token.cast()) };
        let mut length = 0;
        unsafe {
            GetTokenInformation(
                token.as_raw_handle().cast(),
                TokenUser,
                ptr::null_mut(),
                0,
                &mut length,
            );
        }
        if (length as usize) < size_of::<TOKEN_USER>() || length > 64 * 1024 {
            return Err(CaptureError::UnsafePath);
        }
        let mut words = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        // SAFETY: usize storage aligns TOKEN_USER and has the requested byte capacity.
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle().cast(),
                TokenUser,
                words.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        } == 0
        {
            return Err(CaptureError::UnsafePath);
        }
        let user = Self { words };
        if unsafe { IsValidSid(user.sid()) } == 0 {
            return Err(CaptureError::UnsafePath);
        }
        Ok(user)
    }
    #[allow(unsafe_code)]
    fn sid(&self) -> PSID {
        // SAFETY: successfully populated aligned TOKEN_USER buffer remains alive.
        unsafe { (*self.words.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
}
#[allow(unsafe_code)]
pub(super) fn verify(file: &File) -> Result<()> {
    let user = User::current()?;
    let mut owner = ptr::null_mut();
    let mut acl = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    // SAFETY: borrowed live HANDLE; Windows returns a LocalFree-owned descriptor.
    let code = unsafe {
        GetSecurityInfo(
            file.as_raw_handle().cast(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut acl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if code != 0 {
        return Err(CaptureError::UnsafePath);
    }
    let _allocation = Allocation(descriptor);
    let mut control = 0;
    let mut revision = 0;
    if owner.is_null()
        || acl.is_null()
        || unsafe { IsValidSecurityDescriptor(descriptor) } == 0
        || unsafe { EqualSid(owner, user.sid()) } == 0
        || unsafe { IsValidAcl(acl) } == 0
        || unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
        || control & SE_DACL_PROTECTED == 0
    {
        return Err(CaptureError::UnsafePath);
    }
    let mut size = ACL_SIZE_INFORMATION::default();
    if unsafe {
        GetAclInformation(
            acl,
            (&mut size as *mut ACL_SIZE_INFORMATION).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
        || size.AceCount != 2
    {
        return Err(CaptureError::UnsafePath);
    }
    let mut system = [0u32; 17];
    let mut system_length = size_of_val(&system) as u32;
    if unsafe {
        CreateWellKnownSid(
            WinLocalSystemSid,
            ptr::null_mut(),
            system.as_mut_ptr().cast(),
            &mut system_length,
        )
    } == 0
    {
        return Err(CaptureError::UnsafePath);
    }
    let mut seen_user = false;
    let mut seen_system = false;
    for index in 0..2 {
        let mut ace = ptr::null_mut();
        if unsafe { GetAce(acl, index, &mut ace) } == 0 {
            return Err(CaptureError::UnsafePath);
        }
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE
            || u32::from(header.AceFlags) & !(OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE) != 0
            || usize::from(header.AceSize) < size_of::<ACCESS_ALLOWED_ACE>()
        {
            return Err(CaptureError::UnsafePath);
        }
        let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        let sid_bytes =
            usize::from(header.AceSize) - std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart);
        // A SID header is eight bytes; bound its subauthority count before any SID API reads it.
        if sid_bytes < 8 {
            return Err(CaptureError::UnsafePath);
        }
        let sid_pointer = ptr::addr_of!(allowed.SidStart).cast::<u8>();
        let subauthorities = usize::from(unsafe { *sid_pointer.add(1) });
        if 8 + subauthorities * 4 > sid_bytes {
            return Err(CaptureError::UnsafePath);
        }
        let sid = sid_pointer.cast_mut().cast();
        if allowed.Mask != FILE_ALL_ACCESS || unsafe { IsValidSid(sid) } == 0 {
            return Err(CaptureError::UnsafePath);
        }
        if unsafe { EqualSid(sid, user.sid()) } != 0 {
            if seen_user {
                return Err(CaptureError::UnsafePath);
            }
            seen_user = true;
        } else if unsafe { EqualSid(sid, system.as_mut_ptr().cast()) } != 0 {
            if seen_system {
                return Err(CaptureError::UnsafePath);
            }
            seen_system = true;
        } else {
            return Err(CaptureError::UnsafePath);
        }
    }
    if !seen_user || !seen_system {
        return Err(CaptureError::UnsafePath);
    }
    Ok(())
}
