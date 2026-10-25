//! A made-up machine for the DISKS tests (the engine's and the tile's): `/sys` and `/proc` as text, drives as byte buffers, a mount
//! runner that only records. Nothing here opens a real device, mounts anything or runs a program.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::disks::{DiskEnv, Disks, MountFlags};

#[derive(Default)]
pub struct St {
    pub files: BTreeMap<String, String>,
    pub links: BTreeMap<String, String>,
    pub canon: BTreeMap<String, String>,
    pub dirs: BTreeSet<String>,
    /// Whole drives by device path.
    pub devs: BTreeMap<String, Vec<u8>>,
    /// Partition device -> (drive device, offset, length), for drives whose table was read again.
    pub alias: BTreeMap<String, (String, u64, u64)>,
    pub base_mounts: Vec<String>,
    pub mounted: Vec<(String, String, String, MountFlags, String)>,
    pub mount_errors: HashMap<String, String>,
    pub busy: HashSet<String>,
    pub space: HashMap<String, (u64, u64)>,
    pub swaps: String,
    /// Every call that changes anything, in order: "mount", "umount", "sync", "write", "zero", "mke2fs", "reread", "sys", "mkdir", "rmdir".
    pub calls: Vec<String>,
    pub fs_list: String,
    pub next_mm: u32,
    pub reread_fails: bool,
}

#[derive(Default)]
pub struct Fake {
    pub st: Mutex<St>,
}

fn children(prefix: &str, keys: impl Iterator<Item = String>) -> BTreeSet<String> {
    let p = if prefix.ends_with('/') { prefix.to_string() } else { format!("{prefix}/") };
    keys.filter_map(|k| k.strip_prefix(&p).map(|r| r.split('/').next().unwrap_or("").to_string())).filter(|s| !s.is_empty()).collect()
}

impl Fake {
    pub fn new() -> Arc<Fake> {
        let f = Fake::default();
        f.st.lock().unwrap().fs_list = "nodev\tsysfs\nnodev\tproc\nnodev\ttmpfs\n\text4\n\tvfat\n\tiso9660\n".into();
        Arc::new(f)
    }

    pub fn disks(self: &Arc<Self>) -> Disks {
        Disks::new(self.clone())
    }

    pub fn calls(&self) -> Vec<String> {
        self.st.lock().unwrap().calls.clone()
    }

    pub fn wrote(&self) -> bool {
        self.calls().iter().any(|c| c.starts_with("write") || c.starts_with("zero") || c.starts_with("mke2fs"))
    }

    pub fn set(&self, path: &str, text: &str) {
        self.st.lock().unwrap().files.insert(path.into(), text.into());
    }

    /// A drive in `/sys/block`: size in 512-byte sectors, vendor, model; `canon` is the real sysfs path (USB drives have `/usb` in it).
    pub fn add_disk(&self, name: &str, sectors: u64, removable: bool, canon: &str, vendor: &str, model: &str, sector_size: u32) {
        let mut s = self.st.lock().unwrap();
        s.next_mm += 1;
        let major = s.next_mm;
        let b = format!("/sys/block/{name}");
        for (k, v) in [("size", sectors.to_string()), ("removable", (removable as u8).to_string()), ("ro", "0".into()), ("queue/logical_block_size", sector_size.to_string()), ("device/vendor", vendor.into()), ("device/model", model.into()), ("dev", format!("{major}:0"))] {
            s.files.insert(format!("{b}/{k}"), format!("{v}\n"));
        }
        s.canon.insert(b, canon.into());
        s.dirs.insert(format!("/sys/block/{name}"));
    }

    pub fn set_serial_up(&self, canon_dir: &str, serial: &str) {
        self.set(&format!("{canon_dir}/serial"), &format!("{serial}\n"));
    }

    /// A partition of a drive with its contents (the first bytes, `probe`); also available as `/dev/<name>`.
    pub fn add_part(&self, disk: &str, name: &str, number: u32, start_sector: u64, sectors: u64, probe: Vec<u8>) {
        let mut s = self.st.lock().unwrap();
        let major = s.next_mm;
        let b = format!("/sys/block/{disk}/{name}");
        for (k, v) in [("partition", number.to_string()), ("start", start_sector.to_string()), ("size", sectors.to_string()), ("dev", format!("{major}:{number}"))] {
            s.files.insert(format!("{b}/{k}"), format!("{v}\n"));
        }
        let mut data = probe;
        data.resize(data.len().max(4096), 0);
        s.devs.insert(format!("/dev/{name}"), data);
    }

