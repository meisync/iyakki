//! The real Win32 calls behind [`super::windows::Win32`] (F-PKG-07): `CreateFileW` on `\\.\PhysicalDriveN` and `\\.\X:`,
//! `DeviceIoControl` for the volume lock, dismount and the offline attribute, `WriteFile`/`ReadFile` with a sector-aligned buffer,
//! `FlushFileBuffers`, and the elevation check through the process token. No crate: the few functions are declared here.
//!
//! STATUS: compiled for `x86_64-pc-windows-gnu` (type check only, in a scratch crate that includes this file); NEVER RUN on a Windows
//! machine. Nothing here may be called on real hardware before a person has tried it with a spare stick (Manual row stays needed).

#![cfg(windows)]

use crate::windows::Win32;
use std::collections::HashMap;
use std::ffi::c_void;

type Handle = *mut c_void;
const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
const FSCTL_LOCK_VOLUME: u32 = 0x0009_0018;
const FSCTL_UNLOCK_VOLUME: u32 = 0x0009_001C;
const FSCTL_DISMOUNT_VOLUME: u32 = 0x0009_0020;
const IOCTL_DISK_SET_DISK_ATTRIBUTES: u32 = 0x0007_C0F4;
const IOCTL_DISK_UPDATE_PROPERTIES: u32 = 0x0007_0140;
const DISK_ATTRIBUTE_OFFLINE: u64 = 1;
const TOKEN_QUERY: u32 = 8;
const TOKEN_ADJUST_PRIVILEGES: u32 = 0x20;
const SE_PRIVILEGE_ENABLED: u32 = 2;
const TOKEN_ELEVATION_CLASS: u32 = 20;

#[repr(C)]
struct SetDiskAttributes {
    version: u32,
    persist: u8,
    reserved1: [u8; 3],
    attributes: u64,
    attributes_mask: u64,
    reserved2: [u32; 4],
}

#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    event: Handle,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(name: *const u16, access: u32, share: u32, security: *mut c_void, disposition: u32, flags: u32, template: Handle) -> Handle;
    fn CloseHandle(h: Handle) -> i32;
    fn DeviceIoControl(h: Handle, code: u32, input: *const c_void, in_len: u32, output: *mut c_void, out_len: u32, returned: *mut u32, overlapped: *mut c_void) -> i32;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: *mut Overlapped) -> i32;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, overlapped: *mut Overlapped) -> i32;
    fn FlushFileBuffers(h: Handle) -> i32;
    fn GetLastError() -> u32;
    fn GetCurrentProcess() -> Handle;
}

#[link(name = "advapi32")]
extern "system" {
    fn LookupPrivilegeValueW(system: *const u16, name: *const u16, luid: *mut Luid) -> i32;
    fn AdjustTokenPrivileges(token: Handle, disable_all: i32, new: *const TokenPrivileges, len: u32, old: *mut c_void, returned: *mut u32) -> i32;
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(token: Handle, class: u32, info: *mut c_void, len: u32, returned: *mut u32) -> i32;
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Luid {
    low: u32,
    high: i32,
}

#[repr(C)]
struct TokenPrivileges {
    count: u32,
    luid: Luid,
    attributes: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetFirmwareEnvironmentVariableExW(name: *const u16, guid: *const u16, buf: *mut c_void, size: u32, attributes: *mut u32) -> u32;
    fn SetFirmwareEnvironmentVariableExW(name: *const u16, guid: *const u16, value: *const c_void, size: u32, attributes: u32) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last(what: &str) -> String {
    // SAFETY: GetLastError only reads the calling thread's error code.
    format!("{what}: Windows error {}", unsafe { GetLastError() })
}

/// An open handle, closed on drop.
struct Owned(Handle);
impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful CreateFileW and is closed once.
        unsafe { CloseHandle(self.0) };
    }
}

// SAFETY: a Windows file handle is a process-wide token, not tied to the thread that made it; `Owned` is used by one owner at a time (the
// writer thread takes the whole `RealWin32` with it) and closed once.
unsafe impl Send for Owned {}

fn open(path: &str, flags: u32) -> Result<Owned, String> {
    let w = wide(path);
    // SAFETY: `w` is a NUL-terminated UTF-16 string that outlives the call; the other arguments are plain values or null.
    let h = unsafe { CreateFileW(w.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null_mut(), OPEN_EXISTING, flags, std::ptr::null_mut()) };
    if h == INVALID_HANDLE_VALUE || h.is_null() {
        Err(last(&format!("opening {path}")))
    } else {
        Ok(Owned(h))
    }
}

