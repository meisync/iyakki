//! The plain `extern "C"` surface for the page's hand-written glue (`js/engine.js`). Only built for wasm32: no wasm-bindgen, no web-sys.
//!
//! Memory is the module's own linear memory: the host asks for a buffer with `iy_alloc`, writes bytes, calls a function with the pointer
//! and length, and frees it with `iy_free` (a password buffer is overwritten by the host first). Results are read back as a pointer and a
//! length from `iy_reply_ptr`. All calls come from one thread; the state is a thread-local, so there is no `static mut`.

#![allow(unsafe_code)]

use crate::host::Host;
use std::cell::RefCell;

thread_local! {
    static HOST: RefCell<Host> = RefCell::new(Host::new());
}

fn with<R>(f: impl FnOnce(&mut Host) -> R) -> R {
    HOST.with(|h| f(&mut h.borrow_mut()))
}

unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if ptr.is_null() || len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(ptr, len)
    }
}

unsafe fn text<'a>(ptr: *const u8, len: usize) -> &'a str {
    std::str::from_utf8(bytes(ptr, len)).unwrap_or("")
}

#[no_mangle]
pub extern "C" fn iy_alloc(len: usize) -> *mut u8 {
    let mut v = vec![0u8; len.max(1)];
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety
/// `ptr` and `len` must be exactly what `iy_alloc` returned and was asked for.
#[no_mangle]
pub unsafe extern "C" fn iy_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        drop(Vec::from_raw_parts(ptr, len.max(1), len.max(1)));
    }
}

/// # Safety
/// `ptr` and `len` must describe bytes the host wrote into a buffer from `iy_alloc`.
#[no_mangle]
pub unsafe extern "C" fn iy_init(ptr: *const u8, len: usize) -> i32 {
    with(|h| h.init(text(ptr, len)) as i32)
}

/// # Safety
/// As `iy_init`.
#[no_mangle]
pub unsafe extern "C" fn iy_cmd(ptr: *const u8, len: usize) -> i32 {
    with(|h| h.command(text(ptr, len)))
}

/// # Safety
/// As `iy_init`.
#[no_mangle]
pub unsafe extern "C" fn iy_on_text(ptr: *const u8, len: usize) {
    with(|h| h.on_text(text(ptr, len)))
}

/// # Safety
/// As `iy_init`.
#[no_mangle]
pub unsafe extern "C" fn iy_on_binary(ptr: *const u8, len: usize) {
    with(|h| h.on_binary(bytes(ptr, len)))
}

/// The next record for the host as JSON in the reply buffer; its length, or 0 when there is none.
#[no_mangle]
pub extern "C" fn iy_next_out() -> usize {
    with(Host::next_out)
}

#[no_mangle]
pub extern "C" fn iy_reply_ptr() -> *const u8 {
    with(|h| h.reply().as_ptr())
}

#[no_mangle]
pub extern "C" fn iy_fb_ptr() -> *const u8 {
    with(|h| h.framebuffer().0)
}

#[no_mangle]
pub extern "C" fn iy_fb_w() -> usize {
    with(|h| h.framebuffer().1)
}

#[no_mangle]
pub extern "C" fn iy_fb_h() -> usize {
    with(|h| h.framebuffer().2)
}

/// A number that changes whenever the framebuffer does (it fits a double exactly).
#[no_mangle]
pub extern "C" fn iy_fb_version() -> f64 {
    with(|h| h.framebuffer().3 as f64)
}

/// The display-list picture (commands `ui_load` and friends): pointer, width, height and a number that changes with each load.
#[no_mangle]
pub extern "C" fn iy_ui_fb_ptr() -> *const u8 {
    with(|h| h.ui_framebuffer().0)
}

#[no_mangle]
pub extern "C" fn iy_ui_fb_w() -> usize {
    with(|h| h.ui_framebuffer().1)
}

#[no_mangle]
pub extern "C" fn iy_ui_fb_h() -> usize {
    with(|h| h.ui_framebuffer().2)
}

#[no_mangle]
pub extern "C" fn iy_ui_fb_version() -> f64 {
    with(|h| h.ui_framebuffer().3 as f64)
}
