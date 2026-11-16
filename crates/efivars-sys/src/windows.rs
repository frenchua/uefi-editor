//! Windows backend.
//!
//! Enumeration uses the undocumented `NtEnumerateSystemEnvironmentValuesEx`
//! (information class 2, `VARIABLE_NAME_AND_VALUE` records), ported from
//! `winefivars/winefivars.cpp`. Reads and writes of single variables use the
//! documented `Get/SetFirmwareEnvironmentVariableExW`. All of them need
//! `SeSystemEnvironmentPrivilege`, i.e. an elevated process.

use std::ffi::c_void;
use std::io;

use efivars_core::{Guid, VarKey, VarStore, Variable};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, GetLastError, HANDLE, LUID, NTSTATUS,
};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::System::WindowsProgramming::{
    GetFirmwareEnvironmentVariableExW, SetFirmwareEnvironmentVariableExW,
};

type NtEnumerateSystemEnvironmentValuesExFn =
    unsafe extern "system" fn(information_class: u32, buffer: *mut c_void, length: *mut u32) -> NTSTATUS;

const VARIABLE_INFORMATION_VALUES: u32 = 2;
const STATUS_BUFFER_TOO_SMALL: NTSTATUS = 0xC000_0023_u32 as i32;
const STATUS_INFO_LENGTH_MISMATCH: NTSTATUS = 0xC000_0004_u32 as i32;

/// Fixed part of `VARIABLE_NAME_AND_VALUE`; the UTF-16 name follows.
const RECORD_HEADER_LEN: usize = 4 * 4 + 16;

pub struct WindowsStore {
    enumerate: NtEnumerateSystemEnvironmentValuesExFn,
}

impl WindowsStore {
    pub fn new() -> io::Result<Self> {
        enable_system_environment_privilege()?;
        let ntdll = wide("ntdll.dll");
        // SAFETY: plain Win32 calls with valid NUL-terminated arguments; the
        // transmuted pointer has the documented-by-convention signature.
        let enumerate = unsafe {
            let module = GetModuleHandleW(ntdll.as_ptr());
            if module.is_null() {
                return Err(io::Error::last_os_error());
            }
            let proc = GetProcAddress(module, c"NtEnumerateSystemEnvironmentValuesEx".as_ptr().cast())
                .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "NtEnumerateSystemEnvironmentValuesEx not found"))?;
            std::mem::transmute::<unsafe extern "system" fn() -> isize, NtEnumerateSystemEnvironmentValuesExFn>(proc)
        };
        Ok(Self { enumerate })
    }

    fn enumerate_raw(&self) -> io::Result<Vec<u8>> {
        let mut len: u32 = 0;
        // The required size can grow between calls, so retry a few times.
        for _ in 0..4 {
            let mut buf = vec![0u8; len as usize];
            let ptr = if buf.is_empty() { std::ptr::null_mut() } else { buf.as_mut_ptr().cast() };
            // SAFETY: `ptr` is null (size query) or points to `len` writable bytes.
            let status = unsafe { (self.enumerate)(VARIABLE_INFORMATION_VALUES, ptr, &mut len) };
            if status >= 0 && !buf.is_empty() {
                buf.truncate(len as usize);
                return Ok(buf);
            }
            if status != STATUS_BUFFER_TOO_SMALL && status != STATUS_INFO_LENGTH_MISMATCH && status < 0 {
                return Err(io::Error::other(format!(
                    "NtEnumerateSystemEnvironmentValuesEx failed: 0x{:08x}",
                    status as u32
                )));
            }
            if status >= 0 && len == 0 {
                return Ok(Vec::new());
            }
        }
        Err(io::Error::other("variable store kept growing during enumeration"))
    }
}

/// Walk the `VARIABLE_NAME_AND_VALUE` records with full bounds checking.
pub fn parse_records(buffer: &[u8]) -> io::Result<Vec<Variable>> {
    let invalid = |msg: &str| io::Error::new(io::ErrorKind::InvalidData, msg.to_string());
    let u32_at = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;

    let mut vars = Vec::new();
    let mut offset = 0;
    while offset < buffer.len() {
        let rec = &buffer[offset..];
        if rec.len() < RECORD_HEADER_LEN {
            return Err(invalid("truncated VARIABLE_NAME_AND_VALUE record"));
        }
        let next = u32_at(rec, 0);
        let value_offset = u32_at(rec, 4);
        let value_len = u32_at(rec, 8);
        let attributes = u32_at(rec, 12) as u32;
        let vendor = Guid::from_bytes(rec[16..32].try_into().unwrap());
        if value_offset < RECORD_HEADER_LEN
            || value_offset > rec.len()
            || value_len > rec.len() - value_offset
        {
            return Err(invalid("VARIABLE_NAME_AND_VALUE value out of bounds"));
        }
        // The name is NUL-terminated and padded up to ValueOffset.
        let name = efivars_core::devpath::utf16_string(&rec[RECORD_HEADER_LEN..value_offset]);
        let data = rec[value_offset..value_offset + value_len].to_vec();
        vars.push(Variable { key: VarKey::new(name, vendor), attributes, data });

        if next == 0 {
            break;
        }
        if next > rec.len() {
            return Err(invalid("VARIABLE_NAME_AND_VALUE NextEntryOffset out of bounds"));
        }
        offset += next;
    }
    Ok(vars)
}

