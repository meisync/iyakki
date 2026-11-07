// SPDX-License-Identifier: MIT OR Apache-2.0
//! Protocol structures: text in/out, graphics output, file system, loaded image, block I/O, device path, load file 2.

use crate::base::*;
use crate::tables::{SystemTable, Time};
use core::ffi::c_void;

// ---- Simple text input / output ----------------------------------------------------------------------------------------------

/// `EFI_INPUT_KEY`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputKey {
    /// Non-zero for special keys.
    pub scan_code: u16,
    /// UCS-2 character, or 0.
    pub unicode_char: Char16,
}

/// Scan code: up arrow.
pub const SCAN_UP: u16 = 0x01;
/// Scan code: down arrow.
pub const SCAN_DOWN: u16 = 0x02;
/// Scan code: right arrow.
pub const SCAN_RIGHT: u16 = 0x03;
/// Scan code: left arrow.
pub const SCAN_LEFT: u16 = 0x04;
/// Scan code: home.
pub const SCAN_HOME: u16 = 0x05;
/// Scan code: end.
pub const SCAN_END: u16 = 0x06;
/// Scan code: page up.
pub const SCAN_PAGE_UP: u16 = 0x09;
/// Scan code: page down.
pub const SCAN_PAGE_DOWN: u16 = 0x0a;
/// Scan code: escape.
pub const SCAN_ESC: u16 = 0x17;

/// `EFI_SIMPLE_TEXT_INPUT_PROTOCOL`.
#[repr(C)]
pub struct SimpleTextInput {
    /// `Reset`.
    pub reset: unsafe extern "efiapi" fn(this: *mut SimpleTextInput, extended_verification: Boolean) -> Status,
    /// `ReadKeyStroke` (`NOT_READY` when no key).
    pub read_key_stroke: unsafe extern "efiapi" fn(this: *mut SimpleTextInput, key: *mut InputKey) -> Status,
    /// Event signalled when a key is available.
    pub wait_for_key: Event,
}

/// `SIMPLE_TEXT_OUTPUT_MODE`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SimpleTextOutputMode {
    /// Number of modes.
    pub max_mode: i32,
    /// Current mode.
    pub mode: i32,
    /// Current attribute.
    pub attribute: i32,
    /// Cursor column.
    pub cursor_column: i32,
    /// Cursor row.
    pub cursor_row: i32,
    /// Cursor visible.
    pub cursor_visible: Boolean,
}

/// `EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL`.
#[repr(C)]
pub struct SimpleTextOutput {
    /// `Reset`.
    pub reset: unsafe extern "efiapi" fn(this: *mut SimpleTextOutput, extended_verification: Boolean) -> Status,
    /// `OutputString` (NUL-terminated UCS-2).
    pub output_string: unsafe extern "efiapi" fn(this: *mut SimpleTextOutput, string: *const Char16) -> Status,
    /// `TestString`.
    pub test_string: unsafe extern "efiapi" fn(this: *mut SimpleTextOutput, string: *const Char16) -> Status,
    /// `QueryMode`.
    pub query_mode: unsafe extern "efiapi" fn(this: *mut SimpleTextOutput, mode: usize, columns: *mut usize, rows: *mut usize) -> Status,
    /// `SetMode`.
    pub set_mode: unsafe extern "efiapi" fn(this: *mut SimpleTextOutput, mode: usize) -> Status,
    /// `SetAttribute`.
    pub set_attribute: unsafe extern "efiapi" fn(this: *mut SimpleTextOutput, attribute: usize) -> Status,
    /// `ClearScreen`.
    pub clear_screen: unsafe extern "efiapi" fn(this: *mut SimpleTextOutput) -> Status,
    /// `SetCursorPosition`.
    pub set_cursor_position: unsafe extern "efiapi" fn(this: *mut SimpleTextOutput, column: usize, row: usize) -> Status,
    /// `EnableCursor`.
    pub enable_cursor: unsafe extern "efiapi" fn(this: *mut SimpleTextOutput, visible: Boolean) -> Status,
    /// Current mode data.
    pub mode: *mut SimpleTextOutputMode,
}

