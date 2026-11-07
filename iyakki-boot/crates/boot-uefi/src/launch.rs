// SPDX-License-Identifier: MIT OR Apache-2.0
//! Start another EFI image with `LoadImage` / `StartImage`: a Linux kernel (EFI stub) with its command line and initrd, or any
//! other loader. The firmware does the signature check; this file never skips it.

use crate::fs::file_device_path;
use crate::sys::{bs, image, protocol, ucs2};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr::null_mut;
use uefi_sys::*;

/// The initrd as the Linux EFI stub asks for it: a `LoadFile2` interface on a handle that also carries the fixed vendor device path.
#[repr(C)]
struct InitrdFile {
    proto: LoadFile2,
    data: Vec<u8>,
}

unsafe extern "efiapi" fn load_initrd(
    this: *mut LoadFile2,
    _path: *const DevicePathProtocol,
    boot_policy: Boolean,
    size: *mut usize,
    buf: *mut c_void,
) -> Status {
    if boot_policy != 0 {
        return Status::UNSUPPORTED;
    }
    if size.is_null() || this.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let f = &*(this as *const InitrdFile);
    if buf.is_null() || *size < f.data.len() {
        *size = f.data.len();
        return Status::BUFFER_TOO_SMALL;
    }
    core::ptr::copy_nonoverlapping(f.data.as_ptr(), buf as *mut u8, f.data.len());
    *size = f.data.len();
    Status::SUCCESS
}

struct InitrdHandle {
    handle: Handle,
    path: *mut InitrdDevicePath,
    file: *mut InitrdFile,
}

fn install_initrd(data: Vec<u8>) -> Option<InitrdHandle> {
    let path = Box::into_raw(Box::new(InitrdDevicePath::VALUE));
    let file = Box::into_raw(Box::new(InitrdFile { proto: LoadFile2 { load_file: load_initrd }, data }));
    let mut handle: Handle = null_mut();
    unsafe {
        let a = (bs().install_protocol_interface)(&mut handle, &DEVICE_PATH_PROTOCOL_GUID, 0, path as *mut c_void);
        let b = if a.is_error() { a } else { (bs().install_protocol_interface)(&mut handle, &LOAD_FILE2_PROTOCOL_GUID, 0, file as *mut c_void) };
        if b.is_error() {
            if !a.is_error() {
                (bs().uninstall_protocol_interface)(handle, &DEVICE_PATH_PROTOCOL_GUID, path as *mut c_void);
            }
            drop(Box::from_raw(path));
            drop(Box::from_raw(file));
            return None;
        }
    }
    Some(InitrdHandle { handle, path, file })
}

fn remove_initrd(h: InitrdHandle) {
    unsafe {
        (bs().uninstall_protocol_interface)(h.handle, &LOAD_FILE2_PROTOCOL_GUID, h.file as *mut c_void);
        (bs().uninstall_protocol_interface)(h.handle, &DEVICE_PATH_PROTOCOL_GUID, h.path as *mut c_void);
        drop(Box::from_raw(h.path));
        drop(Box::from_raw(h.file));
    }
}

/// Load the image at `path` on the volume `volume` and start it. `options` becomes its load options (UCS-2: the kernel's command
/// line); `initrd` is offered through the initrd protocol while it runs. `Ok(status)` means the image came back (a kernel
/// that started never does); `Err(text)` means it could not be loaded or prepared.
pub fn start(volume: Handle, path: &str, options: Option<&str>, initrd: Option<Vec<u8>>) -> Result<Status, String> {
    // under shim the firmware's check alone refuses a kernel signed with the owner's MOK key: let shim look too (see shim.rs)
    let hooked = crate::shim::install(crate::sys::secure_boot());
    let r = start_inner(volume, path, options, initrd);
    if hooked {
        crate::shim::remove();
    }
    r
}

fn start_inner(volume: Handle, path: &str, options: Option<&str>, initrd: Option<Vec<u8>>) -> Result<Status, String> {
    let dp = file_device_path(volume, path).ok_or_else(|| String::from("cannot build the device path of the file"))?;
    let mut img: Handle = null_mut();
    let s = unsafe { (bs().load_image)(0, image(), dp.as_ptr() as *const DevicePathProtocol, core::ptr::null(), 0, &mut img) };
    if s.is_error() {
        return Err(alloc::format!("LoadImage {path}: {}", status_text(s)));
    }
    let opts: Vec<u16> = options.map(ucs2).unwrap_or_default();
    if let Some(li) = protocol::<LoadedImage>(img, &LOADED_IMAGE_PROTOCOL_GUID) {
        if options.is_some() {
            unsafe {
                (*li).load_options = opts.as_ptr() as *mut c_void;
                (*li).load_options_size = (opts.len() * 2) as u32;
            }
        }
    }
    let ih = match initrd {
        Some(d) => match install_initrd(d) {
            Some(h) => Some(h),
            None => {
                unsafe { (bs().unload_image)(img) };
                return Err(String::from("cannot publish the initrd"));
            }
        },
        None => None,
    };
    unsafe { (bs().set_watchdog_timer)(0, 0, 0, core::ptr::null()) };
    let mut exit_size = 0usize;
    let mut exit_data: *mut Char16 = null_mut();
    let s = unsafe { (bs().start_image)(img, &mut exit_size, &mut exit_data) };
    if !exit_data.is_null() {
        unsafe { (bs().free_pool)(exit_data as *mut c_void) };
    }
    if let Some(h) = ih {
        remove_initrd(h);
    }
    unsafe { (bs().unload_image)(img) };
    drop(opts);
    Ok(s)
}

/// Short text for a status.
pub fn status_text(s: Status) -> String {
    let name = match s {
        Status::SUCCESS => "success",
        Status::LOAD_ERROR => "load error",
        Status::INVALID_PARAMETER => "invalid parameter",
        Status::UNSUPPORTED => "unsupported",
        Status::OUT_OF_RESOURCES => "out of resources",
        Status::NOT_FOUND => "not found",
        Status::ACCESS_DENIED => "access denied",
        Status::SECURITY_VIOLATION => "security violation (not signed for this machine)",
        Status::ABORTED => "aborted",
        Status::DEVICE_ERROR => "device error",
        Status::VOLUME_CORRUPTED => "volume corrupted",
        _ => "error",
    };
    alloc::format!("{name} (0x{:x})", s.0)
}
