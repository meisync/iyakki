// SPDX-License-Identifier: MIT OR Apache-2.0
//! Firmware access: the global tables, the allocator, text output, protocol lookup, small helpers.

use alloc::string::String;
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::fmt::{self, Write};
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use uefi_sys::*;

static SYSTEM_TABLE: AtomicPtr<SystemTable> = AtomicPtr::new(null_mut());
static IMAGE: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

/// Remember the tables handed to the entry point.
pub fn init(image: Handle, st: *mut SystemTable) {
    IMAGE.store(image, Ordering::Relaxed);
    SYSTEM_TABLE.store(st, Ordering::Relaxed);
}

/// This image's handle.
pub fn image() -> Handle {
    IMAGE.load(Ordering::Relaxed)
}

/// The system table (the program is useless without it, so this assumes `init` ran).
pub fn st() -> &'static SystemTable {
    unsafe { &*SYSTEM_TABLE.load(Ordering::Relaxed) }
}

/// Boot services.
pub fn bs() -> &'static BootServices {
    unsafe { &*st().boot_services }
}

/// Runtime services.
pub fn rt() -> &'static RuntimeServices {
    unsafe { &*st().runtime_services }
}

// ---- allocator ---------------------------------------------------------------------------------------------------------------

/// The global allocator: UEFI pool memory. Pool memory is 8-byte aligned; larger alignments over-allocate and keep the original
/// pointer just below the aligned block.
pub struct Pool;

unsafe impl GlobalAlloc for Pool {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let st = SYSTEM_TABLE.load(Ordering::Relaxed);
        if st.is_null() {
            return null_mut();
        }
        let bs = &*(*st).boot_services;
        let mut p: *mut c_void = null_mut();
        if l.align() <= 8 {
            if (bs.allocate_pool)(MemoryType::LoaderData, l.size().max(1), &mut p).is_error() {
                return null_mut();
            }
            return p as *mut u8;
        }
        if (bs.allocate_pool)(MemoryType::LoaderData, l.size() + l.align() + 8, &mut p).is_error() {
            return null_mut();
        }
        let aligned = (p as usize + 8 + l.align() - 1) & !(l.align() - 1);
        *((aligned - 8) as *mut usize) = p as usize;
        aligned as *mut u8
    }

    unsafe fn dealloc(&self, ptr: *mut u8, l: Layout) {
        let st = SYSTEM_TABLE.load(Ordering::Relaxed);
        if st.is_null() {
            return;
        }
        let orig = if l.align() <= 8 { ptr as usize } else { *((ptr as usize - 8) as *const usize) };
        let _ = ((*(*st).boot_services).free_pool)(orig as *mut c_void);
    }
}

// ---- text output -------------------------------------------------------------------------------------------------------------

/// Writes to the firmware text console (which OVMF and most firmware also copy to the serial port). Converts `\n` to `\r\n`.
pub struct Console;

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut buf = [0u16; 128];
        let mut n = 0;
        let flush = |buf: &mut [u16; 128], n: &mut usize| {
            if *n > 0 {
                buf[*n] = 0;
                unsafe { ((*st().con_out).output_string)(st().con_out, buf.as_ptr()) };
                *n = 0;
            }
        };
        for ch in s.chars() {
            if ch == '\n' {
                buf[n] = 13;
                n += 1;
            }
            let mut u = [0u16; 2];
            let enc = ch.encode_utf16(&mut u);
            for &w in enc.iter() {
                buf[n] = w;
                n += 1;
            }
            if n >= 120 {
                flush(&mut buf, &mut n);
            }
        }
        flush(&mut buf, &mut n);
        Ok(())
    }
}

/// While true, `say!` leaves the text console alone (it would draw over the splash picture) and writes to the serial port only.
static QUIET: AtomicBool = AtomicBool::new(false);
/// The serial port was tried and failed (or does not exist): do not try again.
static SERIAL_DEAD: AtomicBool = AtomicBool::new(false);

/// Keep log lines off the screen (`true`) or print them on the text console again (`false`).
pub fn set_quiet(on: bool) {
    QUIET.store(on, Ordering::Relaxed);
}

/// Every `say!` line is also kept here, so the loader can write the whole run to a file on the boot volume (a laptop has no serial
/// port and the splash hides the console). 96 KB holds the device summary, a long scan and the menu many times over.
struct LogSlot(UnsafeCell<boot_core::log::LogBuffer>);
// SAFETY: the loader is single threaded (boot services, one processor); each use is short and no two references are alive at once.
unsafe impl Sync for LogSlot {}
static LOG: LogSlot = LogSlot(UnsafeCell::new(boot_core::log::LogBuffer::new(96 * 1024)));

/// A copy of the log so far.
pub fn log_bytes() -> Vec<u8> {
    // SAFETY: see `LogSlot`.
    unsafe { (*LOG.0.get()).bytes().to_vec() }
}

/// Keeps the formatted text in the log buffer.
struct LogSink;