    /// A drive's own bytes (`/dev/<name>`).
    pub fn set_dev(&self, name: &str, bytes: Vec<u8>) {
        self.st.lock().unwrap().devs.insert(format!("/dev/{name}"), bytes);
    }

    pub fn dev_bytes(&self, name: &str) -> Vec<u8> {
        self.st.lock().unwrap().devs.get(&format!("/dev/{name}")).cloned().unwrap_or_default()
    }

    /// A mount that already exists: `/dev/<name>` at `mp`.
    pub fn add_mount(&self, name: &str, mp: &str, fstype: &str, opts: &str) {
        let mut s = self.st.lock().unwrap();
        s.next_mm += 100;
        let n = s.next_mm;
        s.base_mounts.push(format!("{n} 1 0:{n} / {} {opts} - {fstype} /dev/{name} rw", mp.replace(' ', "\\040")));
    }

    pub fn add_process(&self, pid: u32, comm: &str, cwd: &str, fds: &[&str]) {
        let mut s = self.st.lock().unwrap();
        s.files.insert(format!("/proc/{pid}/comm"), format!("{comm}\n"));
        s.links.insert(format!("/proc/{pid}/cwd"), cwd.into());
        s.links.insert(format!("/proc/{pid}/root"), "/".into());
        s.links.insert(format!("/proc/{pid}/exe"), "/usr/bin/prog".into());
        for (i, f) in fds.iter().enumerate() {
            s.links.insert(format!("/proc/{pid}/fd/{i}"), (*f).into());
        }
    }

    pub fn make_busy(&self, mp: &str) {
        self.st.lock().unwrap().busy.insert(mp.into());
    }

    pub fn fail_mount(&self, key: &str, err: &str) {
        self.st.lock().unwrap().mount_errors.insert(key.into(), err.into());
    }

    pub fn mounts_now(&self) -> Vec<(String, String)> {
        self.st.lock().unwrap().mounted.iter().map(|m| (m.0.clone(), m.1.clone())).collect()
    }

    pub fn last_mount(&self) -> Option<(String, String, String, MountFlags, String)> {
        self.st.lock().unwrap().mounted.last().cloned()
    }
}

fn resolve(s: &St, dev: &str) -> Option<(String, u64, u64)> {
    if let Some((d, off, len)) = s.alias.get(dev) {
        return Some((d.clone(), *off, *len));
    }
    s.devs.get(dev).map(|v| (dev.to_string(), 0, v.len() as u64))
}

impl DiskEnv for Fake {
    fn read_text(&self, path: &str) -> Option<String> {
        let s = self.st.lock().unwrap();
        match path {
            "/proc/self/mountinfo" => {
                let mut t = s.base_mounts.clone();
                for (i, m) in s.mounted.iter().enumerate() {
                    let ro = if m.3.rdonly { "ro" } else { "rw" };
                    t.push(format!("{} 1 0:{} / {} {ro} - {} {} rw", 900 + i, 900 + i, m.1.replace(' ', "\\040"), m.2, m.0));
                }
                Some(t.join("\n") + "\n")
            }
            "/proc/partitions" => Some(s.files.keys().filter(|k| k.ends_with("/size") && k.starts_with("/sys/block/")).map(|k| format!("{k}={}", s.files[k])).collect::<String>() + &s.devs.keys().cloned().collect::<String>()),
            "/proc/swaps" => Some(s.swaps.clone()),
            "/proc/filesystems" => Some(s.fs_list.clone()),
            _ => s.files.get(path).cloned(),
        }
    }

    fn list(&self, path: &str) -> Vec<String> {
        let s = self.st.lock().unwrap();
        let mut keys: Vec<String> = s.files.keys().cloned().collect();
        keys.extend(s.links.keys().cloned());
        keys.extend(s.dirs.iter().cloned());
        keys.extend(s.mounted.iter().map(|m| m.1.clone()));
        let mut v: Vec<String> = children(path, keys.into_iter()).into_iter().collect();
        if path == "/proc" {
            v.retain(|n| n.parse::<u32>().is_ok());
        }
        v
    }

