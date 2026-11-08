//! The x86-64 kernel must see the disks of current laptops without a BIOS change: NVMe, SATA, USB storage and the Intel VMD bridge
//! ("Intel RST/VMD" or RAID mode on HP, Dell and Lenovo Intel laptops since the 11th generation hides the NVMe disk from a kernel
//! without `CONFIG_VMD`; the installed system then stops at "still waiting for the IYAKKI drive"). Built in, not modules: stage 1 finds
//! the volumes before any module is loaded.

use std::path::Path;

fn fragment(rel: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn the_x86_kernel_builds_in_nvme_sata_usb_storage_and_the_intel_vmd_bridge() {
    let t = fragment("dist/buildroot/board/iyakki/linux.fragment");
    for want in ["CONFIG_BLK_DEV_NVME=y", "CONFIG_SATA_AHCI=y", "CONFIG_USB_STORAGE=y", "CONFIG_USB_UAS=y", "CONFIG_VMD=y"] {
        assert!(t.lines().any(|l| l == want), "linux.fragment lacks {want}");
    }
    assert!(!t.lines().any(|l| l == "CONFIG_VMD=m" || l == "# CONFIG_VMD is not set"), "VMD must be built in");
}

#[test]
fn the_hardware_guide_tells_the_person_about_vmd_only_as_a_fallback() {
    // the guide must not send people to the BIOS first: the driver is in the kernel, AHCI is the fallback for an older image
    let d = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/HARDWARE.md")).unwrap();
    assert!(d.contains("VMD"), "docs/HARDWARE.md says nothing about Intel VMD");
    assert!(d.contains("vmd driver") || d.contains("VMD driver"), "docs/HARDWARE.md must say the VMD driver is built in");
}
