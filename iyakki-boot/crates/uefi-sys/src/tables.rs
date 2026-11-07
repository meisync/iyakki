// SPDX-License-Identifier: MIT OR Apache-2.0
//! System table, boot services, runtime services and the small types they use.

use crate::base::*;
use crate::proto::{DevicePathProtocol, SimpleTextInput, SimpleTextOutput};
use core::ffi::c_void;

/// `"IBI SYST"`: signature of the system table header.
pub const SYSTEM_TABLE_SIGNATURE: u64 = 0x5453_5953_2049_4249;
/// `"BOOTSERV"`.
pub const BOOT_SERVICES_SIGNATURE: u64 = 0x5652_4553_544f_4f42;
/// `"RUNTSERV"`.
pub const RUNTIME_SERVICES_SIGNATURE: u64 = 0x5652_4553_544e_5552;

/// `EFI_TABLE_HEADER`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TableHeader {
    /// Table signature.
    pub signature: u64,
    /// Specification revision (major << 16 | minor).
    pub revision: u32,
    /// Size of the whole table.
    pub header_size: u32,
    /// CRC-32 of the table.
    pub crc32: u32,
    /// Zero.
    pub reserved: u32,
}

/// `EFI_CONFIGURATION_TABLE`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ConfigurationTable {
    /// Which table.
    pub vendor_guid: Guid,
    /// Where it is.
    pub vendor_table: *mut c_void,
}

/// `EFI_SYSTEM_TABLE`.
#[repr(C)]
pub struct SystemTable {
    /// Header.
    pub hdr: TableHeader,
    /// UCS-2 vendor string.
    pub firmware_vendor: *const Char16,
    /// Vendor revision.
    pub firmware_revision: u32,
    /// Console input handle.
    pub console_in_handle: Handle,
    /// Console input.
    pub con_in: *mut SimpleTextInput,
    /// Console output handle.
    pub console_out_handle: Handle,
    /// Console output.
    pub con_out: *mut SimpleTextOutput,
    /// Standard error handle.
    pub standard_error_handle: Handle,
    /// Standard error output.
    pub std_err: *mut SimpleTextOutput,
    /// Runtime services.
    pub runtime_services: *mut RuntimeServices,
    /// Boot services (invalid after `exit_boot_services`).
    pub boot_services: *mut BootServices,
    /// Entries in `configuration_table`.
    pub number_of_table_entries: usize,
    /// Configuration tables.
    pub configuration_table: *mut ConfigurationTable,
}

/// `EFI_TIME`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Time {
    /// 1900 to 9999.
    pub year: u16,
    /// 1 to 12.
    pub month: u8,
    /// 1 to 31.
    pub day: u8,
    /// 0 to 23.
    pub hour: u8,
    /// 0 to 59.
    pub minute: u8,
    /// 0 to 59.
    pub second: u8,
    /// Padding.
    pub pad1: u8,
    /// 0 to 999,999,999.
    pub nanosecond: u32,
    /// Minutes from UTC, or 2047 for unspecified.
    pub time_zone: i16,
    /// Daylight flags.
    pub daylight: u8,
    /// Padding.
    pub pad2: u8,
}

/// `EFI_TIME_CAPABILITIES`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct TimeCapabilities {
    /// Counts per second.
    pub resolution: u32,
    /// Accuracy in parts per million.
    pub accuracy: u32,
    /// Sets to zero clear sub-second.
    pub sets_to_zero: Boolean,
}

/// `EFI_RESET_TYPE`.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetType {
    /// Reset, keep memory.
    Cold = 0,
    /// Warm reset.
    Warm = 1,
    /// Power off.
    Shutdown = 2,
    /// Platform specific.
    PlatformSpecific = 3,
}

/// `EFI_MEMORY_TYPE`.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryType {
    /// Reserved.
    Reserved = 0,
    /// Loader code.
    LoaderCode = 1,
    /// Loader data (default for allocations by applications).
    LoaderData = 2,
    /// Boot services code.
    BootServicesCode = 3,
    /// Boot services data.
    BootServicesData = 4,
    /// Runtime services code.
    RuntimeServicesCode = 5,
    /// Runtime services data.
    RuntimeServicesData = 6,
    /// Free memory.
    Conventional = 7,
    /// Memory with errors.
    Unusable = 8,
    /// ACPI reclaim.
    AcpiReclaim = 9,
    /// ACPI NVS.
    AcpiNvs = 10,
    /// Memory mapped I/O.
    MemoryMappedIo = 11,
    /// Memory mapped I/O port space.
    MemoryMappedIoPortSpace = 12,
    /// PAL code.
    PalCode = 13,
    /// Persistent memory.
    Persistent = 14,
}

