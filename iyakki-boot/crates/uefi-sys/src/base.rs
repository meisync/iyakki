// SPDX-License-Identifier: MIT OR Apache-2.0
//! Basic types: handles, GUID, status codes, and the GUIDs a loader uses.

use core::ffi::c_void;
use core::fmt;

/// `EFI_HANDLE`.
pub type Handle = *mut c_void;
/// `EFI_EVENT`.
pub type Event = *mut c_void;
/// `BOOLEAN` (one byte, 0 or 1).
pub type Boolean = u8;
/// `EFI_PHYSICAL_ADDRESS`.
pub type PhysicalAddress = u64;
/// `EFI_VIRTUAL_ADDRESS`.
pub type VirtualAddress = u64;
/// `CHAR16`: UCS-2 code unit.
pub type Char16 = u16;

/// `EFI_GUID`: 16 bytes, fields little-endian in memory except `data4`.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Guid {
    /// First field.
    pub data1: u32,
    /// Second field.
    pub data2: u16,
    /// Third field.
    pub data3: u16,
    /// Last 8 bytes, in written order.
    pub data4: [u8; 8],
}

impl Guid {
    /// Build from the textual fields `data1-data2-data3-d4d5-d6d7d8d9d10d11` (data4 as bytes in written order).
    pub const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Guid {
        Guid { data1, data2, data3, data4 }
    }

    /// The 16 bytes as they sit in memory (and in a device path): first three fields little-endian, then `data4`.
    pub const fn to_bytes(&self) -> [u8; 16] {
        let a = self.data1.to_le_bytes();
        let b = self.data2.to_le_bytes();
        let c = self.data3.to_le_bytes();
        let d = self.data4;
        [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]
    }

    /// Read from the in-memory byte order.
    pub const fn from_bytes(b: [u8; 16]) -> Guid {
        Guid {
            data1: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            data2: u16::from_le_bytes([b[4], b[5]]),
            data3: u16::from_le_bytes([b[6], b[7]]),
            data4: [b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]],
        }
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = &self.data4;
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.data1, self.data2, self.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
        )
    }
}

/// `EFI_STATUS`: native word; the top bit marks an error.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Status(pub usize);

const ERR: usize = 1 << (usize::BITS - 1);

impl Status {
    /// `EFI_SUCCESS`.
    pub const SUCCESS: Status = Status(0);
    /// `EFI_LOAD_ERROR`.
    pub const LOAD_ERROR: Status = Status(ERR | 1);
    /// `EFI_INVALID_PARAMETER`.
    pub const INVALID_PARAMETER: Status = Status(ERR | 2);
    /// `EFI_UNSUPPORTED`.
    pub const UNSUPPORTED: Status = Status(ERR | 3);
    /// `EFI_BAD_BUFFER_SIZE`.
    pub const BAD_BUFFER_SIZE: Status = Status(ERR | 4);
    /// `EFI_BUFFER_TOO_SMALL`.
    pub const BUFFER_TOO_SMALL: Status = Status(ERR | 5);
    /// `EFI_NOT_READY`.
    pub const NOT_READY: Status = Status(ERR | 6);
    /// `EFI_DEVICE_ERROR`.
    pub const DEVICE_ERROR: Status = Status(ERR | 7);
    /// `EFI_WRITE_PROTECTED`.
    pub const WRITE_PROTECTED: Status = Status(ERR | 8);
    /// `EFI_OUT_OF_RESOURCES`.
    pub const OUT_OF_RESOURCES: Status = Status(ERR | 9);
    /// `EFI_VOLUME_CORRUPTED`.
    pub const VOLUME_CORRUPTED: Status = Status(ERR | 10);
    /// `EFI_VOLUME_FULL`.
    pub const VOLUME_FULL: Status = Status(ERR | 11);
    /// `EFI_NO_MEDIA`.
    pub const NO_MEDIA: Status = Status(ERR | 12);
    /// `EFI_MEDIA_CHANGED`.
    pub const MEDIA_CHANGED: Status = Status(ERR | 13);
    /// `EFI_NOT_FOUND`.
    pub const NOT_FOUND: Status = Status(ERR | 14);
    /// `EFI_ACCESS_DENIED`.
    pub const ACCESS_DENIED: Status = Status(ERR | 15);
    /// `EFI_NO_RESPONSE`.
    pub const NO_RESPONSE: Status = Status(ERR | 16);
    /// `EFI_NO_MAPPING`.
    pub const NO_MAPPING: Status = Status(ERR | 17);
    /// `EFI_TIMEOUT`.
    pub const TIMEOUT: Status = Status(ERR | 18);
    /// `EFI_NOT_STARTED`.
    pub const NOT_STARTED: Status = Status(ERR | 19);
    /// `EFI_ALREADY_STARTED`.
    pub const ALREADY_STARTED: Status = Status(ERR | 20);
    /// `EFI_ABORTED`.
    pub const ABORTED: Status = Status(ERR | 21);
    /// `EFI_ICMP_ERROR`.
    pub const ICMP_ERROR: Status = Status(ERR | 22);
    /// `EFI_TFTP_ERROR`.
    pub const TFTP_ERROR: Status = Status(ERR | 23);
    /// `EFI_PROTOCOL_ERROR`.
    pub const PROTOCOL_ERROR: Status = Status(ERR | 24);
    /// `EFI_INCOMPATIBLE_VERSION`.
    pub const INCOMPATIBLE_VERSION: Status = Status(ERR | 25);
    /// `EFI_SECURITY_VIOLATION`: Secure Boot refused the image.
    pub const SECURITY_VIOLATION: Status = Status(ERR | 26);
    /// `EFI_CRC_ERROR`.
    pub const CRC_ERROR: Status = Status(ERR | 27);
    /// `EFI_END_OF_MEDIA`.
    pub const END_OF_MEDIA: Status = Status(ERR | 28);
    /// `EFI_END_OF_FILE`.
    pub const END_OF_FILE: Status = Status(ERR | 31);
    /// `EFI_INVALID_LANGUAGE`.
    pub const INVALID_LANGUAGE: Status = Status(ERR | 32);
    /// `EFI_COMPROMISED_DATA`.
    pub const COMPROMISED_DATA: Status = Status(ERR | 33);
    /// `EFI_WARN_UNKNOWN_GLYPH`.
    pub const WARN_UNKNOWN_GLYPH: Status = Status(1);
    /// `EFI_WARN_DELETE_FAILURE`.
    pub const WARN_DELETE_FAILURE: Status = Status(2);
    /// `EFI_WARN_WRITE_FAILURE`.
    pub const WARN_WRITE_FAILURE: Status = Status(3);
    /// `EFI_WARN_BUFFER_TOO_SMALL`.
    pub const WARN_BUFFER_TOO_SMALL: Status = Status(4);