// ---- Graphics output ---------------------------------------------------------------------------------------------------------

/// `EFI_PIXEL_BITMASK`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PixelBitmask {
    /// Red mask.
    pub red_mask: u32,
    /// Green mask.
    pub green_mask: u32,
    /// Blue mask.
    pub blue_mask: u32,
    /// Reserved mask.
    pub reserved_mask: u32,
}

/// `EFI_GRAPHICS_PIXEL_FORMAT`: red, green, blue, reserved bytes in that order.
pub const PIXEL_RED_GREEN_BLUE_RESERVED_8BIT: u32 = 0;
/// `EFI_GRAPHICS_PIXEL_FORMAT`: blue, green, red, reserved bytes in that order.
pub const PIXEL_BLUE_GREEN_RED_RESERVED_8BIT: u32 = 1;
/// `EFI_GRAPHICS_PIXEL_FORMAT`: described by `pixel_information`.
pub const PIXEL_BIT_MASK: u32 = 2;
/// `EFI_GRAPHICS_PIXEL_FORMAT`: no frame buffer, only `blt`.
pub const PIXEL_BLT_ONLY: u32 = 3;

/// `EFI_GRAPHICS_OUTPUT_MODE_INFORMATION`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GraphicsOutputModeInformation {
    /// Version (0).
    pub version: u32,
    /// Width in pixels.
    pub horizontal_resolution: u32,
    /// Height in pixels.
    pub vertical_resolution: u32,
    /// One of the `PIXEL_*` constants.
    pub pixel_format: u32,
    /// Masks when `pixel_format` is `PIXEL_BIT_MASK`.
    pub pixel_information: PixelBitmask,
    /// Pixels per scan line (stride; can exceed the width).
    pub pixels_per_scan_line: u32,
}

/// `EFI_GRAPHICS_OUTPUT_PROTOCOL_MODE`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GraphicsOutputMode {
    /// Number of modes.
    pub max_mode: u32,
    /// Current mode.
    pub mode: u32,
    /// Current mode information.
    pub info: *mut GraphicsOutputModeInformation,
    /// Size of `info`.
    pub size_of_info: usize,
    /// Physical address of the frame buffer.
    pub frame_buffer_base: PhysicalAddress,
    /// Size of the frame buffer.
    pub frame_buffer_size: usize,
}

/// `EFI_GRAPHICS_OUTPUT_BLT_PIXEL`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BltPixel {
    /// Blue.
    pub blue: u8,
    /// Green.
    pub green: u8,
    /// Red.
    pub red: u8,
    /// Reserved.
    pub reserved: u8,
}

/// `EFI_GRAPHICS_OUTPUT_BLT_OPERATION`.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BltOperation {
    /// Fill the target with one pixel.
    VideoFill = 0,
    /// Screen to buffer.
    VideoToBltBuffer = 1,
    /// Buffer to screen.
    BufferToVideo = 2,
    /// Screen to screen.
    VideoToVideo = 3,
}

/// `EFI_GRAPHICS_OUTPUT_PROTOCOL`.
#[repr(C)]
pub struct GraphicsOutput {
    /// `QueryMode`.
    pub query_mode: unsafe extern "efiapi" fn(
        this: *mut GraphicsOutput,
        mode: u32,
        size: *mut usize,
        info: *mut *mut GraphicsOutputModeInformation,
    ) -> Status,
    /// `SetMode`.
    pub set_mode: unsafe extern "efiapi" fn(this: *mut GraphicsOutput, mode: u32) -> Status,
    /// `Blt`.
    pub blt: unsafe extern "efiapi" fn(
        this: *mut GraphicsOutput,
        buffer: *mut BltPixel,
        op: BltOperation,
        src_x: usize,
        src_y: usize,
        dst_x: usize,
        dst_y: usize,
        width: usize,
        height: usize,
        delta: usize,
    ) -> Status,
    /// Current mode.
    pub mode: *mut GraphicsOutputMode,
}

// ---- Simple pointer ----------------------------------------------------------------------------------------------------------