impl Write for LogSink {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // SAFETY: see `LogSlot`.
        unsafe { (*LOG.0.get()).push_str(s) };
        Ok(())
    }
}

/// Log text without touching the screen: straight to the first serial port the firmware offers. A write that fails or is cut short
/// switches the serial log off for the rest of the run, so a port with nothing behind it can never slow the start.
struct SerialLog;

impl Write for SerialLog {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if SERIAL_DEAD.load(Ordering::Relaxed) {
            return Ok(());
        }
        let Some(port) = locate::<SerialIo>(&SERIAL_IO_PROTOCOL_GUID) else {
            SERIAL_DEAD.store(true, Ordering::Relaxed);
            return Ok(());
        };
        let mut out: Vec<u8> = Vec::with_capacity(s.len() + 8);
        for b in s.bytes() {
            if b == b'\n' {
                out.push(b'\r');
            }
            out.push(b);
        }
        let mut size = out.len();
        let st = unsafe { ((*port).write)(port, &mut size, out.as_ptr() as *const c_void) };
        if st.is_error() || size != out.len() {
            SERIAL_DEAD.store(true, Ordering::Relaxed);
        }
        Ok(())
    }
}

/// Print to the text console (or only to the serial port while `set_quiet(true)`).
pub fn print_fmt(args: fmt::Arguments<'_>) {
    if SYSTEM_TABLE.load(Ordering::Relaxed).is_null() {
        return;
    }
    let _ = LogSink.write_fmt(args);
    if QUIET.load(Ordering::Relaxed) {
        let _ = SerialLog.write_fmt(args);
    } else {
        let _ = Console.write_fmt(args);
    }
}

/// `say!("text {}", x)`: print to the text console (and so to the serial port on most firmware).
#[macro_export]
macro_rules! say {
    ($($arg:tt)*) => { $crate::sys::print_fmt(format_args!($($arg)*)) };
}

// ---- helpers -----------------------------------------------------------------------------------------------------------------

/// Pause (microseconds).
pub fn stall_us(us: usize) {
    unsafe { (bs().stall)(us) };
}

/// UCS-2 with a trailing NUL.
pub fn ucs2(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

/// UCS-2 (no NUL needed) to a `String`.
pub fn from_ucs2(w: &[u16]) -> String {
    char::decode_utf16(w.iter().copied().take_while(|&c| c != 0)).map(|r| r.unwrap_or('?')).collect()
}

/// A protocol interface of a handle, opened for reading only (nothing is claimed).
pub fn protocol<T>(handle: Handle, guid: &Guid) -> Option<*mut T> {
    let mut p: *mut c_void = null_mut();
    let s = unsafe { (bs().open_protocol)(handle, guid, &mut p, image(), null_mut(), OPEN_PROTOCOL_GET_PROTOCOL) };
    if s.is_error() || p.is_null() {
        None
    } else {
        Some(p as *mut T)
    }
}

/// The first interface of a protocol anywhere in the firmware.
pub fn locate<T>(guid: &Guid) -> Option<*mut T> {
    let mut p: *mut c_void = null_mut();
    let s = unsafe { (bs().locate_protocol)(guid, null_mut(), &mut p) };
    if s.is_error() || p.is_null() {
        None
    } else {
        Some(p as *mut T)
    }
}

/// Every handle that has a protocol.
pub fn handles_with(guid: &Guid) -> Vec<Handle> {
    let mut count = 0usize;
    let mut buf: *mut Handle = null_mut();
    let s = unsafe { (bs().locate_handle_buffer)(LocateSearchType::ByProtocol, guid, null_mut(), &mut count, &mut buf) };
    if s.is_error() || buf.is_null() {
        return Vec::new();
    }
    let v = unsafe { core::slice::from_raw_parts(buf, count).to_vec() };
    unsafe { (bs().free_pool)(buf as *mut c_void) };
    v
}

/// Ask the firmware to start drivers on every device (some firmware connects only the boot disk, so other disks would not show up).
pub fn connect_all() {
    let mut count = 0usize;
    let mut buf: *mut Handle = null_mut();
    let s = unsafe { (bs().locate_handle_buffer)(LocateSearchType::AllHandles, core::ptr::null(), null_mut(), &mut count, &mut buf) };
    if s.is_error() || buf.is_null() {
        return;
    }
    for i in 0..count {
        unsafe { (bs().connect_controller)(*buf.add(i), null_mut(), null_mut(), 1) };
    }
    unsafe { (bs().free_pool)(buf as *mut c_void) };
}

/// Is Secure Boot on right now?
pub fn secure_boot() -> bool {
    matches!(crate::vars::get_in("SecureBoot", &GLOBAL_VARIABLE_GUID).as_deref(), Some([1]))
}

/// Restart the machine.
pub fn reset() -> ! {
    unsafe { (rt().reset_system)(ResetType::Cold, Status::SUCCESS, 0, core::ptr::null()) }
}