    fn read_link(&self, path: &str) -> Option<String> {
        self.st.lock().unwrap().links.get(path).cloned()
    }

    fn canonical(&self, path: &str) -> Option<String> {
        self.st.lock().unwrap().canon.get(path).cloned()
    }

    fn dev_read(&self, dev: &str, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        let s = self.st.lock().unwrap();
        let (d, base, lim) = resolve(&s, dev).ok_or_else(|| format!("{dev}: no such device"))?;
        let data = &s.devs[&d];
        let from = (base + offset).min(base + lim) as usize;
        let to = ((base + offset + len as u64).min(base + lim) as usize).min(data.len());
        Ok(data.get(from..to.max(from)).unwrap_or(&[]).to_vec())
    }

    fn dev_write(&self, dev: &str, offset: u64, data: &[u8]) -> Result<(), String> {
        let mut s = self.st.lock().unwrap();
        let (d, base, lim) = resolve(&s, dev).ok_or_else(|| format!("{dev}: no such device"))?;
        if offset + data.len() as u64 > lim {
            return Err(format!("{dev}: write beyond the end"));
        }
        s.calls.push(format!("write {dev} {offset} {}", data.len()));
        let buf = s.devs.get_mut(&d).unwrap();
        let from = (base + offset) as usize;
        buf[from..from + data.len()].copy_from_slice(data);
        Ok(())
    }

    fn dev_zero(&self, dev: &str, offset: u64, len: u64) -> Result<(), String> {
        let mut s = self.st.lock().unwrap();
        let (d, base, lim) = resolve(&s, dev).ok_or_else(|| format!("{dev}: no such device"))?;
        if offset + len > lim {
            return Err(format!("{dev}: write beyond the end"));
        }
        s.calls.push(format!("zero {dev} {offset} {len}"));
        let buf = s.devs.get_mut(&d).unwrap();
        let from = (base + offset) as usize;
        buf[from..from + len as usize].fill(0);
        Ok(())
    }

    fn dev_len(&self, dev: &str) -> u64 {
        let s = self.st.lock().unwrap();
        resolve(&s, dev).map_or(0, |(_, _, l)| l)
    }

    fn space(&self, mountpoint: &str) -> Option<(u64, u64)> {
        self.st.lock().unwrap().space.get(mountpoint).copied()
    }

    fn mount(&self, dev: &str, target: &str, fstype: &str, flags: MountFlags, data: &str) -> Result<(), String> {
        let mut s = self.st.lock().unwrap();
        s.calls.push(format!("mount {dev} {target} {fstype} ro={}", flags.rdonly));
        let key = format!("{fstype}:{}", if flags.rdonly { "ro" } else { "rw" });
        if let Some(e) = s.mount_errors.get(&key).or_else(|| s.mount_errors.get(fstype)).or_else(|| s.mount_errors.get(dev)) {
            return Err(e.clone());
        }
        s.mounted.push((dev.into(), target.into(), fstype.into(), flags, data.into()));
        Ok(())
    }

    fn umount(&self, target: &str) -> Result<(), String> {
        let mut s = self.st.lock().unwrap();
        s.calls.push(format!("umount {target}"));
        if s.busy.contains(target) {
            return Err("device is busy".into());
        }
        let before = s.mounted.len();
        s.mounted.retain(|m| m.1 != target);
        if s.mounted.len() == before {
            // a mount that existed from the start
            let t = target.replace(' ', "\\040");
            s.base_mounts.retain(|l| !l.split_whitespace().nth(4).is_some_and(|mp| mp == t));
        }
        Ok(())
    }

    fn make_dir(&self, path: &str) -> Result<(), String> {
        let mut s = self.st.lock().unwrap();
        s.calls.push(format!("mkdir {path}"));
        s.dirs.insert(path.into());
        Ok(())
    }

    fn chown(&self, path: &str, uid: u32, gid: u32) -> Result<(), String> {
        self.st.lock().unwrap().calls.push(format!("chown {path} {uid} {gid}"));
        Ok(())
    }

