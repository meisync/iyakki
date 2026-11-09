// SPDX-License-Identifier: MIT OR Apache-2.0
//! The device summary written into the boot log at every start: what the firmware says this machine is (SMBIOS), the firmware
//! itself, memory, every PCI function, every USB device, the disks and partitions, the file systems, the screen and the input
//! devices. Read only: nothing is changed, no driver is connected, no mode is set. The point is a stick that came back from a
//! machine that did not start: the log names the machine and shows what its firmware exposed to the loader.

use crate::say;
use crate::sys::{bs, from_ucs2, handles_with, locate, protocol, st};
use alloc::string::String;
use boot_core::hw::{pci_class_name, smbios_string, MemoryTotals};
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr::null_mut;
use uefi_sys::*;

/// Write the summary (`hw:` lines) to the log.
/// `save` writes the log so far after every section: a firmware that faults inside one of these walks (a MacBook Pro 16,2 showed no
/// logo and left no log, 2026-11-08) then leaves the sections before it on the volume, and the last line names the one that stopped.
pub fn summary(save: &dyn Fn(&str)) {
    firmware();
    save("firmware listed; next: SMBIOS");
    smbios();
    save("machine listed; next: memory map");
    memory();
    save("memory listed; next: PCI");
    pci();
    save("PCI listed; next: USB");
    usb();
    save("USB listed; next: disks");
    disks();
    save("disks listed; next: screen");
    screen();
    save("screen listed; next: input");
    input();
}

fn firmware() {
    let t = st();
    let vendor = if t.firmware_vendor.is_null() { String::from("?") } else { unsafe { cstr16(t.firmware_vendor) } };
    let spec = t.hdr.revision;
    say!(
        "hw: firmware \"{vendor}\" revision 0x{:x}, UEFI {}.{}, secure boot {}, setup mode {}, TPM 2.0 protocol {}\n",
        t.firmware_revision,
        spec >> 16,
        (spec & 0xffff) / 10,
        if crate::sys::secure_boot() { "on" } else { "off" },
        match crate::vars::get_in("SetupMode", &GLOBAL_VARIABLE_GUID).as_deref() {
            Some([1]) => "on",
            Some([0]) => "off",
            _ => "?",
        },
        if locate::<c_void>(&TCG2_PROTOCOL_GUID).is_some() { "present" } else { "absent" }
    );
}

/// A NUL-terminated UCS-2 string.
unsafe fn cstr16(p: *const Char16) -> String {
    let mut n = 0;
    while *p.add(n) != 0 && n < 512 {
        n += 1;
    }
    from_ucs2(core::slice::from_raw_parts(p, n))
}

fn config_table(guid: &Guid) -> Option<*mut c_void> {
    let t = st();
    for i in 0..t.number_of_table_entries {
        let e = unsafe { &*t.configuration_table.add(i) };
        if &e.vendor_guid == guid {
            return Some(e.vendor_table);
        }
    }
    None
}

/// The SMBIOS structure table: (address, length).
fn smbios_table() -> Option<(*const u8, usize)> {
    if let Some(ep) = config_table(&SMBIOS3_TABLE_GUID) {
        let ep = ep as *const u8;
        // "_SM3_": table maximum size at 0x0c (u32), table address at 0x10 (u64)
        if unsafe { core::slice::from_raw_parts(ep, 5) } == b"_SM3_" {
            let len = unsafe { u32::from_le_bytes(*(ep.add(0x0c) as *const [u8; 4])) } as usize;
            let addr = unsafe { u64::from_le_bytes(*(ep.add(0x10) as *const [u8; 8])) } as usize;
            if addr != 0 && len > 4 {
                return Some((addr as *const u8, len.min(1 << 20)));
            }
        }
    }
    if let Some(ep) = config_table(&SMBIOS_TABLE_GUID) {
        let ep = ep as *const u8;
        // "_SM_": structure table length at 0x16 (u16), address at 0x18 (u32)
        if unsafe { core::slice::from_raw_parts(ep, 4) } == b"_SM_" {
            let len = unsafe { u16::from_le_bytes(*(ep.add(0x16) as *const [u8; 2])) } as usize;
            let addr = unsafe { u32::from_le_bytes(*(ep.add(0x18) as *const [u8; 4])) } as usize;
            if addr != 0 && len > 4 {
                return Some((addr as *const u8, len));
            }
        }
    }
    None
}