/// `EFI_ALLOCATE_TYPE`.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocateType {
    /// Any pages.
    AnyPages = 0,
    /// At or below an address.
    MaxAddress = 1,
    /// Exactly at an address.
    Address = 2,
}

/// `EFI_LOCATE_SEARCH_TYPE`.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocateSearchType {
    /// Every handle.
    AllHandles = 0,
    /// By register notify.
    ByRegisterNotify = 1,
    /// Handles supporting a protocol.
    ByProtocol = 2,
}

/// `EFI_MEMORY_DESCRIPTOR` (the real stride is returned by `get_memory_map`; use it, not `size_of`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MemoryDescriptor {
    /// `MemoryType` value.
    pub ty: u32,
    /// Start of the range (4 KiB aligned).
    pub physical_start: PhysicalAddress,
    /// Virtual start.
    pub virtual_start: VirtualAddress,
    /// Size in 4 KiB pages.
    pub number_of_pages: u64,
    /// Attribute bits.
    pub attribute: u64,
}

/// `EFI_OPEN_PROTOCOL_INFORMATION_ENTRY`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OpenProtocolInformationEntry {
    /// Agent that opened it.
    pub agent_handle: Handle,
    /// Controller it was opened for.
    pub controller_handle: Handle,
    /// Attributes used.
    pub attributes: u32,
    /// Open count.
    pub open_count: u32,
}

/// `OpenProtocol` attribute: like `HandleProtocol`.
pub const OPEN_PROTOCOL_BY_HANDLE_PROTOCOL: u32 = 0x1;
/// `OpenProtocol` attribute: get without tracking.
pub const OPEN_PROTOCOL_GET_PROTOCOL: u32 = 0x2;
/// `OpenProtocol` attribute: only test.
pub const OPEN_PROTOCOL_TEST_PROTOCOL: u32 = 0x4;
/// `OpenProtocol` attribute: by child controller.
pub const OPEN_PROTOCOL_BY_CHILD_CONTROLLER: u32 = 0x8;
/// `OpenProtocol` attribute: by driver.
pub const OPEN_PROTOCOL_BY_DRIVER: u32 = 0x10;
/// `OpenProtocol` attribute: exclusive.
pub const OPEN_PROTOCOL_EXCLUSIVE: u32 = 0x20;

/// `EVT_TIMER`.
pub const EVT_TIMER: u32 = 0x8000_0000;
/// `EVT_NOTIFY_WAIT`.
pub const EVT_NOTIFY_WAIT: u32 = 0x100;
/// `EVT_NOTIFY_SIGNAL`.
pub const EVT_NOTIFY_SIGNAL: u32 = 0x200;

/// `EFI_TIMER_DELAY`.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerDelay {
    /// Cancel.
    Cancel = 0,
    /// Periodic (100 ns units).
    Periodic = 1,
    /// Once (100 ns units).
    Relative = 2,
}

/// Event notify function.
pub type EventNotify = unsafe extern "efiapi" fn(event: Event, context: *mut c_void);