    fn remove_dir(&self, path: &str) -> Result<(), String> {
        let mut s = self.st.lock().unwrap();
        s.calls.push(format!("rmdir {path}"));
        s.dirs.remove(path);
        Ok(())
    }

    fn sync(&self) {
        self.st.lock().unwrap().calls.push("sync".into());
    }

    fn write_sys(&self, path: &str, text: &str) -> Result<(), String> {
        let mut s = self.st.lock().unwrap();
        s.calls.push(format!("sys {path} {text}"));
        if let Some(name) = path.strip_prefix("/sys/block/").and_then(|r| r.strip_suffix("/device/delete")) {
            // the drive goes away
            let prefix = format!("/sys/block/{name}/");
            s.files.retain(|k, _| !k.starts_with(&prefix));
            s.dirs.remove(&format!("/sys/block/{name}"));
        }
        Ok(())
    }

    fn reread_table(&self, disk_dev: &str) -> Result<(), String> {
        let mut s = self.st.lock().unwrap();
        s.calls.push(format!("reread {disk_dev}"));
        if s.reread_fails {
            return Err("the kernel did not re-read the partition table (busy)".into());
        }
        let name = disk_dev.trim_start_matches("/dev/").to_string();
        let data = s.devs.get(disk_dev).cloned().unwrap_or_default();
        let ss: u32 = s.files.get(&format!("/sys/block/{name}/queue/logical_block_size")).and_then(|t| t.trim().parse().ok()).unwrap_or(512);
        // forget the old partitions
        let prefix = format!("/sys/block/{name}/");
        s.files.retain(|k, _| !(k.starts_with(&prefix) && k.split('/').nth(4).is_some_and(|c| c.starts_with(name.as_str()) && k.split('/').count() == 6)));
        s.alias.retain(|_, (d, _, _)| d != disk_dev);
        let mut cur = std::io::Cursor::new(data.clone());
        let ti = iyakki_storage::gpt::read_disk_table(&mut cur, ss, data.len() as u64);
        for p in &ti.parts {
            let pn = iyakki_storage::partition_name(&name, p.index);
            let b = format!("/sys/block/{name}/{pn}");
            let scale = ss as u64 / 512;
            for (k, v) in [("partition", p.index as u64), ("start", p.first_lba * scale), ("size", p.sectors * scale), ("dev", 0)] {
                if k == "dev" {
                    s.files.insert(format!("{b}/dev"), format!("{}:{}\n", 7, p.index));
                } else {
                    s.files.insert(format!("{b}/{k}"), format!("{v}\n"));
                }
            }
            s.alias.insert(format!("/dev/{pn}"), (disk_dev.to_string(), p.first_lba * ss as u64, p.sectors * ss as u64));
        }
        Ok(())
    }

    fn mke2fs(&self, dev: &str, label: &str, uuid: &str) -> Result<(), String> {
        let mut s = self.st.lock().unwrap();
        s.calls.push(format!("mke2fs {dev} {label} {uuid}"));
        // write the bits of a fresh ext4 superblock that the probe reads
        let (d, base, lim) = resolve(&s, dev).ok_or("no such device")?;
        if lim < 4096 {
            return Err("too small".into());
        }
        let buf = s.devs.get_mut(&d).unwrap();
        let sb = base as usize + 1024;
        buf[sb + 56..sb + 58].copy_from_slice(&[0x53, 0xEF]);
        buf[sb + 0x60..sb + 0x64].copy_from_slice(&0x2C2u32.to_le_bytes());
        buf[sb + 120..sb + 136].fill(0);
        buf[sb + 120..sb + 120 + label.len()].copy_from_slice(label.as_bytes());
        Ok(())
    }

