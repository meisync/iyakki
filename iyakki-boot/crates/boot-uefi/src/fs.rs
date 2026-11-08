// SPDX-License-Identifier: MIT OR Apache-2.0
//! Access to a FAT volume through the firmware's file protocol. Everything reads, except `Volume::write`, which replaces one
//! file (the boot log) and is the only place that opens a file for writing.

use crate::sys::{from_ucs2, protocol, ucs2};
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr::null_mut;
use uefi_sys::*;

/// An opened volume root.
pub struct Volume {
    root: *mut FileProtocol,
    /// The handle that carries the file system.
    pub handle: Handle,
}

/// One directory entry.
pub struct DirEntry {
    /// File name as stored.
    pub name: String,
    /// Is a directory.
    pub dir: bool,
}

/// `/a/b` or `\a\b` to `\a\b`.
pub fn backslash(path: &str) -> String {
    path.replace('/', "\\")
}

impl Volume {
    /// Open the root of the file system on a handle.
    pub fn open(handle: Handle) -> Option<Volume> {
        let fs: *mut SimpleFileSystem = protocol(handle, &SIMPLE_FILE_SYSTEM_PROTOCOL_GUID)?;
        let mut root: *mut FileProtocol = null_mut();
        let s = unsafe { ((*fs).open_volume)(fs, &mut root) };
        if s.is_error() || root.is_null() {
            return None;
        }
        Some(Volume { root, handle })
    }

    fn open_path(&self, path: &str) -> Option<*mut FileProtocol> {
        let name = ucs2(&backslash(path));
        let mut f: *mut FileProtocol = null_mut();
        let s = unsafe { ((*self.root).open)(self.root, &mut f, name.as_ptr(), FILE_MODE_READ, 0) };
        if s.is_error() || f.is_null() {
            None
        } else {
            Some(f)
        }
    }

    /// Size of an opened file.
    fn size_of(f: *mut FileProtocol) -> Option<u64> {
        let mut sz = 0usize;
        let s = unsafe { ((*f).get_info)(f, &FILE_INFO_GUID, &mut sz, null_mut()) };
        if s != Status::BUFFER_TOO_SMALL || sz < core::mem::size_of::<FileInfo>() {
            return None;
        }
        let mut buf = alloc::vec![0u64; sz.div_ceil(8)];
        let s = unsafe { ((*f).get_info)(f, &FILE_INFO_GUID, &mut sz, buf.as_mut_ptr() as *mut c_void) };
        if s.is_error() {
            return None;
        }
        Some(unsafe { (*(buf.as_ptr() as *const FileInfo)).file_size })
    }

    /// Read at most `max` bytes from the start of a file (`None` when it does not exist or cannot be read).
    pub fn read(&self, path: &str, max: usize) -> Option<Vec<u8>> {
        let f = self.open_path(path)?;
        let r = (|| {
            let size = Self::size_of(f)? as usize;
            let mut want = size.min(max);
            let mut v = alloc::vec![0u8; want];
            let s = unsafe { ((*f).read)(f, &mut want, v.as_mut_ptr() as *mut c_void) };
            if s.is_error() {
                return None;
            }
            v.truncate(want);
            Some(v)
        })();
        unsafe { ((*f).close)(f) };
        r
    }

    /// The names in a directory (without `.` and `..`).
    pub fn list(&self, path: &str) -> Option<Vec<DirEntry>> {
        let d = self.open_path(path)?;
        let mut out = Vec::new();
        let mut buf = alloc::vec![0u64; 160];
        loop {
            let mut sz = buf.len() * 8;
            let s = unsafe { ((*d).read)(d, &mut sz, buf.as_mut_ptr() as *mut c_void) };
            if s.is_error() || sz == 0 {
                break;
            }
            let info = buf.as_ptr() as *const FileInfo;
            let base = core::mem::size_of::<FileInfo>();
            let chars = unsafe { core::slice::from_raw_parts((buf.as_ptr() as *const u8).add(base) as *const u16, (sz.saturating_sub(base)) / 2) };
            let name = from_ucs2(chars);
            if name != "." && name != ".." {
                out.push(DirEntry { name, dir: unsafe { (*info).attribute } & FILE_DIRECTORY != 0 });
            }
        }
        unsafe { ((*d).close)(d) };
        Some(out)
    }

    /// Replace the file at `path` with `data` (the whole file; an old one is deleted first so no tail of it survives). The
    /// directory must exist. `false` when the volume refuses (read-only medium, a firmware without write support): the caller
    /// carries on, a log that cannot be written must never stop a start.
    pub fn write(&self, path: &str, data: &[u8]) -> bool {
        let name = ucs2(&backslash(path));
        // an old file: open it for writing and delete it (Delete closes the handle, whatever it returns)
        let mut old: *mut FileProtocol = null_mut();
        let s = unsafe { ((*self.root).open)(self.root, &mut old, name.as_ptr(), FILE_MODE_READ | FILE_MODE_WRITE, 0) };
        if !s.is_error() && !old.is_null() {
            unsafe { ((*old).delete)(old) };
        }
        let mut f: *mut FileProtocol = null_mut();
        let s = unsafe { ((*self.root).open)(self.root, &mut f, name.as_ptr(), FILE_MODE_READ | FILE_MODE_WRITE | FILE_MODE_CREATE, 0) };
        if s.is_error() || f.is_null() {
            return false;
        }
        let mut n = data.len();
        let s = unsafe { ((*f).write)(f, &mut n, data.as_ptr() as *const c_void) };
        let ok = !s.is_error() && n == data.len();
        unsafe {
            ((*f).flush)(f);
            ((*f).close)(f);
        }
        ok
    }

    /// Does the path exist?
    pub fn exists(&self, path: &str) -> bool {
        match self.open_path(path) {
            Some(f) => {
                unsafe { ((*f).close)(f) };
                true
            }
            None => false,
        }
    }
}

impl Drop for Volume {
    fn drop(&mut self) {
        unsafe { ((*self.root).close)(self.root) };
    }
}

/// Build a device path: the volume's path, then a file path node for `path`, then the end node. Needed so a started image knows
/// which volume it came from (another system's loader looks for its own files there).
pub fn file_device_path(volume: Handle, path: &str) -> Option<Vec<u8>> {
    let base: *const DevicePathProtocol = protocol(volume, &DEVICE_PATH_PROTOCOL_GUID)?;
    let mut out: Vec<u8> = Vec::new();
    let mut p = base;
    unsafe {
        while !(*p).is_end_entire() {
            let n = (*p).node_length() as usize;
            if n < 4 {
                return None;
            }
            out.extend_from_slice(core::slice::from_raw_parts(p as *const u8, n));
            p = (p as *const u8).add(n) as *const DevicePathProtocol;
        }
    }
    let name = ucs2(&backslash(path));
    let node = 4 + name.len() * 2;
    out.extend_from_slice(&[DP_TYPE_MEDIA, DP_SUBTYPE_MEDIA_FILE_PATH]);
    out.extend_from_slice(&(node as u16).to_le_bytes());
    for w in &name {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out.extend_from_slice(&[DP_TYPE_END, DP_SUBTYPE_END_ENTIRE, 4, 0]);
    Some(out)
}
