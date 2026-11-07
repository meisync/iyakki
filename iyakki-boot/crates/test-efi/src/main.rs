// SPDX-License-Identifier: MIT OR Apache-2.0
//! A tiny UEFI program for the QEMU tests. It stands in for a Linux kernel (it reports its own path, its load options and the size
//! of the initrd offered through the initrd protocol) or for another system's loader. If its load options contain `reset`, it
//! restarts the machine (a "kernel" that never confirms a good start); otherwise it returns to the loader.
#![no_std]
#![no_main]

use core::ffi::c_void;
use core::fmt::Write;
use core::ptr::null_mut;
use uefi_sys::*;

struct Out(*mut SimpleTextOutput);

impl Write for Out {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let mut buf = [0u16; 96];
        let mut n = 0;
        for ch in s.chars() {
            let mut tmp = [0u16; 2];
            if ch == '\n' {
                buf[n] = 13;
                n += 1;
            }
            for &w in ch.encode_utf16(&mut tmp).iter() {
                buf[n] = w;
                n += 1;
            }
            if n >= 90 {
                buf[n] = 0;
                unsafe { ((*self.0).output_string)(self.0, buf.as_ptr()) };
                n = 0;
            }
        }
        buf[n] = 0;
        unsafe { ((*self.0).output_string)(self.0, buf.as_ptr()) };
        Ok(())
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {}
}

#[no_mangle]
pub extern "efiapi" fn efi_main(image: Handle, st: *mut SystemTable) -> Status {
    let st = unsafe { &*st };
    let bs = unsafe { &*st.boot_services };
    let mut out = Out(st.con_out);
    let mut li: *mut c_void = null_mut();
    unsafe { (bs.handle_protocol)(image, &LOADED_IMAGE_PROTOCOL_GUID, &mut li) };
    let li = li as *mut LoadedImage;

    // own path, last file-path node
    let mut path = [0u16; 128];
    let mut plen = 0;
    if !li.is_null() {
        let mut p = unsafe { (*li).file_path } as *const DevicePathProtocol;
        unsafe {
            while !p.is_null() && !(*p).is_end_entire() {
                let n = (*p).node_length() as usize;
                if n < 4 {
                    break;
                }
                if (*p).ty == DP_TYPE_MEDIA && (*p).sub_type == DP_SUBTYPE_MEDIA_FILE_PATH {
                    let w = core::slice::from_raw_parts((p as *const u8).add(4) as *const u16, (n - 4) / 2);
                    plen = 0;
                    for &c in w.iter().take_while(|&&c| c != 0).take(127) {
                        path[plen] = c;
                        plen += 1;
                    }
                }
                p = (p as *const u8).add(n) as *const DevicePathProtocol;
            }
        }
    }
    let _ = write!(out, "TEST-EFI started path=");
    for &c in &path[..plen] {
        let _ = out.write_char(char::from_u32(c as u32).unwrap_or('?'));
    }

    // load options (UCS-2)
    let mut reset = false;
    let mut confirm = false;
    let _ = write!(out, " options=[");
    if !li.is_null() && unsafe { (*li).load_options_size } > 0 {
        let n = unsafe { (*li).load_options_size } as usize / 2;
        let w = unsafe { core::slice::from_raw_parts((*li).load_options as *const u16, n) };
        let mut text = [0u8; 256];
        let mut tl = 0;
        for &c in w.iter().take_while(|&&c| c != 0).take(255) {
            let b = if c < 128 { c as u8 } else { b'?' };
            text[tl] = b;
            tl += 1;
            let _ = out.write_char(b as char);
        }
        reset = text[..tl].windows(5).any(|x| x == b"reset");
        confirm = text[..tl].windows(7).any(|x| x == b"confirm");
    }
    let _ = write!(out, "]");

    // the initrd as the Linux EFI stub would ask for it
    let mut dp = InitrdDevicePath::VALUE;
    let mut dpp = &mut dp as *mut InitrdDevicePath as *mut DevicePathProtocol;
    let mut dev: Handle = null_mut();
    let s = unsafe { (bs.locate_device_path)(&LOAD_FILE2_PROTOCOL_GUID, &mut dpp, &mut dev) };
    if s.is_error() {
        let _ = write!(out, " initrd=none");
    } else {
        let mut lf: *mut c_void = null_mut();
        unsafe { (bs.handle_protocol)(dev, &LOAD_FILE2_PROTOCOL_GUID, &mut lf) };
        let lf = lf as *mut LoadFile2;
        let mut size = 0usize;
        let s = unsafe { ((*lf).load_file)(lf, dpp, 0, &mut size, null_mut()) };
        if s == Status::BUFFER_TOO_SMALL && size > 0 {
            let mut buf: *mut c_void = null_mut();
            unsafe { (bs.allocate_pool)(MemoryType::LoaderData, size, &mut buf) };
            let s = unsafe { ((*lf).load_file)(lf, dpp, 0, &mut size, buf) };
            let mut sum = 0u32;
            if !s.is_error() {
                for i in 0..size {
                    sum = sum.wrapping_mul(31).wrapping_add(unsafe { *(buf as *const u8).add(i) } as u32);
                }
            }
            let _ = write!(out, " initrd={size} sum={sum:08x}");
        } else {
            let _ = write!(out, " initrd=empty");
        }
    }
    let _ = write!(out, "\n");
    if confirm {
        // what the running system does once it is up: write the loader's confirm variable (non-volatile, boot + runtime access)
        let name: [u16; 12] = [66, 111, 111, 116, 67, 111, 110, 102, 105, 114, 109, 0]; // "BootConfirm"
        let vendor = Guid::new(0x7b5a3c1e, 0x2d84, 0x4f6b, [0x9a, 0x0e, 0x5c, 0x1f, 0x8d, 0x3b, 0x2a, 0x47]);
        let s = unsafe { ((*st.runtime_services).set_variable)(name.as_ptr(), &vendor, 7, 2, b"ok".as_ptr() as *const c_void) };
        let _ = write!(out, "TEST-EFI confirm written: {}\n", if s.is_error() { "failed" } else { "ok" });
    }
    if reset {
        unsafe { ((*st.runtime_services).reset_system)(ResetType::Cold, Status::SUCCESS, 0, core::ptr::null()) };
    }
    Status::SUCCESS
}
