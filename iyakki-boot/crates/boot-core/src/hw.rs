// SPDX-License-Identifier: MIT OR Apache-2.0
//! The pure parts of the device summary in the boot log (`boot-uefi/src/hwlog.rs` does the firmware calls): SMBIOS string lookup,
//! PCI class names, memory map totals. Here so they have host tests.

use alloc::string::String;

/// String number `n` (1-based, 0 = none) of an SMBIOS structure: `body` is the whole structure (formatted part of `fixed` bytes, then
/// the NUL-separated strings, then the closing NUL). Missing or out of range gives an empty string, never a panic.
pub fn smbios_string(body: &[u8], fixed: usize, n: u8) -> String {
    if n == 0 || body.len() <= fixed {
        return String::new();
    }
    let mut rest = &body[fixed..];
    let mut i = 1;
    while !rest.is_empty() {
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        if i == n {
            return String::from_utf8_lossy(&rest[..end]).trim().into();
        }
        if end == rest.len() {
            break;
        }
        rest = &rest[end + 1..];
        i += 1;
    }
    String::new()
}

/// Plain words for a PCI class and subclass.
pub fn pci_class_name(class: u8, sub: u8) -> &'static str {
    match (class, sub) {
        (0x01, 0x01) => "IDE controller",
        (0x01, 0x04) => "RAID controller (Intel VMD/RST when 8086: an NVMe behind it needs the vmd driver)",
        (0x01, 0x06) => "SATA controller",
        (0x01, 0x08) => "NVMe controller",
        (0x01, _) => "storage controller",
        (0x02, 0x00) => "Ethernet",
        (0x02, 0x80) => "network (wireless)",
        (0x02, _) => "network",
        (0x03, 0x00) => "VGA display",
        (0x03, 0x02) => "3D display (second GPU, no display output)",
        (0x03, _) => "display",
        (0x04, 0x01 | 0x03) => "audio",
        (0x04, _) => "multimedia",
        (0x05, _) => "memory controller",
        (0x06, 0x00) => "host bridge",
        (0x06, 0x01) => "ISA bridge",
        (0x06, 0x04) => "PCI bridge",
        (0x06, _) => "bridge",
        (0x07, 0x00) => "serial port",
        (0x07, _) => "communication",
        (0x08, 0x05) => "SD host",
        (0x08, _) => "system peripheral",
        (0x09, _) => "input",
        (0x0c, 0x03) => "USB controller",
        (0x0c, 0x05) => "SMBus",
        (0x0c, 0x80) => "serial bus (I2C/I3C, Intel LPSS)",
        (0x0c, _) => "serial bus",
        (0x0d, _) => "wireless",
        (0x10, _) => "encryption",
        (0x11, _) => "signal processing",
        _ => "other",
    }
}

/// Totals over the firmware memory map, in bytes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MemoryTotals {
    /// RAM the operating system gets: loader, boot and runtime services code and data, conventional, ACPI reclaimable.
    pub usable: u64,
    /// Reserved, unusable, ACPI NVS, persistent and unknown ranges (firmware may mark far more than the RAM).
    pub reserved: u64,
    /// Memory-mapped I/O (and its port space).
    pub mmio: u64,
    /// One past the highest usable RAM address.
    pub top: u64,
}

impl MemoryTotals {
    /// Add one descriptor (`ty` is the `EFI_MEMORY_TYPE` number).
    pub fn add(&mut self, ty: u32, start: u64, bytes: u64) {
        match ty {
            1..=7 | 9 => {
                self.usable += bytes;
                self.top = self.top.max(start.saturating_add(bytes));
            }
            11 | 12 => self.mmio += bytes,
            _ => self.reserved += bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smbios_strings_are_found_by_number_and_missing_ones_are_empty() {
        // type 1 system information: 4-byte header + fields up to 0x1b, strings "HP", "HP Pavilion", "", then the double NUL
        let mut body = alloc::vec![1u8, 0x1b, 0, 0];
        body.extend_from_slice(&[1, 2, 3, 0]); // manufacturer=1 product=2 version=3 serial=0
        body.resize(0x1b, 0);
        body.extend_from_slice(b"HP\0HP Pavilion\0 v1 \0\0");
        assert_eq!(smbios_string(&body, 0x1b, 1), "HP");
        assert_eq!(smbios_string(&body, 0x1b, 2), "HP Pavilion");
        assert_eq!(smbios_string(&body, 0x1b, 3), "v1", "trimmed");
        assert_eq!(smbios_string(&body, 0x1b, 0), "", "0 means no string");
        assert_eq!(smbios_string(&body, 0x1b, 4), "", "past the last string");
        assert_eq!(smbios_string(&body, 0x40, 1), "", "formatted part longer than the body");
        assert_eq!(smbios_string(b"", 4, 1), "");
        assert_eq!(smbios_string(b"\x01\x04\0\0abc", 4, 1), "abc", "a string without its NUL still counts");
    }

    #[test]
    fn pci_classes_name_the_parts_that_matter_for_a_boot() {
        assert_eq!(pci_class_name(0x01, 0x08), "NVMe controller");
        assert!(pci_class_name(0x01, 0x04).contains("VMD"));
        assert_eq!(pci_class_name(0x03, 0x02), "3D display (second GPU, no display output)");
        assert_eq!(pci_class_name(0x0c, 0x03), "USB controller");
        assert_eq!(pci_class_name(0x02, 0x80), "network (wireless)");
        assert_eq!(pci_class_name(0xff, 0xff), "other");
    }

    #[test]
    fn memory_totals_split_usable_reserved_and_mmio_and_track_the_top() {
        let mut m = MemoryTotals::default();
        m.add(7, 0x1000, 0x1000); // conventional
        m.add(2, 0x100000, 0x10000); // loader data
        m.add(10, 0x200000, 0x2000); // ACPI NVS: reserved
        m.add(0, 0x300000, 0x4000); // reserved
        m.add(11, 0xf000_0000, 0x1000_0000); // MMIO
        m.add(9, 0x8_0000_0000, 0x1000); // ACPI reclaim: usable, highest
        assert_eq!(m.usable, 0x1000 + 0x10000 + 0x1000);
        assert_eq!(m.reserved, 0x6000);
        assert_eq!(m.mmio, 0x1000_0000);
        assert_eq!(m.top, 0x8_0000_1000);
    }
}