/// `EFI_SIMPLE_POINTER_STATE`: movement since the last call and the buttons.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SimplePointerState {
    /// Relative movement, x.
    pub relative_movement_x: i32,
    /// Relative movement, y.
    pub relative_movement_y: i32,
    /// Relative movement, z (wheel).
    pub relative_movement_z: i32,
    /// Left button is down.
    pub left_button: Boolean,
    /// Right button is down.
    pub right_button: Boolean,
}

/// `EFI_SIMPLE_POINTER_MODE`: counts per millimetre (0 when unknown) and which buttons exist.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimplePointerMode {
    /// Resolution x.
    pub resolution_x: u64,
    /// Resolution y.
    pub resolution_y: u64,
    /// Resolution z.
    pub resolution_z: u64,
    /// Has a left button.
    pub left_button: Boolean,
    /// Has a right button.
    pub right_button: Boolean,
}

/// `EFI_SIMPLE_POINTER_PROTOCOL` (a mouse, or a touch pad that reports relative movement).
#[repr(C)]
pub struct SimplePointer {
    /// `Reset`.
    pub reset: unsafe extern "efiapi" fn(this: *mut SimplePointer, extended_verification: Boolean) -> Status,
    /// `GetState` (`NOT_READY` when nothing happened).
    pub get_state: unsafe extern "efiapi" fn(this: *mut SimplePointer, state: *mut SimplePointerState) -> Status,
    /// Signalled when the state changes.
    pub wait_for_input: Event,
    /// Resolution and buttons.
    pub mode: *mut SimplePointerMode,
}

// ---- File system -------------------------------------------------------------------------------------------------------------

/// File mode: read.
pub const FILE_MODE_READ: u64 = 0x1;
/// File mode: write (use with read).
pub const FILE_MODE_WRITE: u64 = 0x2;
/// File mode: create (use with read and write).
pub const FILE_MODE_CREATE: u64 = 0x8000_0000_0000_0000;
/// File attribute: read only.
pub const FILE_READ_ONLY: u64 = 0x1;
/// File attribute: hidden.
pub const FILE_HIDDEN: u64 = 0x2;
/// File attribute: system.
pub const FILE_SYSTEM: u64 = 0x4;
/// File attribute: directory.
pub const FILE_DIRECTORY: u64 = 0x10;
/// File attribute: archive.
pub const FILE_ARCHIVE: u64 = 0x20;

/// `EFI_SIMPLE_FILE_SYSTEM_PROTOCOL`.
#[repr(C)]
pub struct SimpleFileSystem {
    /// Revision.
    pub revision: u64,
    /// `OpenVolume`: gives the root directory.
    pub open_volume: unsafe extern "efiapi" fn(this: *mut SimpleFileSystem, root: *mut *mut FileProtocol) -> Status,
}