    fn wait_node(&self, dev: &str) -> bool {
        let s = self.st.lock().unwrap();
        s.alias.contains_key(dev) || s.devs.contains_key(dev)
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// superblock fixtures
// ---------------------------------------------------------------------------------------------------------------------

fn put(b: &mut [u8], off: usize, bytes: &[u8]) {
    b[off..off + bytes.len()].copy_from_slice(bytes);
}

pub fn blank() -> Vec<u8> {
    vec![0u8; crate::disks::PROBE_BYTES + 64]
}

pub fn sb_fat32(label: &str) -> Vec<u8> {
    let mut b = blank();
    put(&mut b, 82, b"FAT32   ");
    let mut l = [b' '; 11];
    l[..label.len()].copy_from_slice(label.as_bytes());
    put(&mut b, 71, &l);
    put(&mut b, 67, &0xA1B2_C3D4u32.to_le_bytes());
    b[510] = 0x55;
    b[511] = 0xAA;
    b
}

pub fn sb_fat16(label: &str, bits: &str) -> Vec<u8> {
    let mut b = blank();
    put(&mut b, 54, format!("{bits:<8}").as_bytes());
    let mut l = [b' '; 11];
    l[..label.len()].copy_from_slice(label.as_bytes());
    put(&mut b, 43, &l);
    b[510] = 0x55;
    b[511] = 0xAA;
    b
}

pub fn sb_ext(label: &str, compat: u32, incompat: u32, ro: u32) -> Vec<u8> {
    let mut b = blank();
    put(&mut b, 1024 + 56, &[0x53, 0xEF]);
    put(&mut b, 1024 + 0x5C, &compat.to_le_bytes());
    put(&mut b, 1024 + 0x60, &incompat.to_le_bytes());
    put(&mut b, 1024 + 0x64, &ro.to_le_bytes());
    put(&mut b, 1024 + 104, &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x01]);
    put(&mut b, 1024 + 120, label.as_bytes());
    b
}

pub fn sb_ext4(label: &str) -> Vec<u8> {
    sb_ext(label, 0x4, 0x2C2, 0x46B)
}

pub fn sb_ntfs() -> Vec<u8> {
    let mut b = blank();
    put(&mut b, 3, b"NTFS    ");
    put(&mut b, 0x48, &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]);
    b[510] = 0x55;
    b[511] = 0xAA;
    b
}

pub fn sb_exfat() -> Vec<u8> {
    let mut b = blank();
    put(&mut b, 3, b"EXFAT   ");
    put(&mut b, 100, &0x1234_5678u32.to_le_bytes());
    b
}

pub fn sb_btrfs(label: &str) -> Vec<u8> {
    let mut b = blank();
    put(&mut b, 0x10040, b"_BHRfS_M");
    put(&mut b, 0x1012B, label.as_bytes());
    put(&mut b, 0x10020, &[0xAB; 16]);
    b
}

pub fn sb_xfs(label: &str) -> Vec<u8> {
    let mut b = blank();
    put(&mut b, 0, b"XFSB");
    put(&mut b, 108, label.as_bytes());
    put(&mut b, 32, &[0xCD; 16]);
    b
}

pub fn sb_f2fs() -> Vec<u8> {
    let mut b = blank();
    put(&mut b, 1024, &[0x10, 0x20, 0xF5, 0xF2]);
    for (i, c) in "phone".encode_utf16().enumerate() {
        put(&mut b, 1024 + 0x6C + 2 * i, &c.to_le_bytes());
    }
    b
}

pub fn sb_iso(label: &str) -> Vec<u8> {
    let mut b = blank();
    put(&mut b, 0x8001, b"CD001");
    let mut l = [b' '; 32];
    l[..label.len()].copy_from_slice(label.as_bytes());
    put(&mut b, 0x8028, &l);
    b
}

pub fn sb_swap() -> Vec<u8> {
    let mut b = blank();
    put(&mut b, 4086, b"SWAPSPACE2");
    b
}

pub fn sb_luks() -> Vec<u8> {
    let mut b = blank();
    put(&mut b, 0, b"LUKS\xba\xbe");
    put(&mut b, 6, &[0, 2]);
    put(&mut b, 168, b"0a1b2c3d-0000-4000-8000-000000000001");
    b
}

pub fn sb_bitlocker() -> Vec<u8> {
    let mut b = blank();
    put(&mut b, 3, b"-FVE-FS-");
    b
}

pub fn sb_apfs() -> Vec<u8> {
    let mut b = blank();
    put(&mut b, 32, b"NXSB");
    b
}

pub fn sb_hfs() -> Vec<u8> {
    let mut b = blank();
    put(&mut b, 1024, b"H+");
    b
}

const SECT: u64 = 2; // 1 KiB units are not used: sizes below are in 512-byte sectors