fn ioctl(h: &Owned, code: u32, input: *const c_void, in_len: u32) -> bool {
    let mut returned = 0u32;
    // SAFETY: `input` is null or points to `in_len` readable bytes held by the caller for the call; no output buffer is used.
    unsafe { DeviceIoControl(h.0, code, input, in_len, std::ptr::null_mut(), 0, &mut returned, std::ptr::null_mut()) != 0 }
}

/// A buffer whose address is a multiple of 4096 (unbuffered I/O needs sector alignment of the memory too).
struct Aligned {
    raw: Vec<u8>,
    off: usize,
    len: usize,
}
impl Aligned {
    fn new(len: usize) -> Aligned {
        let raw = vec![0u8; len + 4096];
        let off = (4096 - (raw.as_ptr() as usize % 4096)) % 4096;
        Aligned { raw, off, len }
    }
    fn bytes(&mut self) -> &mut [u8] {
        let (o, l) = (self.off, self.len);
        &mut self.raw[o..o + l]
    }
}

/// The real thing: one open handle per disk, the locked volumes kept until they are unlocked.
#[derive(Default)]
pub struct RealWin32 {
    disks: HashMap<u32, Owned>,
    volumes: HashMap<String, Owned>,
}

impl RealWin32 {
    pub fn new() -> RealWin32 {
        RealWin32::default()
    }

    fn disk(&mut self, n: u32) -> Result<&Owned, String> {
        if !self.disks.contains_key(&n) {
            let h = open(&crate::windows::physical_path(n), FILE_FLAG_WRITE_THROUGH | FILE_FLAG_NO_BUFFERING)?;
            self.disks.insert(n, h);
        }
        Ok(&self.disks[&n])
    }

    fn io(&mut self, disk: u32, offset: u64, ptr: *mut u8, len: usize, write: bool) -> Result<(), String> {
        let h = self.disk(disk)?.0;
        let mut done = 0usize;
        while done < len {
            let at = offset + done as u64;
            let mut ov = Overlapped { internal: 0, internal_high: 0, offset: at as u32, offset_high: (at >> 32) as u32, event: std::ptr::null_mut() };
            let (mut n, want) = (0u32, (len - done).min(1 << 20) as u32);
            // SAFETY: `ptr` points to `len` bytes owned by the caller for the whole call; `done + want <= len`; `ov` lives across the call.
            let ok = unsafe {
                if write {
                    WriteFile(h, ptr.add(done), want, &mut n, &mut ov)
                } else {
                    ReadFile(h, ptr.add(done), want, &mut n, &mut ov)
                }
            };
            if ok == 0 || n == 0 {
                return Err(last(if write { "writing the disk" } else { "reading the disk" }));
            }
            done += n as usize;
        }
        Ok(())
    }
}

impl Win32 for RealWin32 {
    fn is_elevated(&self) -> bool {
        let mut token: Handle = std::ptr::null_mut();
        // SAFETY: the out pointer is a local; the token is closed below when it was opened.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return false;
        }
        let mut elevation = 0u32;
        let mut returned = 0u32;
        // SAFETY: TokenElevation is a 4-byte DWORD; the buffer is a local u32 of that size.
        let ok = unsafe { GetTokenInformation(token, TOKEN_ELEVATION_CLASS, &mut elevation as *mut u32 as *mut c_void, 4, &mut returned) };
        // SAFETY: `token` was opened by OpenProcessToken above.
        unsafe { CloseHandle(token) };
        ok != 0 && elevation != 0
    }

    fn lock_and_dismount(&mut self, volume: &str) -> Result<(), String> {
        let h = open(volume, 0)?;
        if !ioctl(&h, FSCTL_LOCK_VOLUME, std::ptr::null(), 0) {
            return Err(last(&format!("{volume} is in use and could not be locked (close what uses it)")));
        }
        if !ioctl(&h, FSCTL_DISMOUNT_VOLUME, std::ptr::null(), 0) {
            let e = last(&format!("{volume} could not be dismounted"));
            ioctl(&h, FSCTL_UNLOCK_VOLUME, std::ptr::null(), 0);
            return Err(e);
        }
        self.volumes.insert(volume.to_string(), h);
        Ok(())
    }

    fn unlock(&mut self, volume: &str) {
        if let Some(h) = self.volumes.remove(volume) {
            ioctl(&h, FSCTL_UNLOCK_VOLUME, std::ptr::null(), 0);
        }
    }

    fn set_offline(&mut self, disk: u32, offline: bool) -> Result<(), String> {
        let h = self.disk(disk)?;
        let a = SetDiskAttributes { version: std::mem::size_of::<SetDiskAttributes>() as u32, persist: 0, reserved1: [0; 3], attributes: if offline { DISK_ATTRIBUTE_OFFLINE } else { 0 }, attributes_mask: DISK_ATTRIBUTE_OFFLINE, reserved2: [0; 4] };
        if !ioctl(h, IOCTL_DISK_SET_DISK_ATTRIBUTES, &a as *const SetDiskAttributes as *const c_void, a.version) {
            return Err(last(if offline { "taking the disk offline" } else { "bringing the disk online" }));
        }
        if !offline {
            // let Windows read the new partition table
            ioctl(h, IOCTL_DISK_UPDATE_PROPERTIES, std::ptr::null(), 0);
        }
        Ok(())
    }

    fn write_at(&mut self, disk: u32, offset: u64, buf: &[u8]) -> Result<(), String> {
        let mut a = Aligned::new(buf.len());
        a.bytes().copy_from_slice(buf);
        let p = a.bytes().as_mut_ptr();
        self.io(disk, offset, p, buf.len(), true)
    }

    fn read_at(&mut self, disk: u32, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        let mut a = Aligned::new(buf.len());
        let p = a.bytes().as_mut_ptr();
        self.io(disk, offset, p, buf.len(), false)?;
        buf.copy_from_slice(a.bytes());
        Ok(())
    }

    fn flush(&mut self, disk: u32) -> Result<(), String> {
        let h = self.disk(disk)?.0;
        // SAFETY: `h` is an open handle owned by `self.disks`.
        if unsafe { FlushFileBuffers(h) } == 0 {
            return Err(last("flushing the disk"));
        }
        Ok(())
    }
}