/// `EFI_FILE_PROTOCOL`. Revision 1 ends at `flush`; the `*_ex` slots exist from revision 2 (check `revision` first).
#[repr(C)]
pub struct FileProtocol {
    /// Revision.
    pub revision: u64,
    /// `Open`.
    pub open: unsafe extern "efiapi" fn(
        this: *mut FileProtocol,
        new_handle: *mut *mut FileProtocol,
        file_name: *const Char16,
        open_mode: u64,
        attributes: u64,
    ) -> Status,
    /// `Close`.
    pub close: unsafe extern "efiapi" fn(this: *mut FileProtocol) -> Status,
    /// `Delete`.
    pub delete: unsafe extern "efiapi" fn(this: *mut FileProtocol) -> Status,
    /// `Read` (a directory returns one `FileInfo` per call, size 0 at the end).
    pub read: unsafe extern "efiapi" fn(this: *mut FileProtocol, buffer_size: *mut usize, buffer: *mut c_void) -> Status,
    /// `Write`.
    pub write: unsafe extern "efiapi" fn(this: *mut FileProtocol, buffer_size: *mut usize, buffer: *const c_void) -> Status,
    /// `GetPosition`.
    pub get_position: unsafe extern "efiapi" fn(this: *mut FileProtocol, position: *mut u64) -> Status,
    /// `SetPosition` (`u64::MAX` is the end of file).
    pub set_position: unsafe extern "efiapi" fn(this: *mut FileProtocol, position: u64) -> Status,
    /// `GetInfo`.
    pub get_info: unsafe extern "efiapi" fn(
        this: *mut FileProtocol,
        info_type: *const Guid,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status,
    /// `SetInfo`.
    pub set_info:
        unsafe extern "efiapi" fn(this: *mut FileProtocol, info_type: *const Guid, buffer_size: usize, buffer: *const c_void) -> Status,
    /// `Flush`.
    pub flush: unsafe extern "efiapi" fn(this: *mut FileProtocol) -> Status,
    /// `OpenEx` (revision 2, unused).
    pub open_ex: *const c_void,
    /// `ReadEx` (revision 2, unused).
    pub read_ex: *const c_void,
    /// `WriteEx` (revision 2, unused).
    pub write_ex: *const c_void,
    /// `FlushEx` (revision 2, unused).
    pub flush_ex: *const c_void,
}

/// `EFI_FILE_INFO` header; the UCS-2 name follows at `size_of::<FileInfo>()` (offset 80) and the whole record is `size` bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FileInfo {
    /// Size of the whole record including the name.
    pub size: u64,
    /// Size of the file in bytes.
    pub file_size: u64,
    /// Space used on the volume.
    pub physical_size: u64,
    /// Creation time.
    pub create_time: Time,
    /// Last access time.
    pub last_access_time: Time,
    /// Modification time.
    pub modification_time: Time,
    /// `FILE_*` attribute bits.
    pub attribute: u64,
}

// ---- Loaded image ------------------------------------------------------------------------------------------------------------

/// `EFI_LOADED_IMAGE_PROTOCOL`.
#[repr(C)]
pub struct LoadedImage {
    /// Revision.
    pub revision: u32,
    /// Handle of the image that started this one.
    pub parent_handle: Handle,
    /// System table.
    pub system_table: *mut SystemTable,
    /// Device the image was loaded from.
    pub device_handle: Handle,
    /// Path of the image on that device.
    pub file_path: *mut DevicePathProtocol,
    /// Reserved.
    pub reserved: *mut c_void,
    /// Size of `load_options` in bytes.
    pub load_options_size: u32,
    /// Load options (command line).
    pub load_options: *mut c_void,
    /// Where the image sits in memory.
    pub image_base: *mut c_void,
    /// Size of the image in memory.
    pub image_size: u64,
    /// Memory type of the code.
    pub image_code_type: u32,
    /// Memory type of the data.
    pub image_data_type: u32,
    /// `Unload`.
    pub unload: Option<unsafe extern "efiapi" fn(image: Handle) -> Status>,
}

// ---- Block I/O ---------------------------------------------------------------------------------------------------------------

/// `EFI_BLOCK_IO_MEDIA` (revision 2/3 layout).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BlockIoMedia {
    /// Changes when media changes.
    pub media_id: u32,
    /// Removable media.
    pub removable_media: Boolean,
    /// Media present.
    pub media_present: Boolean,
    /// Partition rather than whole disk.
    pub logical_partition: Boolean,
    /// Read only.
    pub read_only: Boolean,
    /// Write caching.
    pub write_caching: Boolean,
    /// Block size in bytes.
    pub block_size: u32,
    /// Required buffer alignment.
    pub io_align: u32,
    /// Last addressable block.
    pub last_block: u64,
    /// Revision 2: first aligned LBA.
    pub lowest_aligned_lba: u64,
    /// Revision 2: logical blocks per physical block.
    pub logical_blocks_per_physical_block: u32,
    /// Revision 3: optimal transfer length granularity.
    pub optimal_transfer_length_granularity: u32,
}

