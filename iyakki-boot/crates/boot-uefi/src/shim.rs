// SPDX-License-Identifier: MIT OR Apache-2.0
//! Starting images under shim. When shim started this loader, the firmware's own check (`db`/`dbx`) does not know the owner's key:
//! that key lives in shim's MOK list. Shim's own second stage (GRUB, or this loader) is checked by shim, but an image THIS loader
//! starts with `LoadImage` goes to the firmware, which would answer "access denied" for a kernel signed with a MOK key.
//!
//! So, for as long as the loader starts something, the firmware's two security hooks are wrapped (the same method systemd-boot
//! uses): the original check runs first and its answer stands when it says yes; when it says no and shim's lock protocol is
//! present, shim gets to look at the bytes (`ShimLock.Verify`: MOK list, shim's certificate, SBAT) and its yes also lets the image
//! load (only the byte-based hook is wrapped: the path-based one has no bytes to show shim). Nothing is ever allowed that neither the firmware nor shim trusts. Without shim, or with Secure Boot off, nothing is installed.

use crate::say;
use crate::sys::locate;
use core::cell::UnsafeCell;
use core::ffi::c_void;
use uefi_sys::*;

struct State {
    lock: *mut ShimLock,
    sec2: *mut Security2Arch,
    orig2: Option<unsafe extern "efiapi" fn(*const Security2Arch, *const DevicePathProtocol, *mut c_void, usize, Boolean) -> Status>,
}

struct Slot(UnsafeCell<State>);
// SAFETY: the loader is single threaded (boot services, one processor); the hooks run on the same thread inside `LoadImage`.
unsafe impl Sync for Slot {}
static STATE: Slot = Slot(UnsafeCell::new(State { lock: core::ptr::null_mut(), sec2: core::ptr::null_mut(), orig2: None }));

fn state() -> &'static mut State {
    // SAFETY: see `Slot`; each use is short and no two references are alive at once.
    unsafe { &mut *STATE.0.get() }
}

unsafe extern "efiapi" fn hook2(this: *const Security2Arch, file: *const DevicePathProtocol, buffer: *mut c_void, size: usize, boot_policy: Boolean) -> Status {
    let st = state();
    let orig = match st.orig2 {
        Some(f) => f(this, file, buffer, size, boot_policy),
        None => Status::SECURITY_VIOLATION,
    };
    if !orig.is_error() || buffer.is_null() || st.lock.is_null() || size > u32::MAX as usize {
        return orig;
    }
    if ((*st.lock).verify)(buffer, size as u32) == Status::SUCCESS {
        return Status::SUCCESS;
    }
    orig
}

/// Wrap the firmware's checks so shim's MOK list counts. Returns whether anything was installed. Call `remove` when done.
pub fn install(secure_boot: bool) -> bool {
    if !secure_boot {
        return false;
    }
    let st = state();
    let Some(lock) = locate::<ShimLock>(&SHIM_LOCK_PROTOCOL_GUID) else {
        say!("shim: no lock protocol (not started by shim): the firmware's own check alone applies\n");
        return false;
    };
    st.lock = lock;
    if let Some(p) = locate::<Security2Arch>(&SECURITY2_ARCH_PROTOCOL_GUID) {
        unsafe {
            st.sec2 = p;
            st.orig2 = Some((*p).file_authentication);
            (*p).file_authentication = hook2;
        }
    }
    say!("shim: lock protocol found; MOK-signed images are accepted for this start\n");
    !st.sec2.is_null()
}

/// Put the firmware's own checks back.
pub fn remove() {
    let st = state();
    unsafe {
        if let (false, Some(o)) = (st.sec2.is_null(), st.orig2) {
            (*st.sec2).file_authentication = o;
        }
    }
    st.sec2 = core::ptr::null_mut();
    st.orig2 = None;
    st.lock = core::ptr::null_mut();
}
