// SPDX-License-Identifier: MIT OR Apache-2.0
//! UEFI variables under this project's own vendor GUID (not tied to any distribution).

use crate::sys::{rt, ucs2};
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr::null_mut;
use uefi_sys::*;

/// Vendor GUID of the variables below.
pub const VENDOR: Guid = Guid::new(0x7b5a3c1e, 0x2d84, 0x4f6b, [0x9a, 0x0e, 0x5c, 0x1f, 0x8d, 0x3b, 0x2a, 0x47]);
/// The boot-tries state (`boot_core::assess::STATE_LEN` bytes). The running system rewrites it to confirm a good start.
pub const TRIES: &str = "BootTries";
/// The one-shot variable: an item id (UTF-8) to start once. The loader deletes it before it starts anything.
pub const ONESHOT: &str = "BootOnce";

/// The running system writes this (any content, `ok` by convention) once a start is confirmed good, after the sign-in screen is drawn.
/// The loader reads it at the next start, refills the tries (`BootState::mark_good`) and deletes it.
pub const CONFIRM: &str = "BootConfirm";

const ATTR: u32 = VARIABLE_NON_VOLATILE | VARIABLE_BOOTSERVICE_ACCESS | VARIABLE_RUNTIME_ACCESS;

/// Read a variable of any vendor.
pub fn get_in(name: &str, vendor: &Guid) -> Option<Vec<u8>> {
    let n = ucs2(name);
    let mut size = 0usize;
    let s = unsafe { (rt().get_variable)(n.as_ptr(), vendor, null_mut(), &mut size, null_mut()) };
    if s != Status::BUFFER_TOO_SMALL || size == 0 {
        return None;
    }
    let mut v = alloc::vec![0u8; size];
    let s = unsafe { (rt().get_variable)(n.as_ptr(), vendor, null_mut(), &mut size, v.as_mut_ptr() as *mut c_void) };
    if s.is_error() {
        return None;
    }
    v.truncate(size);
    Some(v)
}

/// Read one of ours.
pub fn get(name: &str) -> Option<Vec<u8>> {
    get_in(name, &VENDOR)
}

/// Write one of ours (non-volatile).
pub fn set(name: &str, data: &[u8]) -> bool {
    let n = ucs2(name);
    unsafe { !(rt().set_variable)(n.as_ptr(), &VENDOR, ATTR, data.len(), data.as_ptr() as *const c_void).is_error() }
}

/// Delete one of ours.
pub fn delete(name: &str) {
    let n = ucs2(name);
    unsafe { (rt().set_variable)(n.as_ptr(), &VENDOR, 0, 0, core::ptr::null()) };
}

/// Can the firmware restart into its own setup screen (`OsIndicationsSupported` bit 0)?
pub fn firmware_setup_supported() -> bool {
    get_in("OsIndicationsSupported", &GLOBAL_VARIABLE_GUID).is_some_and(|v| v.first().is_some_and(|b| b & 1 == 1))
}

/// Ask the firmware to show its setup screen at the next start (`OsIndications` bit 0, other bits kept).
pub fn request_firmware_setup() -> bool {
    let mut cur = get_in("OsIndications", &GLOBAL_VARIABLE_GUID).unwrap_or_default();
    cur.resize(8, 0);
    cur[0] |= 1;
    let n = ucs2("OsIndications");
    unsafe { !(rt().set_variable)(n.as_ptr(), &GLOBAL_VARIABLE_GUID, ATTR, cur.len(), cur.as_ptr() as *const c_void).is_error() }
}
