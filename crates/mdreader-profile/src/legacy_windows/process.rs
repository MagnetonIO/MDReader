//! Bounded OS observations; command lines are inspected privately and discarded.
use super::*;
use std::{
    ffi::OsString,
    mem::size_of,
    os::windows::{
        ffi::OsStringExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
};
use windows_sys::{
    Wdk::System::Threading::{NtQueryInformationProcess, ProcessCommandLineInformation},
    Win32::{
        Foundation::{
            GetLastError, LocalFree, ERROR_NO_MORE_FILES, FILETIME, INVALID_HANDLE_VALUE,
            UNICODE_STRING, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        System::{Diagnostics::ToolHelp::*, Threading::*},
        UI::Shell::CommandLineToArgvW,
    },
};
const MAX_PROCESSES: usize = 65536;
const MAX_MATCHES: usize = 16;
const MAX_COMMAND_BYTES: usize = 65536;
pub(super) struct Process {
    handle: OwnedHandle,
    gui: bool,
    pid: u32,
    parent: u32,
    created: u64,
}
impl Process {
    pub(super) fn gui(&self) -> bool {
        self.gui
    }
    #[allow(unsafe_code)]
    pub(super) fn closed(&self) -> Result<()> {
        // SAFETY: retained process handle; zero-time poll, no mutation or wait.
        if unsafe { WaitForSingleObject(self.handle.as_raw_handle().cast(), 0) } != WAIT_OBJECT_0 {
            return Err(Error::NotQuiescent);
        }
        Ok(())
    }
    #[allow(unsafe_code)]
    fn alive(&self) -> Result<()> {
        // SAFETY: query only the retained instance; no PID lookup or blocking.
        if unsafe { WaitForSingleObject(self.handle.as_raw_handle().cast(), 0) } != WAIT_TIMEOUT {
            return Err(Error::NotQuiescent);
        }
        Ok(())
    }
}
#[allow(unsafe_code)]
fn created(handle: &OwnedHandle) -> Result<u64> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: query a borrowed live handle into correctly typed initialized outputs.
    if unsafe {
        GetProcessTimes(
            handle.as_raw_handle().cast(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(Error::NotQuiescent);
    }
    let value = (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
    if value == 0 {
        return Err(Error::NotQuiescent);
    }
    Ok(value)
}
#[allow(unsafe_code)]
fn image(handle: &OwnedHandle) -> Result<PathBuf> {
    let mut wide = vec![0u16; 32768];
    let mut count = wide.len() as u32;
    // SAFETY: bounded live output buffer and borrowed native process handle.
    if unsafe {
        QueryFullProcessImageNameW(
            handle.as_raw_handle().cast(),
            0,
            wide.as_mut_ptr(),
            &mut count,
        )
    } == 0
        || count == 0
        || count as usize >= wide.len()
    {
        return Err(Error::NotQuiescent);
    }
    wide.truncate(count as usize);
    Ok(PathBuf::from(OsString::from_wide(&wide)))
}
#[allow(unsafe_code)]
fn arguments(handle: &OwnedHandle) -> Result<Vec<OsString>> {
    let mut words = vec![0usize; MAX_COMMAND_BYTES / size_of::<usize>()];
    let mut returned = 0u32;
    // SAFETY: aligned initialized local buffer; this information class copies
    // the command line into the supplied buffer, not a remote pointer read.
    let status = unsafe {
        NtQueryInformationProcess(
            handle.as_raw_handle().cast(),
            ProcessCommandLineInformation,
            words.as_mut_ptr().cast(),
            MAX_COMMAND_BYTES as u32,
            &mut returned,
        )
    };
    if status != 0
        || returned as usize > MAX_COMMAND_BYTES
        || (returned as usize) < size_of::<UNICODE_STRING>()
    {
        return Err(Error::Unsupported);
    }
    // SAFETY: the successful kernel result initializes the header at alignment.
    let value = unsafe { &*words.as_ptr().cast::<UNICODE_STRING>() };
    let start = words.as_ptr() as usize;
    let pointer = value.Buffer as usize;
    let length = value.Length as usize;
    if !length.is_multiple_of(2)
        || length == 0
        || pointer < start + size_of::<UNICODE_STRING>()
        || pointer
            .checked_add(length)
            .is_none_or(|end| end > start + returned as usize)
        || !pointer.is_multiple_of(2)
    {
        return Err(Error::Unsupported);
    }
    // SAFETY: range and alignment were checked against the initialized buffer.
    let mut line = unsafe { std::slice::from_raw_parts(value.Buffer, length / 2) }.to_vec();
    if line.contains(&0) {
        return Err(Error::Unsupported);
    }
    line.push(0);
    let mut count = 0;
    // SAFETY: terminated local wide string; Windows owns the returned allocation.
    let array = unsafe { CommandLineToArgvW(line.as_ptr(), &mut count) };
    if array.is_null() {
        return Err(Error::Unsupported);
    }
    let result = (|| {
        if !(1..=4).contains(&count) {
            return Err(Error::Unsupported);
        }
        let mut arguments = Vec::new();
        for i in 0..count as usize {
            // SAFETY: CommandLineToArgvW reports this many allocated pointers.
            let argument = unsafe { *array.add(i) };
            let mut length = 0;
            while length < MAX_COMMAND_BYTES / 2 {
                // SAFETY: each API-returned argument is a terminated wide string.
                if unsafe { *argument.add(length) } == 0 {
                    break;
                }
                length += 1;
            }
            if length == MAX_COMMAND_BYTES / 2 {
                return Err(Error::Bounds);
            }
            // SAFETY: bounded observed argument length in the API allocation.
            arguments.push(OsString::from_wide(unsafe {
                std::slice::from_raw_parts(argument, length)
            }));
        }
        Ok(arguments)
    })();
    // SAFETY: release exactly the local allocation returned by CommandLineToArgvW.
    unsafe {
        LocalFree(array.cast());
    }
    result
}
#[allow(unsafe_code)]
pub(super) fn matching(executable: &Path, profile: &Path) -> Result<Vec<Process>> {
    let executable = executable.canonicalize().map_err(|_| Error::Conflict)?;
    // SAFETY: fixed read-only process snapshot flags, no targeted mutation.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(Error::NotQuiescent);
    }
    // SAFETY: successful snapshot transfers one owned handle.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: live snapshot and correctly sized initialized ABI output.
    let mut found = unsafe { Process32FirstW(snapshot.as_raw_handle().cast(), &mut entry) };
    let mut processes = Vec::new();
    let mut scanned = 0;
    while found != 0 {
        scanned += 1;
        if scanned > MAX_PROCESSES {
            return Err(Error::Bounds);
        }
        let n = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .ok_or(Error::Bounds)?;
        let name = OsString::from_wide(&entry.szExeFile[..n]);
        if name
            .to_str()
            .is_some_and(|n| n.eq_ignore_ascii_case("MDReader.exe"))
        {
            // SAFETY: only query and synchronization rights, never terminate/write.
            let raw = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    0,
                    entry.th32ProcessID,
                )
            };
            if raw.is_null() {
                return Err(Error::NotQuiescent);
            }
            // SAFETY: successful native open transfers one retained lifetime handle.
            let handle = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
            if image(&handle)?
                .canonicalize()
                .map_err(|_| Error::Conflict)?
                == executable
            {
                // SAFETY: compare the actual opened process, not only snapshot PID.
                if unsafe { GetProcessId(handle.as_raw_handle().cast()) } != entry.th32ProcessID
                    || entry.th32ProcessID == unsafe { GetCurrentProcessId() }
                {
                    return Err(Error::NotQuiescent);
                }
                let args = arguments(&handle)?;
                let gui = args.len() == 1;
                let service = args.len() == 3
                    && args[1] == "--mdreader-service"
                    && Path::new(&args[2])
                        .canonicalize()
                        .map_err(|_| Error::Conflict)?
                        == profile
                            .join("reader-v1")
                            .canonicalize()
                            .map_err(|_| Error::Conflict)?;
                if !gui && !service {
                    return Err(Error::Unsupported);
                }
                if processes.len() == MAX_MATCHES {
                    return Err(Error::Bounds);
                }
                let created = created(&handle)?;
                processes.push(Process {
                    handle,
                    gui,
                    pid: entry.th32ProcessID,
                    parent: entry.th32ParentProcessID,
                    created,
                });
            }
        }
        // SAFETY: retained snapshot and reusable correctly sized output.
        found = unsafe { Process32NextW(snapshot.as_raw_handle().cast(), &mut entry) };
    }
    // SAFETY: read the final enumeration status before any other native call.
    if unsafe { GetLastError() } != ERROR_NO_MORE_FILES {
        return Err(Error::NotQuiescent);
    }
    for process in &processes {
        process.alive()?;
        if !process.gui {
            // The ToolHelp parent PID is only a hint until linked to the actual
            // retained, live GUI handle. Creation order rejects a recycled PID.
            let parent = processes
                .iter()
                .find(|parent| parent.gui && parent.pid == process.parent)
                .ok_or(Error::Unsupported)?;
            if process.created < parent.created {
                return Err(Error::NotQuiescent);
            }
            parent.alive()?;
        }
    }
    Ok(processes)
}