    /// Top bit set.
    pub const fn is_error(self) -> bool {
        self.0 & ERR != 0
    }
    /// Exactly `EFI_SUCCESS`.
    pub const fn is_success(self) -> bool {
        self.0 == 0
    }
    /// Non-zero without the error bit.
    pub const fn is_warning(self) -> bool {
        self.0 != 0 && !self.is_error()
    }
    /// Success (and warnings) as `Ok`, errors as `Err`.
    pub const fn ok(self) -> Result<Status, Status> {
        if self.is_error() {
            Err(self)
        } else {
            Ok(self)
        }
    }
}

impl fmt::Debug for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Status({:#x})", self.0)
    }
}

/// Variable attribute: kept across resets.
pub const VARIABLE_NON_VOLATILE: u32 = 0x1;
/// Variable attribute: visible to boot services.
pub const VARIABLE_BOOTSERVICE_ACCESS: u32 = 0x2;
/// Variable attribute: visible at runtime.
pub const VARIABLE_RUNTIME_ACCESS: u32 = 0x4;

/// `EFI_TPL` levels.
pub const TPL_APPLICATION: usize = 4;
/// Callback level.
pub const TPL_CALLBACK: usize = 8;
/// Notify level.
pub const TPL_NOTIFY: usize = 16;
/// High level.
pub const TPL_HIGH_LEVEL: usize = 31;