/// `EFI_BOOT_SERVICES` (UEFI 2.x layout; slots we do not use are opaque pointers).
#[repr(C)]
pub struct BootServices {
    /// Header.
    pub hdr: TableHeader,
    /// `RaiseTPL`.
    pub raise_tpl: unsafe extern "efiapi" fn(new_tpl: usize) -> usize,
    /// `RestoreTPL`.
    pub restore_tpl: unsafe extern "efiapi" fn(old_tpl: usize),
    /// `AllocatePages`.
    pub allocate_pages: unsafe extern "efiapi" fn(ty: AllocateType, mem: MemoryType, pages: usize, addr: *mut PhysicalAddress) -> Status,
    /// `FreePages`.
    pub free_pages: unsafe extern "efiapi" fn(addr: PhysicalAddress, pages: usize) -> Status,
    /// `GetMemoryMap`.
    pub get_memory_map: unsafe extern "efiapi" fn(
        size: *mut usize,
        map: *mut MemoryDescriptor,
        key: *mut usize,
        desc_size: *mut usize,
        desc_version: *mut u32,
    ) -> Status,
    /// `AllocatePool`.
    pub allocate_pool: unsafe extern "efiapi" fn(mem: MemoryType, size: usize, buffer: *mut *mut c_void) -> Status,
    /// `FreePool`.
    pub free_pool: unsafe extern "efiapi" fn(buffer: *mut c_void) -> Status,
    /// `CreateEvent`.
    pub create_event:
        unsafe extern "efiapi" fn(ty: u32, tpl: usize, notify: Option<EventNotify>, ctx: *mut c_void, event: *mut Event) -> Status,
    /// `SetTimer`.
    pub set_timer: unsafe extern "efiapi" fn(event: Event, ty: TimerDelay, trigger_time: u64) -> Status,
    /// `WaitForEvent`.
    pub wait_for_event: unsafe extern "efiapi" fn(count: usize, events: *mut Event, index: *mut usize) -> Status,
    /// `SignalEvent`.
    pub signal_event: unsafe extern "efiapi" fn(event: Event) -> Status,
    /// `CloseEvent`.
    pub close_event: unsafe extern "efiapi" fn(event: Event) -> Status,
    /// `CheckEvent`.
    pub check_event: unsafe extern "efiapi" fn(event: Event) -> Status,
    /// `InstallProtocolInterface`.
    pub install_protocol_interface:
        unsafe extern "efiapi" fn(handle: *mut Handle, protocol: *const Guid, interface_type: u32, interface: *mut c_void) -> Status,
    /// `ReinstallProtocolInterface` (unused).
    pub reinstall_protocol_interface: *const c_void,
    /// `UninstallProtocolInterface`.
    pub uninstall_protocol_interface: unsafe extern "efiapi" fn(handle: Handle, protocol: *const Guid, interface: *mut c_void) -> Status,
    /// `HandleProtocol`.
    pub handle_protocol: unsafe extern "efiapi" fn(handle: Handle, protocol: *const Guid, interface: *mut *mut c_void) -> Status,
    /// Reserved.
    pub reserved: *const c_void,
    /// `RegisterProtocolNotify` (unused).
    pub register_protocol_notify: *const c_void,
    /// `LocateHandle`.
    pub locate_handle: unsafe extern "efiapi" fn(
        search: LocateSearchType,
        protocol: *const Guid,
        key: *mut c_void,
        size: *mut usize,
        buffer: *mut Handle,
    ) -> Status,
    /// `LocateDevicePath`.
    pub locate_device_path:
        unsafe extern "efiapi" fn(protocol: *const Guid, path: *mut *mut DevicePathProtocol, device: *mut Handle) -> Status,
    /// `InstallConfigurationTable` (unused).
    pub install_configuration_table: *const c_void,
    /// `LoadImage`.
    pub load_image: unsafe extern "efiapi" fn(
        boot_policy: Boolean,
        parent: Handle,
        path: *const DevicePathProtocol,
        source: *const c_void,
        source_size: usize,
        image: *mut Handle,
    ) -> Status,
    /// `StartImage`.
    pub start_image: unsafe extern "efiapi" fn(image: Handle, exit_data_size: *mut usize, exit_data: *mut *mut Char16) -> Status,
    /// `Exit`.
    pub exit: unsafe extern "efiapi" fn(image: Handle, status: Status, data_size: usize, data: *mut Char16) -> Status,
    /// `UnloadImage`.
    pub unload_image: unsafe extern "efiapi" fn(image: Handle) -> Status,
    /// `ExitBootServices`.
    pub exit_boot_services: unsafe extern "efiapi" fn(image: Handle, map_key: usize) -> Status,
    /// `GetNextMonotonicCount` (unused).
    pub get_next_monotonic_count: *const c_void,
    /// `Stall` (microseconds).
    pub stall: unsafe extern "efiapi" fn(microseconds: usize) -> Status,
    /// `SetWatchdogTimer`.
    pub set_watchdog_timer: unsafe extern "efiapi" fn(timeout: usize, code: u64, data_size: usize, data: *const Char16) -> Status,
    /// `ConnectController`.
    pub connect_controller:
        unsafe extern "efiapi" fn(controller: Handle, drivers: *mut Handle, remaining: *mut DevicePathProtocol, recursive: Boolean) -> Status,
    /// `DisconnectController` (unused).
    pub disconnect_controller: *const c_void,
    /// `OpenProtocol`.
    pub open_protocol: unsafe extern "efiapi" fn(
        handle: Handle,
        protocol: *const Guid,
        interface: *mut *mut c_void,
        agent: Handle,
        controller: Handle,
        attributes: u32,
    ) -> Status,
    /// `CloseProtocol`.
    pub close_protocol: unsafe extern "efiapi" fn(handle: Handle, protocol: *const Guid, agent: Handle, controller: Handle) -> Status,
    /// `OpenProtocolInformation`.
    pub open_protocol_information: unsafe extern "efiapi" fn(
        handle: Handle,
        protocol: *const Guid,
        buffer: *mut *mut OpenProtocolInformationEntry,
        count: *mut usize,
    ) -> Status,
    /// `ProtocolsPerHandle`.
    pub protocols_per_handle:
        unsafe extern "efiapi" fn(handle: Handle, buffer: *mut *mut *const Guid, count: *mut usize) -> Status,
    /// `LocateHandleBuffer` (result must be freed with `free_pool`).
    pub locate_handle_buffer: unsafe extern "efiapi" fn(
        search: LocateSearchType,
        protocol: *const Guid,
        key: *mut c_void,
        count: *mut usize,
        buffer: *mut *mut Handle,
    ) -> Status,
    /// `LocateProtocol`.
    pub locate_protocol: unsafe extern "efiapi" fn(protocol: *const Guid, registration: *mut c_void, interface: *mut *mut c_void) -> Status,
    /// `InstallMultipleProtocolInterfaces` (variadic, unused).
    pub install_multiple_protocol_interfaces: *const c_void,
    /// `UninstallMultipleProtocolInterfaces` (variadic, unused).
    pub uninstall_multiple_protocol_interfaces: *const c_void,
    /// `CalculateCrc32`.
    pub calculate_crc32: unsafe extern "efiapi" fn(data: *const c_void, size: usize, crc: *mut u32) -> Status,
    /// `CopyMem`.
    pub copy_mem: unsafe extern "efiapi" fn(dest: *mut c_void, src: *const c_void, len: usize),
    /// `SetMem`.
    pub set_mem: unsafe extern "efiapi" fn(buffer: *mut c_void, size: usize, value: u8),
    /// `CreateEventEx` (unused).
    pub create_event_ex: *const c_void,
}