/// A drive with the most things on it a test needs: the system NVMe, a USB stick, a second NVMe with Windows, a SATA disk with a
/// BitLocker volume and a Mac volume, and an SD card.
pub fn machine() -> Arc<Fake> {
    let f = Fake::new();
    let gb = |n: u64| n * 1_000_000_000 / 512;
    // the system disk: ESP at /boot, ext4 at /
    f.add_disk("nvme0n1", gb(512), false, "/sys/devices/pci0000:00/0000:00:1d.0/nvme/nvme0/nvme0n1", "", "Samsung SSD 980", 512);
    f.set("/sys/block/nvme0n1/device/serial", "S64DNX0T123456\n");
    f.add_part("nvme0n1", "nvme0n1p1", 1, 2048, gb(1), sb_fat32("IYAKKI-BOOT"));
    f.add_part("nvme0n1", "nvme0n1p2", 2, 2048 + gb(1), gb(511) - 4096, sb_ext4("rootfs"));
    f.add_mount("nvme0n1p1", "/boot", "vfat", "rw");
    f.add_mount("nvme0n1p2", "/", "ext4", "rw");
    f.st.lock().unwrap().space.insert("/".into(), (300_000_000_000, 500_000_000_000));
    // a USB stick, FAT32, not mounted
    f.add_disk("sda", gb(64), true, "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-2/1-2:1.0/host6/target6:0:0/6:0:0:0/block/sda", "SanDisk", "Ultra", 512);
    f.set_serial_up("/sys/devices/pci0000:00/0000:00:14.0/usb1/1-2", "4C530001230815ABCD");
    f.add_part("sda", "sda1", 1, 2048, gb(64) - 2048, sb_fat32("STICK"));
    // a second NVMe with Windows on it (NTFS) and a recovery partition
    f.add_disk("nvme1n1", gb(1000), false, "/sys/devices/pci0000:00/0000:00:1d.1/nvme/nvme1/nvme1n1", "", "WD Black SN770", 512);
    f.set("/sys/block/nvme1n1/device/serial", "WD-WX12345678\n");
    f.add_part("nvme1n1", "nvme1n1p1", 1, 2048, gb(900), sb_ntfs());
    f.add_part("nvme1n1", "nvme1n1p2", 2, 2048 + gb(900), gb(100) - 4096, sb_bitlocker());
    // a SATA disk: LUKS, APFS and HFS+
    f.add_disk("sdb", gb(2000), false, "/sys/devices/pci0000:00/0000:00:17.0/ata1/host0/target0:0:0/0:0:0:0/block/sdb", "ATA", "ST2000DM008", 512);
    f.set("/sys/block/sdb/device/serial", "ZFL12345\n");
    f.add_part("sdb", "sdb1", 1, 2048, gb(500), sb_luks());
    f.add_part("sdb", "sdb2", 2, 2048 + gb(500), gb(500), sb_apfs());
    f.add_part("sdb", "sdb3", 3, 2048 + gb(1000), gb(500), sb_hfs());
    f.add_part("sdb", "sdb4", 4, 2048 + gb(1500), gb(400), sb_btrfs("archive"));
    // an SD card
    f.add_disk("mmcblk0", gb(32), false, "/sys/devices/pci0000:00/0000:00:1a.0/mmc_host/mmc0/mmc0:aaaa/block/mmcblk0", "", "", 512);
    f.set("/sys/block/mmcblk0/device/type", "SD\n");
    f.set("/sys/block/mmcblk0/device/serial", "0x1a2b3c4d\n");
    f.add_part("mmcblk0", "mmcblk0p1", 1, 8192, gb(32) - 8192, sb_exfat());
    let _ = SECT;
    f
}

/// A blank 128 MiB USB stick for the partition manager (`sdc`, serial `SERIAL-SDC-9F3A`).
pub fn blank_stick(f: &Fake) {
    f.add_disk("sdc", 262144, true, "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-3/1-3:1.0/host7/target7:0:0/7:0:0:0/block/sdc", "Generic", "Flash Drive", 512);
    f.set_serial_up("/sys/devices/pci0000:00/0000:00:14.0/usb1/1-3", "SERIAL-SDC-9F3A");
    f.set_dev("sdc", vec![0u8; 262144 * 512]);
}