fn smbios() {
    let Some((base, len)) = smbios_table() else {
        say!("hw: machine: no SMBIOS table\n");
        return;
    };
    let all = unsafe { core::slice::from_raw_parts(base, len) };
    let mut off = 0;
    let mut shown = 0;
    while off + 4 <= all.len() && shown < 3 {
        let ty = all[off];
        let fixed = all[off + 1] as usize;
        if fixed < 4 {
            break;
        }
        // the formatted part, then strings up to a double NUL
        let mut end = off + fixed;
        while end + 1 < all.len() && !(all[end] == 0 && all[end + 1] == 0) {
            end += 1;
        }
        end = (end + 2).min(all.len());
        let body = &all[off..end];
        let s = |n: usize| if n < fixed { smbios_string(body, fixed, body[n]) } else { String::new() };
        match ty {
            0 => say!("hw: BIOS \"{}\" version \"{}\" date {}\n", s(4), s(5), s(8)),
            1 => say!("hw: machine \"{}\" product \"{}\" version \"{}\" family \"{}\"\n", s(4), s(5), s(6), s(0x1a)),
            2 => say!("hw: board \"{}\" product \"{}\" version \"{}\"\n", s(4), s(5), s(6)),
            _ => {}
        }
        if matches!(ty, 0 | 1 | 2) {
            shown += 1;
        }
        if ty == 127 {
            break;
        }
        off = end;
    }
}

fn memory() {
    let mut size = 0usize;
    let mut key = 0usize;
    let mut desc_size = 0usize;
    let mut ver = 0u32;
    unsafe { (bs().get_memory_map)(&mut size, null_mut(), &mut key, &mut desc_size, &mut ver) };
    if size == 0 || desc_size == 0 {
        say!("hw: memory: the map could not be read\n");
        return;
    }
    size += desc_size * 8;
    let mut buf = alloc::vec![0u8; size];
    let s = unsafe { (bs().get_memory_map)(&mut size, buf.as_mut_ptr() as *mut MemoryDescriptor, &mut key, &mut desc_size, &mut ver) };
    if s.is_error() {
        say!("hw: memory: the map could not be read ({})\n", crate::launch::status_text(s));
        return;
    }
    let mut m = MemoryTotals::default();
    let mut off = 0;
    while off + desc_size <= size {
        let d = unsafe { &*(buf.as_ptr().add(off) as *const MemoryDescriptor) };
        m.add(d.ty as u32, d.physical_start, d.number_of_pages * 4096);
        off += desc_size;
    }
    say!("hw: memory {} MB usable, {} MB in reserved ranges, {} MB MMIO, highest RAM address 0x{:x}\n", m.usable >> 20, m.reserved >> 20, m.mmio >> 20, m.top);
}

fn pci() {
    let handles = handles_with(&PCI_IO_PROTOCOL_GUID);
    say!("hw: pci: {} functions\n", handles.len());
    let mut rows: Vec<(usize, usize, usize, usize, u32, u32)> = Vec::new();
    for h in handles {
        let Some(p) = protocol::<PciIo>(h, &PCI_IO_PROTOCOL_GUID) else { continue };
        let (mut seg, mut bus, mut dev, mut fun) = (0usize, 0usize, 0usize, 0usize);
        let mut id = 0u32;
        let mut class = 0u32;
        unsafe {
            if ((*p).get_location)(p, &mut seg, &mut bus, &mut dev, &mut fun).is_error() {
                continue;
            }
            if ((*p).pci.read)(p, PCI_IO_WIDTH_UINT32, 0, 1, &mut id as *mut u32 as *mut c_void).is_error() || id == 0xffff_ffff {
                continue; // a handle whose function answers nothing (hidden by the firmware): not a device
            }
            ((*p).pci.read)(p, PCI_IO_WIDTH_UINT32, 8, 1, &mut class as *mut u32 as *mut c_void);
        }
        rows.push((seg, bus, dev, fun, id, class));
    }
    rows.sort_unstable();
    for (seg, bus, dev, fun, id, class) in rows {
        let (c, sc) = ((class >> 24) as u8, (class >> 16) as u8);
        say!(
            "hw: pci {:04x}:{:02x}:{:02x}.{} {:04x}:{:04x} class {:02x}{:02x} {}\n",
            seg,
            bus,
            dev,
            fun,
            id & 0xffff,
            id >> 16,
            c,
            sc,
            pci_class_name(c, sc)
        );
    }
}

/// The firmware's own text for a handle's device path (`?` when it has none or the firmware cannot print it).
pub fn path_text(h: Handle) -> String {
    let Some(dp) = protocol::<DevicePathProtocol>(h, &DEVICE_PATH_PROTOCOL_GUID) else { return String::from("?") };
    let Some(t) = locate::<DevicePathToText>(&DEVICE_PATH_TO_TEXT_PROTOCOL_GUID) else { return String::from("(no path printer)") };
    let s = unsafe { ((*t).convert_device_path_to_text)(dp, 1, 1) };
    if s.is_null() {
        return String::from("?");
    }
    let out = unsafe { cstr16(s) };
    unsafe { (bs().free_pool)(s as *mut c_void) };
    out
}