/// `EFI_RUNTIME_SERVICES` (subset typed; the rest are opaque slots).
#[repr(C)]
pub struct RuntimeServices {
    /// Header.
    pub hdr: TableHeader,
    /// `GetTime`.
    pub get_time: unsafe extern "efiapi" fn(time: *mut Time, caps: *mut TimeCapabilities) -> Status,
    /// `SetTime` (unused).
    pub set_time: *const c_void,
    /// `GetWakeupTime` (unused).
    pub get_wakeup_time: *const c_void,
    /// `SetWakeupTime` (unused).
    pub set_wakeup_time: *const c_void,
    /// `SetVirtualAddressMap` (unused).
    pub set_virtual_address_map: *const c_void,
    /// `ConvertPointer` (unused).
    pub convert_pointer: *const c_void,
    /// `GetVariable`.
    pub get_variable: unsafe extern "efiapi" fn(
        name: *const Char16,
        vendor: *const Guid,
        attributes: *mut u32,
        data_size: *mut usize,
        data: *mut c_void,
    ) -> Status,
    /// `GetNextVariableName`.
    pub get_next_variable_name:
        unsafe extern "efiapi" fn(name_size: *mut usize, name: *mut Char16, vendor: *mut Guid) -> Status,
    /// `SetVariable` (size 0 with no attributes deletes).
    pub set_variable: unsafe extern "efiapi" fn(
        name: *const Char16,
        vendor: *const Guid,
        attributes: u32,
        data_size: usize,
        data: *const c_void,
    ) -> Status,
    /// `GetNextHighMonotonicCount` (unused).
    pub get_next_high_monotonic_count: *const c_void,
    /// `ResetSystem`.
    pub reset_system: unsafe extern "efiapi" fn(ty: ResetType, status: Status, data_size: usize, data: *const c_void) -> !,
    /// `UpdateCapsule` (unused).
    pub update_capsule: *const c_void,
    /// `QueryCapsuleCapabilities` (unused).
    pub query_capsule_capabilities: *const c_void,
    /// `QueryVariableInfo`.
    pub query_variable_info:
        unsafe extern "efiapi" fn(attributes: u32, max_storage: *mut u64, remaining: *mut u64, max_variable: *mut u64) -> Status,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check_layout;