/// `EFI_BLOCK_IO_PROTOCOL`.
#[repr(C)]
pub struct BlockIo {
    /// Revision.
    pub revision: u64,
    /// Media data.
    pub media: *mut BlockIoMedia,
    /// `Reset`.
    pub reset: unsafe extern "efiapi" fn(this: *mut BlockIo, extended_verification: Boolean) -> Status,
    /// `ReadBlocks`.
    pub read_blocks:
        unsafe extern "efiapi" fn(this: *mut BlockIo, media_id: u32, lba: u64, buffer_size: usize, buffer: *mut c_void) -> Status,
    /// `WriteBlocks`.
    pub write_blocks:
        unsafe extern "efiapi" fn(this: *mut BlockIo, media_id: u32, lba: u64, buffer_size: usize, buffer: *const c_void) -> Status,
    /// `FlushBlocks`.
    pub flush_blocks: unsafe extern "efiapi" fn(this: *mut BlockIo) -> Status,
}

// ---- Device path -------------------------------------------------------------------------------------------------------------

/// Device path node type: hardware.
pub const DP_TYPE_HARDWARE: u8 = 0x01;
/// Device path node type: ACPI.
pub const DP_TYPE_ACPI: u8 = 0x02;
/// Device path node type: messaging (USB, SATA, NVMe ...).
pub const DP_TYPE_MESSAGING: u8 = 0x03;
/// Device path node type: media (partition, file, vendor).
pub const DP_TYPE_MEDIA: u8 = 0x04;
/// Device path node type: BIOS boot specification.
pub const DP_TYPE_BIOS_BOOT: u8 = 0x05;
/// Device path node type: end.
pub const DP_TYPE_END: u8 = 0x7f;
/// End node sub type: end of the whole path.
pub const DP_SUBTYPE_END_ENTIRE: u8 = 0xff;
/// End node sub type: end of one instance.
pub const DP_SUBTYPE_END_INSTANCE: u8 = 0x01;
/// Media sub type: hard drive partition.
pub const DP_SUBTYPE_MEDIA_HARD_DRIVE: u8 = 0x01;
/// Media sub type: vendor defined (GUID follows).
pub const DP_SUBTYPE_MEDIA_VENDOR: u8 = 0x03;
/// Media sub type: file path (UCS-2 name follows).
pub const DP_SUBTYPE_MEDIA_FILE_PATH: u8 = 0x04;

/// `EFI_DEVICE_PATH_PROTOCOL`: the header of every node; the node is `length` bytes long.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DevicePathProtocol {
    /// Node type.
    pub ty: u8,
    /// Node sub type.
    pub sub_type: u8,
    /// Node length in bytes (little-endian, header included).
    pub length: [u8; 2],
}

impl DevicePathProtocol {
    /// A header with the given type and total node length.
    pub const fn new(ty: u8, sub_type: u8, length: u16) -> DevicePathProtocol {
        DevicePathProtocol { ty, sub_type, length: length.to_le_bytes() }
    }
    /// Node length in bytes.
    pub const fn node_length(&self) -> u16 {
        u16::from_le_bytes(self.length)
    }
    /// End of the entire path.
    pub const fn is_end_entire(&self) -> bool {
        self.ty == DP_TYPE_END && self.sub_type == DP_SUBTYPE_END_ENTIRE
    }
    /// End of an instance or of the entire path.
    pub const fn is_end(&self) -> bool {
        self.ty == DP_TYPE_END
    }
}

/// The device path the Linux EFI stub looks for: one vendor media node with `LINUX_EFI_INITRD_MEDIA_GUID`, then the end node.
/// Install it with `DEVICE_PATH_PROTOCOL_GUID` and a `LOAD_FILE2` interface on a new handle.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitrdDevicePath {
    /// Vendor media node header (length 20).
    pub vendor: DevicePathProtocol,
    /// The initrd GUID.
    pub guid: Guid,
    /// End node.
    pub end: DevicePathProtocol,
}

impl InitrdDevicePath {
    /// The fixed path.
    pub const VALUE: InitrdDevicePath = InitrdDevicePath {
        vendor: DevicePathProtocol::new(DP_TYPE_MEDIA, DP_SUBTYPE_MEDIA_VENDOR, 20),
        guid: LINUX_EFI_INITRD_MEDIA_GUID,
        end: DevicePathProtocol::new(DP_TYPE_END, DP_SUBTYPE_END_ENTIRE, 4),
    };
}