/// Write the image file to `drive` with the real calls: the whole procedure of [`crate::windows::write_stream`] (elevation check, volume
/// locks, offline, aligned blocks, read-back) over [`RealWin32`]. `sector` comes from the disk list ([`crate::windows::sector_size`]).
pub fn write_image_file(image: &std::path::Path, drive: &crate::drives::Drive, sector: u32, patch: Option<&dyn Fn(&mut [u8]) -> Result<(), String>>, progress: &mut dyn FnMut(u64, u64)) -> Result<u64, String> {
    let mut f = std::fs::File::open(image).map_err(|e| format!("{}: {e}", image.display()))?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    crate::windows::write_stream(&mut RealWin32::new(), drive, sector, &mut f, len, 4 << 20, patch, progress)
}

/// The firmware variables of the machine through the Windows calls (`Get/SetFirmwareEnvironmentVariableExW`), as a store for the boot entry
/// (F-INST-12). Windows cannot list variables, so the entry numbers come from `BootOrder` and from probing `Boot0000`..`Boot00FF`.
/// NEVER RUN on a machine: type-checked for the Windows target only.
pub struct WinFirmware {
    guid: Vec<u16>,
}

impl WinFirmware {
    /// Ask for the privilege that firmware variables need (an elevated process has it, switched off until asked for).
    pub fn new() -> Result<WinFirmware, String> {
        let mut token: Handle = std::ptr::null_mut();
        // SAFETY: the out pointer is a local; the token is closed below.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) } == 0 {
            return Err(last("opening the process token"));
        }
        let name = wide("SeSystemEnvironmentPrivilege");
        let mut luid = Luid::default();
        // SAFETY: `name` is NUL-terminated and outlives the call; `luid` is a local.
        let found = unsafe { LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) } != 0;
        let tp = TokenPrivileges { count: 1, luid, attributes: SE_PRIVILEGE_ENABLED };
        // SAFETY: `tp` is a TOKEN_PRIVILEGES with exactly one entry, `token` is open; no previous state is asked for.
        let ok = found && unsafe { AdjustTokenPrivileges(token, 0, &tp, std::mem::size_of::<TokenPrivileges>() as u32, std::ptr::null_mut(), std::ptr::null_mut()) } != 0;
        let err = last("asking for the firmware-variable privilege");
        // SAFETY: `token` was opened above.
        unsafe { CloseHandle(token) };
        if !ok {
            return Err(err);
        }
        Ok(WinFirmware { guid: wide("{8BE4DF61-93CA-11D2-AA0D-00E098032B8C}") })
    }
}