/// The firmware vendor string ("Apple", "American Megatrends", "INSYDE Corp.", "EDK II" ...).
pub fn vendor() -> String {
    let t = st();
    if t.firmware_vendor.is_null() {
        String::new()
    } else {
        unsafe { cstr16(t.firmware_vendor) }
    }
}

fn usb() {
    let handles = handles_with(&USB_IO_PROTOCOL_GUID);
    say!("hw: usb: {} interfaces\n", handles.len());
    // Apple's firmware never returns from the USB I/O descriptor call for one of its internal devices (MacBookPro14,1, 2026-11-08:
    // the logo stayed and the log ended at "next: USB"); the count is listed, the devices are not asked
    if vendor().starts_with("Apple") {
        say!("hw: usb: not described on Apple firmware (its USB I/O descriptor call does not return on some Macs)\n");
        return;
    }
    for h in handles.into_iter().take(64) {
        let Some(u) = protocol::<UsbIo>(h, &USB_IO_PROTOCOL_GUID) else { continue };
        let mut d = UsbDeviceDescriptor::default();
        if unsafe { ((*u).get_device_descriptor)(u, &mut d) }.is_error() {
            continue;
        }
        let (v, p, c) = (d.id_vendor, d.id_product, d.device_class);
        say!("hw: usb {v:04x}:{p:04x} class {c:02x} at {}\n", path_text(h));
    }
}

fn disks() {
    let handles = handles_with(&BLOCK_IO_PROTOCOL_GUID);
    let fs: Vec<Handle> = handles_with(&SIMPLE_FILE_SYSTEM_PROTOCOL_GUID);
    say!("hw: disks: {} block devices, {} file systems the firmware can read\n", handles.len(), fs.len());
    for h in handles.into_iter().take(64) {
        let Some(b) = protocol::<BlockIo>(h, &BLOCK_IO_PROTOCOL_GUID) else { continue };
        let m = unsafe { &*(*b).media };
        let mb = if m.block_size == 0 { 0 } else { (m.last_block.saturating_add(1)).saturating_mul(m.block_size as u64) >> 20 };
        say!(
            "hw: {} {} MB, {} B blocks{}{}{}{}, at {}\n",
            if m.logical_partition != 0 { "partition" } else { "disk" },
            mb,
            m.block_size,
            if m.removable_media != 0 { ", removable" } else { "" },
            if m.media_present == 0 { ", no medium" } else { "" },
            if m.read_only != 0 { ", read-only" } else { "" },
            if fs.contains(&h) { ", file system" } else { "" },
            path_text(h)
        );
    }
}

fn screen() {
    let handles = handles_with(&GRAPHICS_OUTPUT_PROTOCOL_GUID);
    if handles.is_empty() {
        say!("hw: screen: no graphics output protocol (text only)\n");
        return;
    }
    for (i, h) in handles.iter().enumerate().take(4) {
        let Some(g) = protocol::<GraphicsOutput>(*h, &GRAPHICS_OUTPUT_PROTOCOL_GUID) else { continue };
        unsafe {
            let m = (*g).mode;
            if m.is_null() || (*m).info.is_null() {
                say!("hw: screen {i}: no mode\n");
                continue;
            }
            let info = &*(*m).info;
            say!(
                "hw: screen {i}: {}x{} (mode {} of {}), pixel format {}, {} pixels per line, frame buffer {} KB at 0x{:x}, at {}\n",
                info.horizontal_resolution,
                info.vertical_resolution,
                (*m).mode,
                (*m).max_mode,
                info.pixel_format,
                info.pixels_per_scan_line,
                (*m).frame_buffer_size >> 10,
                (*m).frame_buffer_base,
                path_text(*h)
            );
        }
    }
}

fn input() {
    say!(
        "hw: input: {} keyboards (text input), {} mice (simple pointer), {} touch or tablet (absolute pointer), {} serial ports\n",
        handles_with(&SIMPLE_TEXT_INPUT_PROTOCOL_GUID).len(),
        handles_with(&SIMPLE_POINTER_PROTOCOL_GUID).len(),
        handles_with(&ABSOLUTE_POINTER_PROTOCOL_GUID).len(),
        handles_with(&SERIAL_IO_PROTOCOL_GUID).len()
    );
}