impl VarStore for WindowsStore {
    fn describe(&self) -> String {
        "Windows firmware (NtEnumerateSystemEnvironmentValuesEx)".into()
    }

    fn list(&mut self) -> io::Result<Vec<Variable>> {
        parse_records(&self.enumerate_raw()?)
    }

    fn read(&mut self, key: &VarKey) -> io::Result<Variable> {
        let name = wide(&key.name);
        let guid = wide(&format!("{{{}}}", key.vendor));
        let mut buf = vec![0u8; 4096];
        loop {
            let mut attributes = 0u32;
            // SAFETY: buffers are valid for the sizes passed.
            let len = unsafe {
                GetFirmwareEnvironmentVariableExW(
                    name.as_ptr(), guid.as_ptr(), buf.as_mut_ptr().cast(), buf.len() as u32, &mut attributes,
                )
            };
            if len != 0 {
                buf.truncate(len as usize);
                return Ok(Variable { key: key.clone(), attributes, data: buf });
            }
            // SAFETY: trivially safe.
            let err = unsafe { GetLastError() };
            if err == ERROR_INSUFFICIENT_BUFFER && buf.len() < 1 << 24 {
                buf.resize(buf.len() * 4, 0);
                continue;
            }
            // A zero-length variable also returns 0 with ERROR_SUCCESS.
            if err == ERROR_SUCCESS {
                return Ok(Variable { key: key.clone(), attributes, data: Vec::new() });
            }
            return Err(io::Error::from_raw_os_error(err as i32));
        }
    }

    fn write(&mut self, var: &Variable) -> io::Result<()> {
        if var.data.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "an empty write deletes the variable; use delete"));
        }
        set_variable(&var.key, &var.data, var.attributes)
    }

    fn delete(&mut self, key: &VarKey) -> io::Result<()> {
        set_variable(key, &[], 0)
    }
}

fn set_variable(key: &VarKey, data: &[u8], attributes: u32) -> io::Result<()> {
    let name = wide(&key.name);
    let guid = wide(&format!("{{{}}}", key.vendor));
    let ptr = if data.is_empty() { std::ptr::null() } else { data.as_ptr().cast() };
    // SAFETY: buffers are valid for the sizes passed.
    let ok = unsafe {
        SetFirmwareEnvironmentVariableExW(name.as_ptr(), guid.as_ptr(), ptr, data.len() as u32, attributes)
    };
    if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

fn enable_system_environment_privilege() -> io::Result<()> {
    let name = wide("SeSystemEnvironmentPrivilege");
    // SAFETY: standard token adjustment; the handle is closed on every path.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut luid = LUID { LowPart: 0, HighPart: 0 };
        if LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) == 0 {
            let err = io::Error::last_os_error();
            CloseHandle(token);
            return Err(err);
        }
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
        };
        let adjusted = AdjustTokenPrivileges(token, 0, &privileges, 0, std::ptr::null_mut(), std::ptr::null_mut());
        // AdjustTokenPrivileges "succeeds" with ERROR_NOT_ALL_ASSIGNED when the
        // token lacks the privilege, so the last error must be checked too.
        let err = GetLastError();
        CloseHandle(token);
        if adjusted == 0 || err != ERROR_SUCCESS {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "could not enable SeSystemEnvironmentPrivilege ({}); run as Administrator",
                    io::Error::from_raw_os_error(err as i32)
                ),
            ));
        }
    }
    Ok(())
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use efivars_core::guids::EFI_GLOBAL_VARIABLE;

    fn record(name: &str, value: &[u8], last: bool) -> Vec<u8> {
        let name: Vec<u8> = name.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        let value_offset = RECORD_HEADER_LEN + name.len();
        let total = (value_offset + value.len()).next_multiple_of(8);
        let mut r = Vec::new();
        r.extend_from_slice(&(if last { 0 } else { total as u32 }).to_le_bytes());
        r.extend_from_slice(&(value_offset as u32).to_le_bytes());
        r.extend_from_slice(&(value.len() as u32).to_le_bytes());
        r.extend_from_slice(&7u32.to_le_bytes());
        r.extend_from_slice(&EFI_GLOBAL_VARIABLE.to_bytes());
        r.extend_from_slice(&name);
        r.extend_from_slice(value);
        r.resize(total, 0);
        r
    }

    #[test]
    fn parses_records() {
        let buf = [record("Timeout", &[5, 0], false), record("BootOrder", &[1, 0, 2, 0], true)].concat();
        let vars = parse_records(&buf).unwrap();
        assert_eq!(vars.len(), 2);
        assert_eq!(vars[0].key, VarKey::new("Timeout", EFI_GLOBAL_VARIABLE));
        assert_eq!(vars[0].data, [5, 0]);
        assert_eq!(vars[1].key.name, "BootOrder");
        assert_eq!(vars[1].attributes, 7);
    }

    #[test]
    fn rejects_out_of_bounds() {
        let mut buf = record("Timeout", &[5, 0], true);
        buf[8..12].copy_from_slice(&1000u32.to_le_bytes());
        assert!(parse_records(&buf).is_err());
        assert!(parse_records(&buf[..10]).is_err());
    }
}
