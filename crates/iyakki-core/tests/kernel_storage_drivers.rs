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

#[test]
fn the_x86_kernel_builds_in_dm_crypt_with_xts_aes_ni_and_loop_for_encrypted_homes() {
    // F-STORE-05: a container file is attached to a loop device and mapped through dm-crypt (aes-xts-plain64) at sign-in. Built in, not
    // modules: nothing may depend on a module being loaded at that moment. The Mac variant includes this fragment, and the pc image takes
    // its kernel from the mac build (repack --kernel-from target/buildroot-mac), so one fragment covers every x86-64 image.
    let t = fragment("dist/buildroot/board/iyakki/linux.fragment");
    for want in ["CONFIG_MD=y", "CONFIG_BLK_DEV_DM=y", "CONFIG_DM_CRYPT=y", "CONFIG_BLK_DEV_LOOP=y", "CONFIG_CRYPTO_XTS=y", "CONFIG_CRYPTO_AES_NI_INTEL=y"] {
        assert!(t.lines().any(|l| l == want), "linux.fragment lacks {want}");
    }
    assert!(!t.lines().any(|l| l.starts_with("CONFIG_DM_CRYPT=m") || l.starts_with("CONFIG_CRYPTO_XTS=m")), "must be built in");
}