    #[test]
    fn signatures_spell_the_ascii_names() {
        assert_eq!(&SYSTEM_TABLE_SIGNATURE.to_le_bytes(), b"IBI SYST");
        assert_eq!(&BOOT_SERVICES_SIGNATURE.to_le_bytes(), b"BOOTSERV");
        assert_eq!(&RUNTIME_SERVICES_SIGNATURE.to_le_bytes(), b"RUNTSERV");
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn x86_64_layouts() {
        check_layout!(TableHeader, 24, { signature: 0, revision: 8, header_size: 12, crc32: 16, reserved: 20 });
        check_layout!(ConfigurationTable, 24, { vendor_guid: 0, vendor_table: 16 });
        check_layout!(SystemTable, 120, {
            hdr: 0, firmware_vendor: 24, firmware_revision: 32, console_in_handle: 40, con_in: 48, console_out_handle: 56,
            con_out: 64, standard_error_handle: 72, std_err: 80, runtime_services: 88, boot_services: 96,
            number_of_table_entries: 104, configuration_table: 112,
        });
        check_layout!(Time, 16, { year: 0, month: 2, day: 3, hour: 4, minute: 5, second: 6, nanosecond: 8, time_zone: 12, daylight: 14 });
        check_layout!(MemoryDescriptor, 40, { ty: 0, physical_start: 8, virtual_start: 16, number_of_pages: 24, attribute: 32 });
        check_layout!(OpenProtocolInformationEntry, 24, { agent_handle: 0, controller_handle: 8, attributes: 16, open_count: 20 });
        check_layout!(RuntimeServices, 136, {
            get_time: 24, set_time: 32, get_wakeup_time: 40, set_wakeup_time: 48, set_virtual_address_map: 56, convert_pointer: 64,
            get_variable: 72, get_next_variable_name: 80, set_variable: 88, get_next_high_monotonic_count: 96, reset_system: 104,
            update_capsule: 112, query_capsule_capabilities: 120, query_variable_info: 128,
        });
        check_layout!(BootServices, 376, {
            raise_tpl: 24, restore_tpl: 32, allocate_pages: 40, free_pages: 48, get_memory_map: 56, allocate_pool: 64, free_pool: 72,
            create_event: 80, set_timer: 88, wait_for_event: 96, signal_event: 104, close_event: 112, check_event: 120,
            install_protocol_interface: 128, reinstall_protocol_interface: 136, uninstall_protocol_interface: 144,
            handle_protocol: 152, reserved: 160, register_protocol_notify: 168, locate_handle: 176, locate_device_path: 184,
            install_configuration_table: 192, load_image: 200, start_image: 208, exit: 216, unload_image: 224,
            exit_boot_services: 232, get_next_monotonic_count: 240, stall: 248, set_watchdog_timer: 256,
            connect_controller: 264, disconnect_controller: 272, open_protocol: 280, close_protocol: 288,
            open_protocol_information: 296, protocols_per_handle: 304, locate_handle_buffer: 312, locate_protocol: 320,
            install_multiple_protocol_interfaces: 328, uninstall_multiple_protocol_interfaces: 336, calculate_crc32: 344,
            copy_mem: 352, set_mem: 360, create_event_ex: 368,
        });
    }

    #[test]
    fn enum_values_match_the_specification() {
        assert_eq!(MemoryType::LoaderData as u32, 2);
        assert_eq!(MemoryType::Conventional as u32, 7);
        assert_eq!(MemoryType::Persistent as u32, 14);
        assert_eq!(AllocateType::Address as u32, 2);
        assert_eq!(LocateSearchType::ByProtocol as u32, 2);
        assert_eq!(ResetType::Shutdown as u32, 2);
        assert_eq!(TimerDelay::Relative as u32, 2);
    }
}