/// `EFI_SIMPLE_TEXT_INPUT_PROTOCOL`.
pub const SIMPLE_TEXT_INPUT_PROTOCOL_GUID: Guid = Guid::new(0x387477c1, 0x69c7, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
/// `EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL`.
pub const SIMPLE_TEXT_OUTPUT_PROTOCOL_GUID: Guid = Guid::new(0x387477c2, 0x69c7, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
/// `EFI_SIMPLE_POINTER_PROTOCOL`.
pub const SIMPLE_POINTER_PROTOCOL_GUID: Guid = Guid::new(0x31878c87, 0x0b75, 0x11d5, [0x9a, 0x4f, 0x00, 0x90, 0x27, 0x3f, 0xc1, 0x4d]);
/// `EFI_SERIAL_IO_PROTOCOL`.
pub const SERIAL_IO_PROTOCOL_GUID: Guid = Guid::new(0xbb25cf6f, 0xf1d4, 0x11d2, [0x9a, 0x0c, 0x00, 0x90, 0x27, 0x3f, 0xc1, 0xfd]);
/// `EFI_GRAPHICS_OUTPUT_PROTOCOL`.
pub const GRAPHICS_OUTPUT_PROTOCOL_GUID: Guid = Guid::new(0x9042a9de, 0x23dc, 0x4a38, [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a]);
/// `EFI_SIMPLE_FILE_SYSTEM_PROTOCOL`.
pub const SIMPLE_FILE_SYSTEM_PROTOCOL_GUID: Guid = Guid::new(0x964e5b22, 0x6459, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
/// `EFI_LOADED_IMAGE_PROTOCOL`.
pub const LOADED_IMAGE_PROTOCOL_GUID: Guid = Guid::new(0x5b1b31a1, 0x9562, 0x11d2, [0x8e, 0x3f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
/// `EFI_LOADED_IMAGE_DEVICE_PATH_PROTOCOL`.
pub const LOADED_IMAGE_DEVICE_PATH_PROTOCOL_GUID: Guid = Guid::new(0xbc62157e, 0x3e33, 0x4fec, [0x99, 0x20, 0x2d, 0x3b, 0x36, 0xd7, 0x50, 0xdf]);
/// `EFI_BLOCK_IO_PROTOCOL`.
pub const BLOCK_IO_PROTOCOL_GUID: Guid = Guid::new(0x964e5b21, 0x6459, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
/// `EFI_DEVICE_PATH_PROTOCOL`.
pub const DEVICE_PATH_PROTOCOL_GUID: Guid = Guid::new(0x09576e91, 0x6d3f, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
/// `EFI_FILE_INFO_ID`.
pub const FILE_INFO_GUID: Guid = Guid::new(0x09576e92, 0x6d3f, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
/// `EFI_FILE_SYSTEM_INFO_ID`.
pub const FILE_SYSTEM_INFO_GUID: Guid = Guid::new(0x09576e93, 0x6d3f, 0x11d2, [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b]);
/// `EFI_LOAD_FILE2_PROTOCOL`.
pub const LOAD_FILE2_PROTOCOL_GUID: Guid = Guid::new(0x4006c0c1, 0xfcb3, 0x403e, [0x99, 0x6d, 0x4a, 0x6c, 0x87, 0x24, 0xe0, 0x6d]);
/// Linux initrd media device path vendor GUID: the Linux EFI stub asks this handle's `LOAD_FILE2` for the initrd.
pub const LINUX_EFI_INITRD_MEDIA_GUID: Guid = Guid::new(0x5568e427, 0x68fc, 0x4f3d, [0xac, 0x74, 0xca, 0x55, 0x52, 0x31, 0xcc, 0x68]);
/// `EFI_GLOBAL_VARIABLE` vendor (BootOrder, Boot####, SecureBoot, SetupMode ...).
pub const GLOBAL_VARIABLE_GUID: Guid = Guid::new(0x8be4df61, 0x93ca, 0x11d2, [0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b, 0x8c]);
/// `EFI_IMAGE_SECURITY_DATABASE` vendor (db, dbx).
pub const IMAGE_SECURITY_DATABASE_GUID: Guid = Guid::new(0xd719b2cb, 0x3d3a, 0x4596, [0xa3, 0xbc, 0xda, 0xd0, 0x0e, 0x67, 0x65, 0x6f]);
/// ACPI 2.0 configuration table.
pub const ACPI_20_TABLE_GUID: Guid = Guid::new(0x8868e871, 0xe4f1, 0x11d3, [0xbc, 0x22, 0x00, 0x80, 0xc7, 0x3c, 0x88, 0x81]);
/// SMBIOS 3 configuration table.
pub const SMBIOS3_TABLE_GUID: Guid = Guid::new(0xf2fd1544, 0x9794, 0x4a2c, [0x99, 0x2e, 0xe5, 0xbb, 0xcf, 0x20, 0xe3, 0x94]);
/// Shim's lock protocol (present when started by shim).
pub const SHIM_LOCK_PROTOCOL_GUID: Guid = Guid::new(0x605dab50, 0xe046, 0x4300, [0xab, 0xb6, 0x3d, 0xd8, 0x10, 0xdd, 0x8b, 0x23]);
/// `EFI_SECURITY_ARCH_PROTOCOL` (the firmware's image check, path based).
pub const SECURITY_ARCH_PROTOCOL_GUID: Guid = Guid::new(0xa46423e3, 0x4617, 0x49f1, [0xb9, 0xff, 0xd1, 0xbf, 0xa9, 0x11, 0x58, 0x39]);
/// `EFI_SECURITY2_ARCH_PROTOCOL` (the firmware's image check, with the file's bytes).
pub const SECURITY2_ARCH_PROTOCOL_GUID: Guid = Guid::new(0x94ab2f58, 0x1438, 0x4ef1, [0x91, 0x52, 0x18, 0x94, 0x1a, 0x3a, 0x0e, 0x68]);
/// `EFI_TCG2_PROTOCOL` (TPM 2.0 measured boot).
pub const TCG2_PROTOCOL_GUID: Guid = Guid::new(0x607f766c, 0x7455, 0x42be, [0x93, 0x0b, 0xe4, 0xd7, 0x6d, 0xb2, 0x72, 0x0f]);
/// GPT partition type of an EFI System Partition (not a protocol GUID; found in the partition table).
pub const GPT_ESP_TYPE_GUID: Guid = Guid::new(0xc12a7328, 0xf81f, 0x11d2, [0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b]);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_size_and_alignment() {
        assert_eq!(core::mem::size_of::<Guid>(), 16);
        assert_eq!(core::mem::align_of::<Guid>(), 4);
    }

    #[test]
    fn guid_byte_order_is_mixed_endian() {
        // 5568e427-68fc-4f3d-ac74-ca555231cc68 as stored in memory
        let b = LINUX_EFI_INITRD_MEDIA_GUID.to_bytes();
        assert_eq!(b, [0x27, 0xe4, 0x68, 0x55, 0xfc, 0x68, 0x3d, 0x4f, 0xac, 0x74, 0xca, 0x55, 0x52, 0x31, 0xcc, 0x68]);
        assert_eq!(Guid::from_bytes(b), LINUX_EFI_INITRD_MEDIA_GUID);
        // the in-memory bytes of the struct itself match on a little-endian host
        #[cfg(target_endian = "little")]
        {
            let raw: [u8; 16] = unsafe { core::mem::transmute(LINUX_EFI_INITRD_MEDIA_GUID) };
            assert_eq!(raw, b);
        }
        let g = GLOBAL_VARIABLE_GUID.to_bytes();
        assert_eq!(&g[..4], &[0x61, 0xdf, 0xe4, 0x8b]);
        assert_eq!(&g[8..], &[0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b, 0x8c]);
    }

    #[test]
    fn guid_debug_prints_canonical_text() {
        extern crate std;
        use std::format;
        assert_eq!(format!("{:?}", LINUX_EFI_INITRD_MEDIA_GUID), "5568e427-68fc-4f3d-ac74-ca555231cc68");
        assert_eq!(format!("{:?}", GRAPHICS_OUTPUT_PROTOCOL_GUID), "9042a9de-23dc-4a38-96fb-7aded080516a");
        assert_eq!(format!("{:?}", GPT_ESP_TYPE_GUID), "c12a7328-f81f-11d2-ba4b-00a0c93ec93b");
    }

    #[test]
    fn all_guids_are_distinct() {
        let all = [
            SIMPLE_TEXT_INPUT_PROTOCOL_GUID, SIMPLE_TEXT_OUTPUT_PROTOCOL_GUID, GRAPHICS_OUTPUT_PROTOCOL_GUID,
            SIMPLE_FILE_SYSTEM_PROTOCOL_GUID, LOADED_IMAGE_PROTOCOL_GUID, LOADED_IMAGE_DEVICE_PATH_PROTOCOL_GUID,
            BLOCK_IO_PROTOCOL_GUID, DEVICE_PATH_PROTOCOL_GUID, FILE_INFO_GUID, FILE_SYSTEM_INFO_GUID, LOAD_FILE2_PROTOCOL_GUID,
            LINUX_EFI_INITRD_MEDIA_GUID, GLOBAL_VARIABLE_GUID, IMAGE_SECURITY_DATABASE_GUID, ACPI_20_TABLE_GUID,
            SMBIOS3_TABLE_GUID, SHIM_LOCK_PROTOCOL_GUID, TCG2_PROTOCOL_GUID, GPT_ESP_TYPE_GUID, SECURITY_ARCH_PROTOCOL_GUID, SECURITY2_ARCH_PROTOCOL_GUID,
        ];
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                assert_ne!(all[i], all[j], "{i} {j}");
            }
        }
    }

    #[test]
    fn status_values() {
        let top = 1usize << (usize::BITS - 1);
        assert_eq!(Status::SUCCESS.0, 0);
        assert_eq!(Status::LOAD_ERROR.0, top | 1);
        assert_eq!(Status::NOT_FOUND.0, top | 14);
        assert_eq!(Status::SECURITY_VIOLATION.0, top | 26);
        assert_eq!(Status::END_OF_FILE.0, top | 31);
        assert!(Status::BUFFER_TOO_SMALL.is_error());
        assert!(!Status::SUCCESS.is_error() && Status::SUCCESS.is_success());
        assert!(Status::WARN_UNKNOWN_GLYPH.is_warning() && !Status::WARN_UNKNOWN_GLYPH.is_error());
        assert_eq!(Status::WARN_UNKNOWN_GLYPH.ok(), Ok(Status::WARN_UNKNOWN_GLYPH));
        assert_eq!(Status::ABORTED.ok(), Err(Status::ABORTED));
    }

    #[test]
    fn status_is_one_word() {
        assert_eq!(core::mem::size_of::<Status>(), core::mem::size_of::<usize>());
    }
}
