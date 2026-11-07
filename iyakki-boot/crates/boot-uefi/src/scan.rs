// SPDX-License-Identifier: MIT OR Apache-2.0
//! Find other systems: the EFI loaders on every FAT volume the firmware shows. Read-only: nothing is ever written to another disk.

use crate::fs::Volume;
use crate::sys::{connect_all, handles_with, protocol};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use boot_core::discover::{Discovered, Source, Target, Trust, UnsignedReason};
use uefi_sys::*;

/// At most this many loaders are offered.
const MAX_FOUND: usize = 24;
/// At most this many directories of `\EFI` are looked at per volume.
const MAX_DIRS: usize = 24;
/// Helper programs that are never systems of their own.
const SKIP: [&str; 6] = ["mmx64.efi", "fbx64.efi", "mmaa64.efi", "fbaa64.efi", "mmia32.efi", "fbia32.efi"];
const OS_RELEASE: [&str; 3] = ["/etc/os-release", "/usr/lib/os-release", "/os-release"];

/// One loader and how to reach it.
pub struct Found {
    /// What the core's discovery model needs.
    pub disc: Discovered,
    /// The volume it is on.
    pub volume: Handle,
    /// Its path on that volume.
    pub path: String,
}

/// The disk index and partition number of a volume: partitions with the same device path before the partition node are one disk.
fn disk_part(handle: Handle, disks: &mut Vec<Vec<u8>>) -> (u8, u8) {
    let Some(first) = protocol::<DevicePathProtocol>(handle, &DEVICE_PATH_PROTOCOL_GUID) else { return (0, 0) };
    let mut p = first;
    let mut prefix: Vec<u8> = Vec::new();
    unsafe {
        while !(*p).is_end_entire() {
            let n = (*p).node_length() as usize;
            if n < 4 {
                break;
            }
            let raw = core::slice::from_raw_parts(p as *const u8, n);
            if (*p).ty == DP_TYPE_MEDIA && (*p).sub_type == DP_SUBTYPE_MEDIA_HARD_DRIVE && n >= 8 {
                let part = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]).min(255) as u8;
                let disk = match disks.iter().position(|d| *d == prefix) {
                    Some(i) => i,
                    None => {
                        disks.push(prefix);
                        disks.len() - 1
                    }
                };
                return (disk.min(255) as u8, part);
            }
            prefix.extend_from_slice(raw);
            p = (p as *const u8).add(n) as *mut DevicePathProtocol;
        }
    }
    (0, 0)
}

/// Does the PE file carry an Authenticode signature (a certificate table entry)? Not a check against the firmware's keys: the
/// firmware does that when the image is loaded.
fn has_signature(head: &[u8]) -> bool {
    let u32at = |o: usize| head.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let Some(pe) = u32at(0x3c).map(|v| v as usize) else { return false };
    if head.get(pe..pe + 4) != Some(b"PE\0\0") {
        return false;
    }
    let opt = pe + 24;
    let dirs = match head.get(opt..opt + 2) {
        Some([0x0b, 0x02]) => opt + 112,
        Some([0x0b, 0x01]) => opt + 96,
        _ => return false,
    };
    // data directory 4 is the certificate table: file offset and size
    matches!((u32at(dirs + 4 * 8), u32at(dirs + 4 * 8 + 4)), (Some(off), Some(sz)) if off != 0 && sz != 0)
}

fn lower(s: &str) -> String {
    s.to_ascii_lowercase()
}

/// Scan every simple-file-system volume. `own_volume` and `own_path` name the running loader, which is skipped.
pub fn scan(own_volume: Handle, own_path: &str) -> Vec<Found> {
    connect_all();
    let mut out: Vec<Found> = Vec::new();
    let mut disks: Vec<Vec<u8>> = Vec::new();
    for h in handles_with(&SIMPLE_FILE_SYSTEM_PROTOCOL_GUID) {
        let (disk, part) = disk_part(h, &mut disks);
        let Some(vol) = Volume::open(h) else { continue };
        let mut paths: Vec<String> = Vec::new();
        if vol.exists("/EFI/Microsoft/Boot/bootmgfw.efi") {
            paths.push("/EFI/Microsoft/Boot/bootmgfw.efi".to_string());
        }
        if let Some(dirs) = vol.list("/EFI") {
            for d in dirs.iter().filter(|d| d.dir).take(MAX_DIRS) {
                let dir = alloc::format!("/EFI/{}", d.name);
                for f in vol.list(&dir).unwrap_or_default().iter().filter(|f| !f.dir) {
                    let l = lower(&f.name);
                    if l.ends_with(".efi") && !SKIP.contains(&l.as_str()) {
                        paths.push(alloc::format!("{dir}/{}", f.name));
                    }
                }
            }
        }
        let mut os_release: Option<String> = None;
        let mut tried = false;
        for path in paths {
            if h == own_volume && lower(&path) == lower(own_path) {
                continue;
            }
            if out.len() >= MAX_FOUND {
                return out;
            }
            if !tried {
                tried = true;
                os_release = OS_RELEASE
                    .iter()
                    .find_map(|p| vol.read(p, 4096))
                    .and_then(|b| String::from_utf8(b).ok());
            }
            let head = vol.read(&path, 4096).unwrap_or_default();
            let trust = if has_signature(&head) { Trust::Signed } else { Trust::Unsigned(UnsignedReason::NoSignature) };
            let target = if h == own_volume { Target::BootVolume { path: path.clone() } } else { Target::EfiChain { disk, part, path: path.clone() } };
            out.push(Found {
                disc: Discovered { target, source: Source::EspScan, trust, os_release: os_release.clone() },
                volume: h,
                path,
            });
        }
    }
    out
}