/// `EFI_LOAD_FILE2_PROTOCOL`. For the initrd: when `buffer` is null, set `*buffer_size` and return `BUFFER_TOO_SMALL`; otherwise
/// copy the initrd to `buffer` and return `SUCCESS`.
#[repr(C)]
pub struct LoadFile2 {
    /// `LoadFile`.
    pub load_file: unsafe extern "efiapi" fn(
        this: *mut LoadFile2,
        file_path: *const DevicePathProtocol,
        boot_policy: Boolean,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check_layout;

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn x86_64_layouts() {
        check_layout!(InputKey, 4, { scan_code: 0, unicode_char: 2 });
        check_layout!(SimpleTextInput, 24, { reset: 0, read_key_stroke: 8, wait_for_key: 16 });
        check_layout!(SimpleTextOutputMode, 24, { max_mode: 0, mode: 4, attribute: 8, cursor_column: 12, cursor_row: 16, cursor_visible: 20 });
        check_layout!(SimpleTextOutput, 80, {
            reset: 0, output_string: 8, test_string: 16, query_mode: 24, set_mode: 32, set_attribute: 40, clear_screen: 48,
            set_cursor_position: 56, enable_cursor: 64, mode: 72,
        });
        check_layout!(PixelBitmask, 16, { red_mask: 0, green_mask: 4, blue_mask: 8, reserved_mask: 12 });
        check_layout!(GraphicsOutputModeInformation, 36, {
            version: 0, horizontal_resolution: 4, vertical_resolution: 8, pixel_format: 12, pixel_information: 16,
            pixels_per_scan_line: 32,
        });
        check_layout!(GraphicsOutputMode, 40, {
            max_mode: 0, mode: 4, info: 8, size_of_info: 16, frame_buffer_base: 24, frame_buffer_size: 32,
        });
        check_layout!(BltPixel, 4, { blue: 0, green: 1, red: 2, reserved: 3 });
        check_layout!(GraphicsOutput, 32, { query_mode: 0, set_mode: 8, blt: 16, mode: 24 });
        check_layout!(SimplePointerState, 16, { relative_movement_x: 0, relative_movement_y: 4, relative_movement_z: 8, left_button: 12, right_button: 13 });
        check_layout!(SimplePointerMode, 32, { resolution_x: 0, resolution_y: 8, resolution_z: 16, left_button: 24, right_button: 25 });
        check_layout!(SimplePointer, 32, { reset: 0, get_state: 8, wait_for_input: 16, mode: 24 });
        check_layout!(SimpleFileSystem, 16, { revision: 0, open_volume: 8 });
        check_layout!(FileProtocol, 120, {
            revision: 0, open: 8, close: 16, delete: 24, read: 32, write: 40, get_position: 48, set_position: 56, get_info: 64,
            set_info: 72, flush: 80, open_ex: 88, read_ex: 96, write_ex: 104, flush_ex: 112,
        });
        check_layout!(FileInfo, 80, {
            size: 0, file_size: 8, physical_size: 16, create_time: 24, last_access_time: 40, modification_time: 56, attribute: 72,
        });
        check_layout!(LoadedImage, 96, {
            revision: 0, parent_handle: 8, system_table: 16, device_handle: 24, file_path: 32, reserved: 40, load_options_size: 48,
            load_options: 56, image_base: 64, image_size: 72, image_code_type: 80, image_data_type: 84, unload: 88,
        });
        check_layout!(BlockIoMedia, 48, {
            media_id: 0, removable_media: 4, media_present: 5, logical_partition: 6, read_only: 7, write_caching: 8, block_size: 12,
            io_align: 16, last_block: 24, lowest_aligned_lba: 32, logical_blocks_per_physical_block: 40,
            optimal_transfer_length_granularity: 44,
        });
        check_layout!(BlockIo, 48, { revision: 0, media: 8, reset: 16, read_blocks: 24, write_blocks: 32, flush_blocks: 40 });
        check_layout!(LoadFile2, 8, { load_file: 0 });
    }

    #[test]
    fn device_path_layouts() {
        check_layout!(DevicePathProtocol, 4, { ty: 0, sub_type: 1, length: 2 });
        check_layout!(InitrdDevicePath, 24, { vendor: 0, guid: 4, end: 20 });
    }

    #[test]
    fn device_path_nodes() {
        let n = DevicePathProtocol::new(DP_TYPE_MEDIA, DP_SUBTYPE_MEDIA_FILE_PATH, 0x0123);
        assert_eq!(n.length, [0x23, 0x01]);
        assert_eq!(n.node_length(), 0x123);
        assert!(!n.is_end());
        let e = DevicePathProtocol::new(DP_TYPE_END, DP_SUBTYPE_END_ENTIRE, 4);
        assert!(e.is_end() && e.is_end_entire());
        let i = DevicePathProtocol::new(DP_TYPE_END, DP_SUBTYPE_END_INSTANCE, 4);
        assert!(i.is_end() && !i.is_end_entire());
    }

    #[cfg(target_endian = "little")]
    #[test]
    fn initrd_device_path_bytes() {
        let raw: [u8; 24] = unsafe { core::mem::transmute(InitrdDevicePath::VALUE) };
        assert_eq!(&raw[..4], &[0x04, 0x03, 20, 0]);
        assert_eq!(&raw[4..20], &LINUX_EFI_INITRD_MEDIA_GUID.to_bytes());
        assert_eq!(&raw[20..], &[0x7f, 0xff, 4, 0]);
    }

    #[test]
    fn constants() {
        assert_eq!(FILE_MODE_CREATE, 1 << 63);
        assert_eq!(FILE_MODE_READ | FILE_MODE_WRITE, 3);
        assert_eq!(FILE_DIRECTORY, 0x10);
        assert_eq!(PIXEL_BLT_ONLY, 3);
        assert_eq!(BltOperation::BufferToVideo as u32, 2);
        assert_eq!(SCAN_ESC, 0x17);
    }
}

// ---- Serial port ---------------------------------------------------------------------------------------------------------------

/// `EFI_SERIAL_IO_PROTOCOL`: a serial port used directly, without the text console. Only `Write` is described; the other slots keep
/// the offsets exact.
#[repr(C)]
pub struct SerialIo {
    /// Revision of the protocol.
    pub revision: u32,
    /// `Reset`.
    pub reset: *const c_void,
    /// `SetAttributes`.
    pub set_attributes: *const c_void,
    /// `SetControl`.
    pub set_control: *const c_void,
    /// `GetControl`.
    pub get_control: *const c_void,
    /// `Write`: `buffer_size` is bytes in, bytes written out.
    pub write: unsafe extern "efiapi" fn(this: *mut SerialIo, buffer_size: *mut usize, buffer: *const c_void) -> Status,
    /// `Read`.
    pub read: *const c_void,
    /// `Mode`.
    pub mode: *const c_void,
}

/// Shim's lock protocol (`SHIM_LOCK_PROTOCOL_GUID`): `verify` checks an image's bytes against the MOK list and shim's own certificate.
/// Only the first member is used here; the others are shim's `Hash` and `Context` (never called).
#[repr(C)]
pub struct ShimLock {
    /// `Verify(buffer, size)`: `SUCCESS` when the image is trusted.
    pub verify: unsafe extern "efiapi" fn(buffer: *mut c_void, size: u32) -> Status,
    /// `Hash`, not used.
    pub hash: *mut c_void,
    /// `Context`, not used.
    pub context: *mut c_void,
}

/// `EFI_SECURITY_ARCH_PROTOCOL`: the firmware calls `file_authentication_state` for an image it loads from a path.
#[repr(C)]
pub struct SecurityArch {
    /// `FileAuthenticationState`.
    pub file_authentication_state: unsafe extern "efiapi" fn(this: *const SecurityArch, status: u32, file: *const DevicePathProtocol) -> Status,
}

/// `EFI_SECURITY2_ARCH_PROTOCOL`: the firmware calls `file_authentication` with the image's bytes before it loads it.
#[repr(C)]
pub struct Security2Arch {
    /// `FileAuthentication`.
    pub file_authentication:
        unsafe extern "efiapi" fn(this: *const Security2Arch, file: *const DevicePathProtocol, buffer: *mut c_void, size: usize, boot_policy: Boolean) -> Status,
}