impl crate::efi::VarStore for WinFirmware {
    fn read(&self, base: &str) -> Option<Vec<u8>> {
        let name = wide(base);
        let mut buf = vec![0u8; 64 * 1024];
        let mut attrs = 0u32;
        // SAFETY: `name` and `self.guid` are NUL-terminated; `buf` has the size given; `attrs` is a local.
        let n = unsafe { GetFirmwareEnvironmentVariableExW(name.as_ptr(), self.guid.as_ptr(), buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut attrs) };
        if n == 0 {
            // "no such variable", or any other error: treated as absent (a write that follows would then fail loudly)
            return None;
        }
        buf.truncate(n as usize);
        Some(buf)
    }

    fn write(&mut self, base: &str, data: &[u8]) -> Result<(), String> {
        let name = wide(base);
        // SAFETY: `name` and `self.guid` are NUL-terminated; `data` is readable for its length.
        let ok = unsafe { SetFirmwareEnvironmentVariableExW(name.as_ptr(), self.guid.as_ptr(), data.as_ptr() as *const c_void, data.len() as u32, 7) };
        if ok == 0 {
            return Err(last(&format!("writing the firmware variable {base}")));
        }
        Ok(())
    }

    fn boot_numbers(&self) -> Vec<u16> {
        let mut n: Vec<u16> = self.read("BootOrder").unwrap_or_default().chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        n.extend(0..=0xFFu16);
        n
    }
}

const SE_FILE_OBJECT: u32 = 1;
const OWNER_AND_DACL: u32 = 1 | 4;
const TOKEN_USER_CLASS: u32 = 1;

#[link(name = "advapi32")]
extern "system" {
    fn GetNamedSecurityInfoW(name: *const u16, kind: u32, info: u32, owner: *mut *mut c_void, group: *mut *mut c_void, dacl: *mut *mut c_void, sacl: *mut *mut c_void, sd: *mut *mut c_void) -> u32;
    fn ConvertSecurityDescriptorToStringSecurityDescriptorW(sd: *mut c_void, revision: u32, info: u32, out: *mut *mut u16, len: *mut u32) -> i32;
    fn ConvertSidToStringSidW(sid: *mut c_void, out: *mut *mut u16) -> i32;
}
#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(p: *mut c_void) -> *mut c_void;
}

fn take_wide(p: *mut u16) -> String {
    // SAFETY: `p` is a NUL-terminated UTF-16 string allocated by Windows; it is read once up to the NUL and freed with LocalFree.
    unsafe {
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
        LocalFree(p as *mut c_void);
        s
    }
}

/// The file's owner and access list as an SDDL string, and the caller's SID string (F-INST-12, read by `winacl`).
pub fn file_security(file: &std::path::Path) -> Result<(String, String), String> {
    let w = wide(&file.display().to_string());
    let mut sd: *mut c_void = std::ptr::null_mut();
    // SAFETY: `w` is NUL-terminated and outlives the call; the unused out pointers are null; `sd` receives a block freed below.
    let err = unsafe { GetNamedSecurityInfoW(w.as_ptr(), SE_FILE_OBJECT, OWNER_AND_DACL, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), &mut sd) };
    if err != 0 {
        return Err(format!("reading the security of {}: Windows error {err}", file.display()));
    }
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: `sd` is the descriptor just returned; `text` receives a string freed by `take_wide`.
    let ok = unsafe { ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, 1, OWNER_AND_DACL, &mut text, std::ptr::null_mut()) };
    let sddl = if ok != 0 { Ok(take_wide(text)) } else { Err(last("writing the security descriptor as text")) };
    // SAFETY: `sd` was allocated by GetNamedSecurityInfoW and is freed once.
    unsafe { LocalFree(sd) };
    Ok((sddl?, current_user_sid()?))
}

fn current_user_sid() -> Result<String, String> {
    let mut token: Handle = std::ptr::null_mut();
    // SAFETY: the out pointer is a local; the token is closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(last("opening the process token"));
    }
    let mut buf = vec![0u8; 512];
    let mut returned = 0u32;
    // SAFETY: the buffer is `buf.len()` bytes and lives across the call.
    let ok = unsafe { GetTokenInformation(token, TOKEN_USER_CLASS, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut returned) };
    // SAFETY: `token` was opened above.
    unsafe { CloseHandle(token) };
    if ok == 0 {
        return Err(last("reading the caller's account"));
    }
    // SAFETY: TOKEN_USER starts with a pointer to the SID, which lies inside `buf`; the buffer is 8-byte aligned enough via Vec<u8> read_unaligned.
    let sid = unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const *mut c_void) };
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: `sid` points into `buf`, alive here; `text` receives a string freed by `take_wide`.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(last("writing the caller's account as text"));
    }
    Ok(take_wide(text))
}
