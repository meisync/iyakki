//! Where IYAKKI keeps its data.
//!
//! The live ISO runs from RAM, so by default accounts, settings, vaults, documents and chats are
//! forgotten at power-off. A disk (or partition) formatted as ext4 with the label `IYAKKI-DATA` fixes
//! that: it is found and mounted at boot over the data directory. This crate finds such a volume,
//! lists the disks an administrator could use, and prepares one — with refusals for anything
//! mounted, for the medium IYAKKI booted from, and for disks that hold other data unless the caller
//! explicitly forces it. Nothing here shells out except `mke2fs` (no shell, fixed arguments).

use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

pub const LABEL: &str = "IYAKKI-DATA";

pub mod crashlog;
pub mod gpt;
pub mod loss;
pub mod virt;

/// Written at the start of a data partition created by the installer (`iyakki-usb`): "format me on first boot".
/// Our own marker, so IYAKKI never formats a partition it did not mark.
pub const PENDING_MARKER: &[u8; 20] = b"IYAKKI-FORMAT-ME-v1\0";

/// Where to look. `real()` for the running system; tests point these at temp directories.
#[derive(Clone, Debug)]
pub struct Roots {
    pub sys_block: PathBuf,
    pub dev: PathBuf,
    pub mounts: PathBuf,
    /// Production: nodes must be block devices. Tests use regular files as stand-ins.
    pub require_block: bool,
}

impl Roots {
    pub fn real() -> Roots {
        Roots { sys_block: "/sys/block".into(), dev: "/dev".into(), mounts: MOUNTS_PATH.into(), require_block: true }
    }
}

/// The kernel's list of mounts. Every crate reads mounts through [`read_mounts`] / [`mounts_text`] (one reader; gate `kernel_single_reader`).
pub const MOUNTS_PATH: &str = "/proc/mounts";
/// The kernel's list of swap areas ([`read_swaps`] / [`swaps_text`]).
pub const SWAPS_PATH: &str = "/proc/swaps";

/// The text of the kernel's mount list.
pub fn read_mounts() -> std::io::Result<String> {
    std::fs::read_to_string(MOUNTS_PATH)
}

/// The mount list, empty when it cannot be read.
pub fn mounts_text() -> String {
    read_mounts().unwrap_or_default()
}

/// The first word of `<proc>/uptime` (seconds since boot, as the kernel prints it); `None` when unreadable.
pub fn uptime_token_in(proc: &Path) -> Option<String> {
    std::fs::read_to_string(proc.join("uptime")).ok().and_then(|u| u.split_whitespace().next().map(str::to_string))
}

/// Seconds since boot with the fraction, from `<proc>/uptime`; `None` when unreadable or not a number.
pub fn uptime_in(proc: &Path) -> Option<f64> {
    uptime_token_in(proc)?.parse().ok()
}

/// [`uptime_in`] on the real `/proc`.
pub fn uptime_now() -> Option<f64> {
    uptime_in(Path::new("/proc"))
}

/// [`uptime_token_in`] on the real `/proc`.
pub fn uptime_token() -> Option<String> {
    uptime_token_in(Path::new("/proc"))
}

/// The kernel command line (`<proc>/cmdline`); empty when unreadable.
pub fn cmdline_in(proc: &Path) -> String {
    std::fs::read_to_string(proc.join("cmdline")).unwrap_or_default()
}

/// [`cmdline_in`] on the real `/proc`.
pub fn cmdline_text() -> String {
    cmdline_in(Path::new("/proc"))
}

/// The text of the kernel's swap list.
pub fn read_swaps() -> std::io::Result<String> {
    std::fs::read_to_string(SWAPS_PATH)
}

/// The swap list, empty when it cannot be read.
pub fn swaps_text() -> String {
    read_swaps().unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Empty,
    /// An IYAKKI-DATA volume.
    Iyakki,
    /// A data partition prepared by the installer that is waiting to be formatted (see [`format_pending`]).
    Pending,
    /// The medium IYAKKI booted from (ISO 9660, including hybrid USB sticks).
    Iso9660,
    /// Something else, described for the user ("ext4 “backup”", "partition table", "NTFS", …).
    Other(String),
}

impl Content {
    pub fn describe(&self) -> String {
        match self {
            Content::Empty => "empty".into(),
            Content::Iyakki => "IYAKKI data".into(),
            Content::Pending => "IYAKKI data partition (not yet formatted)".into(),
            Content::Iso9660 => "IYAKKI boot medium".into(),
            Content::Other(s) => s.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockDev {
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub removable: bool,
    pub model: String,
    /// `Some(disk)` for a partition.
    pub parent: Option<String>,
    pub mounted_at: Option<String>,
    pub content: Content,
}

impl BlockDev {
    pub fn human_size(&self) -> String {
        human(self.size_bytes)
    }
}

pub fn human(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let (mut v, mut i) = (b as f64, 0);
    while v >= 1000.0 && i < U.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

/// Look at the first bytes of a device and say what is on it.
pub fn probe(path: &Path) -> Result<Content, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    buf.truncate(n);
    Ok(classify(&buf))
}

pub fn classify(b: &[u8]) -> Content {
    let at = |off: usize, len: usize| b.get(off..off + len);
    if at(0, PENDING_MARKER.len()) == Some(&PENDING_MARKER[..]) {
        return Content::Pending;
    }
    if at(0x8001, 5) == Some(b"CD001") {
        return Content::Iso9660;
    }
    // ext2/3/4: magic 0xEF53 at 1024+56, volume label at 1024+120 (16 bytes)
    if at(1024 + 56, 2) == Some(&[0x53, 0xEF]) {
        let label: String = at(1024 + 120, 16).unwrap_or(&[]).iter().take_while(|c| **c != 0).map(|c| *c as char).collect();
        return if label == LABEL { Content::Iyakki } else { Content::Other(if label.is_empty() { "ext4 filesystem".into() } else { format!("ext4 filesystem “{label}”") }) };
    }
    if at(512, 8) == Some(b"EFI PART") {
        return Content::Other("GPT partition table".into());
    }
    if at(3, 8) == Some(b"NTFS    ") {
        return Content::Other("NTFS filesystem".into());
    }
    if at(82, 5) == Some(b"FAT32") || at(54, 3) == Some(b"FAT") {
        return Content::Other("FAT filesystem".into());
    }
    if at(4086, 10) == Some(b"SWAPSPACE2") {
        return Content::Other("swap space".into());
    }
    if at(510, 2) == Some(&[0x55, 0xAA]) {
        return Content::Other("partition table".into());
    }
    if b.iter().all(|x| *x == 0) {
        return Content::Empty;
    }
    Content::Other("data of unknown type".into())
}

/// (device path as written in the mount table → mount point)
fn mount_table(roots: &Roots) -> Vec<(String, String)> {
    std::fs::read_to_string(&roots.mounts)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.to_string(), it.next()?.replace("\\040", " ")))
        })
        .collect()
}

fn read_trim(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default().trim().to_string()
}

/// Disks and their partitions, most useful first (partitions after their disk). Optical drives,
/// RAM disks, loop and device-mapper nodes are not offered.
pub fn list(roots: &Roots) -> Vec<BlockDev> {
    let mounts = mount_table(roots);
    let mut out = Vec::new();
    let mut disks: Vec<String> = std::fs::read_dir(&roots.sys_block).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    disks.sort();
    let skip = |n: &str| ["loop", "ram", "zram", "dm-", "md", "sr", "fd", "nbd"].iter().any(|p| n.starts_with(p));
    let make = |name: &str, dir: &Path, parent: Option<&str>, removable: bool, model: &str| -> Option<BlockDev> {
        let sectors: u64 = read_trim(&dir.join("size")).parse().ok()?;
        if sectors == 0 {
            return None;
        }
        let path = roots.dev.join(name);
        if !path.exists() {
            return None; // no node yet (mdev has not created it)
        }
        let mounted_at = mounts.iter().find(|(src, _)| Path::new(src) == path).map(|(_, m)| m.clone());
        let content = probe(&path).unwrap_or_else(|e| Content::Other(format!("unreadable ({e})")));
        Some(BlockDev { name: name.into(), path, size_bytes: sectors * 512, removable, model: model.into(), parent: parent.map(String::from), mounted_at, content })
    };
    for d in disks.iter().filter(|d| !skip(d)) {
        let dir = roots.sys_block.join(d);
        let removable = read_trim(&dir.join("removable")) == "1";
        let model = [read_trim(&dir.join("device/vendor")), read_trim(&dir.join("device/model"))].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" ");
        if let Some(b) = make(d, &dir, None, removable, &model) {
            out.push(b);
        }
        let mut parts: Vec<String> = std::fs::read_dir(&dir).into_iter().flatten().flatten().filter(|e| e.path().join("partition").exists()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        parts.sort();
        for p in parts {
            if let Some(b) = make(&p, &dir.join(&p), Some(d), removable, &model) {
                out.push(b);
            }
        }
    }
    out
}

/// The disk the system was started from: the one that holds the boot volume the first stage left mounted at `/.stage1/boot`.
/// `None` when the system did not start that way (a live ISO, a test).
fn boot_disk(roots: &Roots, devs: &[BlockDev]) -> Option<String> {
    let src = mount_table(roots).into_iter().find(|(_, m)| m == "/.stage1/boot")?.0;
    let name = Path::new(&src).file_name()?.to_string_lossy().into_owned();
    let d = devs.iter().find(|d| d.name == name)?;
    Some(d.parent.clone().unwrap_or(name))
}

/// The IYAKKI-DATA volume, if one is present: [`find_data_for`] with no explicit choice.
pub fn find_data(roots: &Roots) -> Option<BlockDev> {
    find_data_for(roots, None)
}

/// The data volume this system uses. It takes **its own**, not whatever it finds:
/// * `choice` (`iyakki.data=/dev/sdb2` on the kernel command line): exactly that partition, when it is a data volume (or one waiting to
///   be formatted); nothing else is looked at. This is how a person loads the data of another partition or disk.
/// * the system started from a disk (the first stage left its boot volume at `/.stage1/boot`): the data volume of that same disk. A
///   partition of it that is still waiting to be formatted counts as it, so another disk's volume is never taken in its place, and a
///   boot disk without one stays in RAM until the person chooses.
/// * no boot disk known (a live ISO, a computer's own disks): the first free data volume, as before.
pub fn find_data_for(roots: &Roots, choice: Option<&str>) -> Option<BlockDev> {
    let devs = list(roots);
    let free = |d: &BlockDev| d.mounted_at.is_none();
    if let Some(c) = choice.filter(|c| !c.is_empty()) {
        let name = Path::new(c).file_name()?.to_string_lossy().into_owned();
        return devs.into_iter().find(|d| d.name == name && d.parent.is_some() && d.content == Content::Iyakki && free(d));
    }
    if let Some(bd) = boot_disk(roots, &devs) {
        return devs.into_iter().find(|d| d.parent.as_deref() == Some(bd.as_str()) && free(d) && d.content == Content::Iyakki);
    }
    devs.into_iter().find(|d| d.content == Content::Iyakki && free(d))
}

/// Can `dev` be turned into the data volume? `Ok(None)` = yes, it is empty;
/// `Ok(Some(what))` = yes, but only if the caller forces it, because `what` would be erased;
/// `Err` = never (in use, or the boot medium).
pub fn eligibility(dev: &BlockDev, all: &[BlockDev]) -> Result<Option<String>, String> {
    if let Some(m) = &dev.mounted_at {
        return Err(format!("{} is in use (mounted at {m})", dev.name));
    }
    if let Some(c) = all.iter().find(|c| c.parent.as_deref() == Some(&dev.name) && c.mounted_at.is_some()) {
        return Err(format!("{} is in use ({} is mounted at {})", dev.name, c.name, c.mounted_at.clone().unwrap_or_default()));
    }
    if dev.content == Content::Iso9660 {
        return Err(format!("{} is the medium IYAKKI booted from", dev.name));
    }
    if let Some(p) = &dev.parent {
        if all.iter().any(|d| &d.name == p && d.content == Content::Iso9660) {
            return Err(format!("{} is part of the medium IYAKKI booted from", dev.name));
        }
    }
    if dev.parent.is_none() && all.iter().any(|c| c.parent.as_deref() == Some(&dev.name) && c.content != Content::Empty) && dev.content == Content::Empty {
        // a disk whose first sectors are blank but which has live partitions: do not wipe it silently
        return Ok(Some("its partitions".into()));
    }
    Ok(match &dev.content {
        Content::Empty | Content::Pending => None,
        other => Some(other.describe()),
    })
}

/// A partition marked by the installer and not yet formatted (never one that is mounted). When the system started from a disk, only
/// that disk's own marked partition counts.
pub fn find_pending(roots: &Roots) -> Option<BlockDev> {
    let devs = list(roots);
    let bd = boot_disk(roots, &devs);
    devs.into_iter().find(|d| d.content == Content::Pending && d.mounted_at.is_none() && d.parent.is_some() && bd.as_ref().is_none_or(|b| d.parent.as_ref() == Some(b)))
}

/// A partition the GUID partition table of a disk says is the IYAKKI data volume (label `IYAKKI-DATA`, type Linux filesystem),
/// found from the table itself: it exists whether or not the kernel has made a device for it yet. The partition's name
/// follows the kernel's rule (`sda3`; `nvme0n1p3` and `mmcblk0p3` for disks whose names end in a digit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GptHint {
    pub disk: String,
    pub part: String,
    pub number: u32,
}

/// Name of partition `n` of `disk` as the kernel names it.
pub fn partition_name(disk: &str, n: u32) -> String {
    if disk.chars().last().is_some_and(|c| c.is_ascii_digit()) { format!("{disk}p{n}") } else { format!("{disk}{n}") }
}

/// The data partitions the disks' GPTs announce (4096-byte sectors are read as such). Used to decide whether boot should wait for
/// a partition node that is still to appear, and to explain a data volume that cannot be found.
pub fn gpt_hints(roots: &Roots) -> Vec<GptHint> {
    let mut out = Vec::new();
    for d in list(roots).into_iter().filter(|d| d.parent.is_none() && d.content != Content::Iso9660) {
        let ss: u32 = read_trim(&roots.sys_block.join(&d.name).join("queue/logical_block_size")).parse().ok().filter(|n| matches!(n, 512 | 4096)).unwrap_or(512);
        let Ok(mut f) = std::fs::File::open(&d.path) else { continue };
        let Ok(p) = gpt::read_gpt(&mut f, ss, d.size_bytes / ss as u64) else { continue };
        for e in p.table.entries.iter().filter(|e| e.name == LABEL && e.type_guid == gpt::TYPE_LINUX_DATA) {
            out.push(GptHint { disk: d.name.clone(), part: partition_name(&d.name, e.index), number: e.index });
        }
    }
    out
}

/// Where the installer leaves the administrator's first-boot choices (zram, swap file, ...) inside the marked, not yet
/// formatted data partition: after the marker, still within the 64 KiB `probe` reads. Formatting destroys it, so the
/// first boot reads it before [`format_pending`] and keeps it in `system.conf`.
pub const PENDING_CONF_OFFSET: u64 = 4096;
pub const PENDING_CONF_MAGIC: &str = "IYAKKI-INSTALL-CONF-v1\n";
pub const PENDING_CONF_MAX: usize = 8192;

/// The bytes to write at [`PENDING_CONF_OFFSET`] for `conf` (`key=value` lines).
pub fn pending_conf_bytes(conf: &str) -> Result<Vec<u8>, String> {
    let mut v = format!("{PENDING_CONF_MAGIC}{conf}").into_bytes();
    if v.len() >= PENDING_CONF_MAX {
        return Err("the install settings are too long".into());
    }
    v.push(0);
    Ok(v)
}

/// The settings the installer left in a pending data partition (or file, in tests).
pub fn read_pending_conf(path: &Path) -> Option<String> {
    use std::io::Seek;
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(std::io::SeekFrom::Start(PENDING_CONF_OFFSET)).ok()?;
    let mut buf = vec![0u8; PENDING_CONF_MAX];
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => n += k,
        }
    }
    let text = std::str::from_utf8(&buf[..n]).ok()?;
    let body = text.strip_prefix(PENDING_CONF_MAGIC)?;
    Some(body.split('\0').next().unwrap_or("").to_string())
}

/// First boot after an install: format the marked partition as the IYAKKI-DATA volume. Returns its name.
pub fn format_pending(roots: &Roots, mke2fs: &Path) -> Result<Option<String>, String> {
    let Some(dev) = find_pending(roots) else { return Ok(None) };
    make_filesystem(mke2fs, &dev.path)?;
    if probe(&dev.path)? != Content::Iyakki {
        return Err("the new data filesystem did not verify".into());
    }
    Ok(Some(dev.name))
}

/// The first sector of a raw install image with its marked data partition (MBR partition 2 at `part_start`, the last one) stretched to the
/// end of a disk of `disk_sectors`: `(sector, new size in sectors)`. `None` unless it is an MBR whose entry for that start is type 0x83,
/// no other partition lies after it and the gain is over 1 MiB. 1 MiB aligned, within the 32-bit sector count of an MBR.
pub fn grown_mbr(sector0: &[u8], part_start: u64, disk_sectors: u64) -> Option<(Vec<u8>, u64)> {
    if sector0.len() < 512 || sector0[510..512] != [0x55, 0xAA] {
        return None;
    }
    let entry = |i: usize| {
        let e = 446 + 16 * i;
        let u = |o: usize| u32::from_le_bytes([sector0[e + o], sector0[e + o + 1], sector0[e + o + 2], sector0[e + o + 3]]) as u64;
        (sector0[e + 4], u(8), u(12))
    };
    let (i, (kind, start, size)) = (0..4).map(|i| (i, entry(i))).find(|(_, (k, st, _))| *k != 0 && *st == part_start)?;
    if kind != 0x83 || (0..4).any(|j| j != i && entry(j).0 != 0 && entry(j).1 > start) {
        return None;
    }
    let sectors = ((disk_sectors.checked_sub(start)?).min(0xFFFF_FFFF - start) / 2048) * 2048;
    if sectors <= size + 2048 {
        return None;
    }
    let mut out = sector0[..512].to_vec();
    let e = 446 + 16 * i;
    out[e + 12..e + 16].copy_from_slice(&(sectors as u32).to_le_bytes());
    out[e + 5..e + 8].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    Some((out, sectors))
}

/// First start after a raw image was written to a stick or disk: the marked data partition is the 1 MiB the image carries, so stretch it
/// to the end of the drive before it is formatted (the installer does this when it writes; a plain copy of the image does not). Only a
/// partition still waiting to be formatted, only the last MBR partition, only its size field; the kernel is told with BLKPG so the
/// mounted boot partition beside it is not disturbed. `Ok(Some(sectors))` when it grew.
pub fn grow_pending(roots: &Roots) -> Result<Option<u64>, String> {
    let Some(dev) = find_pending(roots) else { return Ok(None) };
    let Some(disk) = dev.parent.clone() else { return Ok(None) };
    let sys = roots.sys_block.join(&disk);
    let num = |p: &Path| read_trim(p).parse::<u64>().ok();
    let (Some(start), Some(disk_sectors), Some(index)) = (num(&sys.join(&dev.name).join("start")), num(&sys.join("size")), num(&sys.join(&dev.name).join("partition"))) else { return Ok(None) };
    let disk_path = roots.dev.join(&disk);
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(&disk_path).map_err(|e| format!("{}: {e}", disk_path.display()))?;
    let mut s0 = [0u8; 512];
    f.read_exact(&mut s0).map_err(|e| e.to_string())?;
    let Some((new0, sectors)) = grown_mbr(&s0, start, disk_sectors) else { return Ok(None) };
    f.seek(std::io::SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    f.write_all(&new0).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    if roots.require_block {
        use std::os::unix::io::AsRawFd;
        #[repr(C)]
        struct Part { start: i64, length: i64, pno: i32, devname: [u8; 64], volname: [u8; 64] }
        #[repr(C)]
        struct Arg { op: i32, flags: i32, datalen: i32, data: *mut Part }
        let mut p = Part { start: (start * 512) as i64, length: (sectors * 512) as i64, pno: index as i32, devname: [0; 64], volname: [0; 64] };
        let mut a = Arg { op: 3, flags: 0, datalen: std::mem::size_of::<Part>() as i32, data: &mut p };
        // SAFETY: `a` and `p` live across the call and match the kernel's blkpg_ioctl_arg / blkpg_partition layouts; 0x1269 is BLKPG, op 3 BLKPG_RESIZE_PARTITION.
        if unsafe { libc::ioctl(f.as_raw_fd(), 0x1269 as _, &mut a) } != 0 {
            return Err(format!("the partition table was updated but the kernel did not resize {}: {}", dev.name, std::io::Error::last_os_error()));
        }
    }
    Ok(Some(sectors))
}

/// What the data directory currently lives on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DataLocation {
    /// A disk volume: data survives power-off.
    Disk(String),
    /// RAM (live session): data is lost at power-off.
    Memory,
    /// Something else we cannot classify (container volume, network share, …).
    Other(String),
}

/// Classify `data_dir` from the mount table (longest matching mount point wins).
pub fn data_location(roots: &Roots, data_dir: &Path) -> DataLocation {
    let table = mount_table(roots);
    let content = std::fs::read_to_string(&roots.mounts).unwrap_or_default();
    let best = table.iter().filter(|(_, m)| data_dir == Path::new(m) || data_dir.starts_with(m)).max_by_key(|(_, m)| m.len());
    let Some((src, m)) = best else { return DataLocation::Other("unknown".into()) };
    let fstype = content.lines().find(|l| l.split_whitespace().nth(1).map(|x| x.replace("\\040", " ")) == Some(m.clone())).and_then(|l| l.split_whitespace().nth(2)).unwrap_or("");
    match fstype {
        "tmpfs" | "ramfs" | "rootfs" => DataLocation::Memory,
        _ if src.starts_with("/dev/") => DataLocation::Disk(src.clone()),
        other => DataLocation::Other(other.to_string()),
    }
}

/// Free and total bytes of the filesystem holding `path`.
pub fn space(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `c` is a valid NUL-terminated path and `st` is a properly sized, writable statvfs.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    if rc != 0 {
        return None;
    }
    let bs = st.f_frsize as u64;
    Some((st.f_bavail as u64 * bs, st.f_blocks as u64 * bs))
}

/// Mount/unmount seam, so the destructive flow can be tested without root.
pub trait Mounter: Send + Sync {
    fn mount(&self, dev: &Path, target: &Path) -> Result<(), String>;
    fn unmount(&self, target: &Path) -> Result<(), String>;
}

pub struct SysMounter;

#[cfg(target_os = "linux")]
impl Mounter for SysMounter {
    fn mount(&self, dev: &Path, target: &Path) -> Result<(), String> {
        use std::os::unix::ffi::OsStrExt;
        let _ = std::fs::create_dir_all(target);
        let (s, t) = (std::ffi::CString::new(dev.as_os_str().as_bytes()).map_err(|e| e.to_string())?, std::ffi::CString::new(target.as_os_str().as_bytes()).map_err(|e| e.to_string())?);
        let (f, d) = (std::ffi::CString::new("ext4").unwrap(), std::ffi::CString::new("").unwrap());
        // SAFETY: all pointers are valid NUL-terminated strings for the duration of the call.
        let rc = unsafe { libc::mount(s.as_ptr(), t.as_ptr(), f.as_ptr(), libc::MS_NOSUID | libc::MS_NODEV, d.as_ptr().cast()) };
        if rc == 0 { Ok(()) } else { Err(format!("mount {}: {}", dev.display(), std::io::Error::last_os_error())) }
    }

    fn unmount(&self, target: &Path) -> Result<(), String> {
        use std::os::unix::ffi::OsStrExt;
        let t = std::ffi::CString::new(target.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        // SAFETY: valid NUL-terminated path.
        let rc = unsafe { libc::umount(t.as_ptr()) };
        if rc == 0 { Ok(()) } else { Err(format!("umount {}: {}", target.display(), std::io::Error::last_os_error())) }
    }
}

/// Mounting a data disk is something only the Linux system does; elsewhere (the desktop window on a Mac) the disks it works
/// with are virtual and never mounted.
#[cfg(not(target_os = "linux"))]
impl Mounter for SysMounter {
    fn mount(&self, _dev: &Path, _target: &Path) -> Result<(), String> {
        Err("mounting disks is only done by the Linux system".into())
    }

    fn unmount(&self, _target: &Path) -> Result<(), String> {
        Err("mounting disks is only done by the Linux system".into())
    }
}

/// Boot step: mount the IYAKKI-DATA volume over `data_dir`. `Ok(None)` = none present (stay in RAM).
pub fn mount_data(roots: &Roots, mounter: &dyn Mounter, data_dir: &Path) -> Result<Option<String>, String> {
    mount_data_for(roots, mounter, data_dir, None)
}

/// [`mount_data`] with the partition the person chose (`iyakki.data=/dev/...`), see [`find_data_for`].
pub fn mount_data_for(roots: &Roots, mounter: &dyn Mounter, data_dir: &Path, choice: Option<&str>) -> Result<Option<String>, String> {
    let Some(dev) = find_data_for(roots, choice) else { return Ok(None) };
    let _ = std::fs::create_dir_all(data_dir);
    mounter.mount(&dev.path, data_dir)?;
    set_private(data_dir);
    Ok(Some(dev.name))
}

/// The ext4 UUID in the superblock of `bytes` (the data from offset 1024 of the partition), as `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`.
fn ext_uuid(sb: &[u8]) -> Option<String> {
    if sb.len() < 120 || sb[56..58] != [0x53, 0xEF] || sb[104..120].iter().all(|b| *b == 0) {
        return None;
    }
    let h: String = sb[104..120].iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]))
}

/// Make `<dev>/disk/by-uuid/<uuid>` -> `../../<name>` for the data partition. The system has no udev, and the kubelet (cAdvisor) finds the
/// filesystem of its image folder through that directory: without it the log repeats "cannot find filesystem info for device /dev/sda2"
/// every 10 seconds and the kubelet cannot read its disk use (so it never evicts on disk pressure). Returns the UUID when the link was made.
pub fn link_disk_uuid(roots: &Roots, name: &str) -> Result<Option<String>, String> {
    let path = roots.dev.join(name);
    let mut f = std::fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut sb = [0u8; 1024];
    f.seek(std::io::SeekFrom::Start(1024)).map_err(|e| e.to_string())?;
    f.read_exact(&mut sb).map_err(|e| e.to_string())?;
    let Some(uuid) = ext_uuid(&sb) else { return Ok(None) };
    let dir = roots.dev.join("disk/by-uuid");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let link = dir.join(&uuid);
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(format!("../../{name}"), &link).map_err(|e| format!("{}: {e}", link.display()))?;
    Ok(Some(uuid))
}

fn set_private(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistReport {
    pub device: String,
    pub files_copied: usize,
    pub bytes_copied: u64,
}

/// Recursively copy `from` into `to` (regular files and directories only; symlinks are skipped).
pub fn copy_tree(from: &Path, to: &Path) -> Result<(usize, u64), String> {
    use std::os::unix::fs::PermissionsExt;
    fn walk(from: &Path, to: &Path, depth: u32, n: &mut usize, bytes: &mut u64) -> Result<(), String> {
        if depth > 32 {
            return Err("directory tree too deep".into());
        }
        std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
        if let Ok(m) = std::fs::metadata(from) {
            let _ = std::fs::set_permissions(to, std::fs::Permissions::from_mode(m.permissions().mode() & 0o7777));
        }
        for e in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?.flatten() {
            let ft = e.file_type().map_err(|e| e.to_string())?;
            let dst = to.join(e.file_name());
            if ft.is_dir() {
                walk(&e.path(), &dst, depth + 1, n, bytes)?;
            } else if ft.is_file() {
                *bytes += std::fs::copy(e.path(), &dst).map_err(|e2| format!("{}: {e2}", e.path().display()))?;
                *n += 1;
            }
        }
        Ok(())
    }
    let (mut n, mut bytes) = (0, 0);
    walk(from, to, 0, &mut n, &mut bytes)?;
    Ok((n, bytes))
}

/// Devices at least this large get a bigger bytes-per-inode ratio, so the inode tables (written lazily by the
/// kernel in the background, on a USB disk that is a lot of write traffic) stay small: 931 GB at the default
/// 16 KiB ratio is ~15 GB of inode tables, at 64 KiB it is ~3.8 GB.
const BIG_DEVICE: u64 = 256 << 30;

/// The fixed `mke2fs` arguments (no shell). `nodiscard`: do not TRIM the whole device first, which on a 931 GB USB
/// SSD took ~7 s of boot (nothing needs the old contents trimmed; the volume is new). Lazy table and journal
/// initialisation keep the filesystem fully valid (checksummed groups are flagged uninitialised) and the journal stays.
pub fn mke2fs_args(dev: &Path, size_bytes: u64) -> Vec<std::ffi::OsString> {
    mke2fs_args_with(dev, size_bytes, &fresh_uuid())
}

/// A random (version 4) UUID from `/dev/urandom`, which never waits. Handing `mke2fs` its UUID and directory hash seed
/// keeps it from asking the kernel for random numbers itself: right after power-on that call blocks until the random
/// pool is ready, which delayed the format by several seconds on a machine with little early entropy. Neither value is a secret.
fn fresh_uuid() -> String {
    use std::io::Read;
    let mut b = [0u8; 16];
    let ok = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_ok();
    if !ok {
        // no urandom: mix the clock, still unique enough for a volume id
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        b = (t ^ (std::process::id() as u128) << 64).to_le_bytes();
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: Vec<String> = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", h[0..4].concat(), h[4..6].concat(), h[6..8].concat(), h[8..10].concat(), h[10..16].concat())
}

fn mke2fs_args_with(dev: &Path, size_bytes: u64, uuid: &str) -> Vec<std::ffi::OsString> {
    let mut a: Vec<std::ffi::OsString> = ["-t", "ext4", "-L", LABEL, "-U", uuid, "-F", "-q", "-m", "1"].iter().map(Into::into).collect();
    if size_bytes >= BIG_DEVICE {
        a.extend(["-i", "65536"].iter().map(std::ffi::OsString::from));
    }
    a.push("-E".into());
    a.push(format!("nodiscard,lazy_itable_init=1,lazy_journal_init=1,hash_seed={uuid}").into());
    a.push(dev.into());
    a
}

fn device_size(dev: &Path) -> u64 {
    use std::io::{Seek, SeekFrom};
    std::fs::File::open(dev).and_then(|mut f| f.seek(SeekFrom::End(0))).unwrap_or(0)
}

/// Run `mke2fs` without a shell.
fn make_filesystem(mke2fs: &Path, dev: &Path) -> Result<(), String> {
    let out = std::process::Command::new(mke2fs)
        .args(mke2fs_args(dev, device_size(dev)))
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("could not run {}: {e}", mke2fs.display()))?;
    if out.status.success() { Ok(()) } else { Err(format!("mke2fs failed: {}", String::from_utf8_lossy(&out.stderr).trim())) }
}

/// Turn `device` (e.g. "vdb" or "/dev/vdb1") into the IYAKKI-DATA volume and copy the current data
/// (`seed`) onto it so nothing already created is lost. `force` must be true when the device holds
/// other data. The new volume is *not* mounted over the running data directory; the caller tells
/// the user to reboot.
pub fn persist(roots: &Roots, mounter: &dyn Mounter, mke2fs: &Path, scratch: &Path, device: &str, force: bool, seed: Option<&Path>) -> Result<PersistReport, String> {
    let name = device.trim().trim_start_matches("/dev/");
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return Err("give a device name such as vdb or sda1".into());
    }
    let all = list(roots);
    let dev = all.iter().find(|d| d.name == name).ok_or_else(|| format!("no such disk: {name}"))?;
    if roots.require_block {
        use std::os::unix::fs::FileTypeExt;
        if !std::fs::metadata(&dev.path).map(|m| m.file_type().is_block_device()).unwrap_or(false) {
            return Err(format!("{} is not a block device", dev.path.display()));
        }
    }
    match eligibility(dev, &all)? {
        Some(what) if !force => return Err(format!("{name} contains {what}; refusing to erase it without confirmation")),
        _ => {}
    }
    make_filesystem(mke2fs, &dev.path)?;
    if probe(&dev.path)? != Content::Iyakki {
        return Err("the new filesystem did not verify".into());
    }
    let (mut files, mut bytes) = (0, 0);
    if let Some(seed) = seed.filter(|s| s.is_dir()) {
        let target = scratch.join("iyakki-newdata");
        let _ = std::fs::remove_dir_all(&target);
        mounter.mount(&dev.path, &target)?;
        let r = copy_tree(seed, &target);
        set_private(&target);
        // flush and detach even when the copy failed
        let _ = unsafe_sync();
        let u = mounter.unmount(&target);
        let _ = std::fs::remove_dir(&target);
        let (f, b) = r?;
        u?;
        (files, bytes) = (f, b);
    }
    Ok(PersistReport { device: name.into(), files_copied: files, bytes_copied: bytes })
}

fn unsafe_sync() {
    // SAFETY: sync(2) takes no arguments.
    unsafe { libc::sync() };
}

#[cfg(test)]
mod tests {
    #[test]
    fn uptime_is_read_from_the_given_proc_folder() {
        let d = std::env::temp_dir().join(format!("iyakki-storage-uptime-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(uptime_in(&d), None, "no file");
        std::fs::write(d.join("uptime"), "93784.55 180000.00\n").unwrap();
        assert_eq!(uptime_in(&d), Some(93784.55));
        assert_eq!(uptime_token_in(&d).as_deref(), Some("93784.55"));
        std::fs::write(d.join("uptime"), "junk\n").unwrap();
        assert_eq!(uptime_in(&d), None);
        assert_eq!(uptime_token_in(&d).as_deref(), Some("junk"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cmdline_is_read_from_the_given_proc_folder_and_empty_when_missing() {
        let d = std::env::temp_dir().join(format!("iyakki-storage-cmdline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(cmdline_in(&d), "");
        std::fs::write(d.join("cmdline"), "quiet iyakki.x=1\n").unwrap();
        assert_eq!(cmdline_in(&d), "quiet iyakki.x=1\n");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn kernel_lists_are_read_from_their_fixed_paths_and_empty_when_missing() {
        assert_eq!((MOUNTS_PATH, SWAPS_PATH), ("/proc/mounts", "/proc/swaps"));
        assert_eq!(Roots::real().mounts, std::path::PathBuf::from(MOUNTS_PATH));
        // the real files exist on Linux; both calls must agree with the plain read
        assert_eq!(mounts_text(), std::fs::read_to_string(MOUNTS_PATH).unwrap_or_default());
        assert_eq!(read_swaps().is_ok(), std::fs::read_to_string(SWAPS_PATH).is_ok());
    }

    use super::*;

    #[test]
    fn the_data_partition_gets_a_by_uuid_link_for_the_kubelet() {
        let d = std::env::temp_dir().join(format!("iyakki-uuidlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let mut img = vec![0u8; 4096];
        img[1024 + 56..1024 + 58].copy_from_slice(&[0x53, 0xEF]);
        img[1024 + 104..1024 + 120].copy_from_slice(&[0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
        std::fs::write(d.join("sda2"), &img).unwrap();
        std::fs::write(d.join("sda3"), vec![0u8; 4096]).unwrap();
        let roots = Roots { sys_block: d.join("sys"), dev: d.clone(), mounts: d.join("mounts"), require_block: false };
        let uuid = link_disk_uuid(&roots, "sda2").unwrap().expect("an ext4 superblock has a uuid");
        assert_eq!(uuid, "12345678-9abc-def0-1122-334455667788");
        assert_eq!(std::fs::read_link(d.join("disk/by-uuid").join(&uuid)).unwrap(), Path::new("../../sda2"));
        assert_eq!(link_disk_uuid(&roots, "sda3").unwrap(), None, "not ext4: no link");
        assert!(link_disk_uuid(&roots, "sda2").unwrap().is_some(), "a second start replaces the link");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_plain_copy_of_the_image_is_stretched_to_fill_the_drive_but_only_the_last_data_partition() {
        let mut s0 = [0u8; 512];
        s0[510..512].copy_from_slice(&[0x55, 0xAA]);
        let set = |s: &mut [u8; 512], i: usize, kind: u8, start: u32, size: u32| {
            let e = 446 + 16 * i;
            s[e + 4] = kind;
            s[e + 8..e + 12].copy_from_slice(&start.to_le_bytes());
            s[e + 12..e + 16].copy_from_slice(&size.to_le_bytes());
        };
        set(&mut s0, 0, 0xEF, 2048, 524288);
        set(&mut s0, 1, 0x83, 526336, 2048);
        let disk = 125_000_000u64; // about 64 GB
        let (n, sectors) = grown_mbr(&s0, 526336, disk).expect("grows");
        assert_eq!(sectors, (disk - 526336) / 2048 * 2048);
        assert_eq!(u32::from_le_bytes(n[446 + 16 + 12..446 + 16 + 16].try_into().unwrap()) as u64, sectors);
        assert_eq!(&n[446..446 + 16], &s0[446..446 + 16], "the boot partition is untouched");
        assert!(grown_mbr(&s0, 2048, disk).is_none(), "not the data type");
        assert!(grown_mbr(&s0, 526336, 526336 + 4096).is_none(), "under 1 MiB to gain");
        assert!(grown_mbr(&s0, 999, disk).is_none(), "no such partition");
        let mut later = s0;
        set(&mut later, 2, 0x83, 900000, 2048);
        assert!(grown_mbr(&later, 526336, disk).is_none(), "another partition lies after it");
        let mut bad = s0;
        bad[511] = 0;
        assert!(grown_mbr(&bad, 526336, disk).is_none(), "no MBR signature");
    }

    use std::sync::Mutex;

    /// A fake machine: /sys/block + /dev as temp dirs, disks as sparse files.
    struct Machine {
        root: PathBuf,
        roots: Roots,
    }

    fn machine(tag: &str) -> Machine {
        let root = std::env::temp_dir().join(format!("iyakki-storage-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["sys/block", "dev"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("mounts"), "").unwrap();
        let roots = Roots { sys_block: root.join("sys/block"), dev: root.join("dev"), mounts: root.join("mounts"), require_block: false };
        Machine { root, roots }
    }

    impl Machine {
        /// Add a disk (or a partition when `parent` is given) backed by a file of `mib` MiB with `content` at its start.
        fn disk(&self, name: &str, parent: Option<&str>, mib: u64, content: &[(usize, &[u8])]) {
            let dir = match parent {
                Some(p) => self.root.join("sys/block").join(p).join(name),
                None => self.root.join("sys/block").join(name),
            };
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("size"), format!("{}\n", mib * 2048)).unwrap();
            if parent.is_some() {
                std::fs::write(dir.join("partition"), "1\n").unwrap();
            } else {
                std::fs::write(dir.join("removable"), "0\n").unwrap();
                std::fs::create_dir_all(dir.join("device")).unwrap();
                std::fs::write(dir.join("device/model"), "Test Disk  \n").unwrap();
            }
            let node = self.root.join("dev").join(name);
            let f = std::fs::File::create(&node).unwrap();
            f.set_len(mib * 1024 * 1024).unwrap();
            drop(f);
            use std::os::unix::fs::FileExt;
            let f = std::fs::OpenOptions::new().write(true).open(&node).unwrap();
            for (off, bytes) in content {
                f.write_all_at(bytes, *off as u64).unwrap();
            }
        }
        fn mounted(&self, dev: &str, at: &str) {
            let mut m = std::fs::read_to_string(self.root.join("mounts")).unwrap();
            m.push_str(&format!("{}/dev/{dev} {at} ext4 rw 0 0\n", self.root.display()));
            std::fs::write(self.root.join("mounts"), m).unwrap();
        }
    }

    fn ext4(label: &str) -> Vec<(usize, Vec<u8>)> {
        let mut l = label.as_bytes().to_vec();
        l.resize(16, 0);
        vec![(1024 + 56, vec![0x53, 0xEF]), (1024 + 120, l)]
    }
    fn with<'a>(v: &'a [(usize, Vec<u8>)]) -> Vec<(usize, &'a [u8])> {
        v.iter().map(|(o, b)| (*o, b.as_slice())).collect()
    }

    #[test]
    fn the_gpt_names_the_data_partition_even_before_the_kernel_has_a_node_for_it() {
        let m = machine("gpthint");
        // a GPT disk "nvme0n1" (p suffix) with 4096-byte sectors and one "sda" with 512: both announce IYAKKI-DATA as partition 3
        for (name, ss) in [("nvme0n1", 4096u32), ("sda", 512)] {
            let mib = 64u64;
            let total = mib * 1024 * 1024 / ss as u64;
            let al = 1024 * 1024 / ss as u64;
            let mut t = gpt::Table { sector_size: ss, total_sectors: total, disk_guid: [7; 16], entries: vec![] };
            for (i, (ty, name)) in [(gpt::TYPE_BIOS_BOOT, "IYAKKI-BIOS"), (gpt::TYPE_ESP, "IYAKKI-BOOT"), (gpt::TYPE_LINUX_DATA, "IYAKKI-DATA")].into_iter().enumerate() {
                t.entries.push(gpt::Entry { index: i as u32 + 1, type_guid: ty, guid: [i as u8 + 1; 16], first_lba: (i as u64 + 1) * al, last_lba: (i as u64 + 2) * al - 1, attrs: 0, name: name.into() });
            }
            let mut img = vec![0u8; (mib * 1024 * 1024) as usize];
            let s = ss as usize;
            img[s..2 * s].copy_from_slice(&t.header_bytes(false));
            img[2 * s..2 * s + 16384].copy_from_slice(&t.entries_bytes());
            let b = t.backup_entries_lba() as usize * s;
            img[b..b + 16384].copy_from_slice(&t.entries_bytes());
            img[(total as usize - 1) * s..].copy_from_slice(&t.header_bytes(true));
            m.disk(name, None, mib, &[]);
            std::fs::write(m.root.join("dev").join(name), &img).unwrap();
            let q = m.root.join("sys/block").join(name).join("queue");
            std::fs::create_dir_all(&q).unwrap();
            std::fs::write(q.join("logical_block_size"), format!("{ss}\n")).unwrap();
        }
        m.disk("vdz", None, 8, &[]); // a disk without a table: nothing to say
        let h = gpt_hints(&m.roots);
        assert_eq!(h, vec![GptHint { disk: "nvme0n1".into(), part: "nvme0n1p3".into(), number: 3 }, GptHint { disk: "sda".into(), part: "sda3".into(), number: 3 }]);
        assert_eq!(partition_name("mmcblk0", 2), "mmcblk0p2");
        assert_eq!(partition_name("vda", 2), "vda2");
        // the label alone is not enough: a partition of another type called IYAKKI-DATA is ignored
        assert!(gpt_hints(&machine("gpthint-none").roots).is_empty());
    }

    #[test]
    fn probe_recognises_what_is_on_a_disk() {
        assert_eq!(classify(&vec![0u8; 70_000]), Content::Empty);
        let mut iso = vec![0u8; 70_000];
        iso[0x8001..0x8006].copy_from_slice(b"CD001");
        iso[510] = 0x55;
        iso[511] = 0xAA; // hybrid USB sticks also carry an MBR: ISO wins
        assert_eq!(classify(&iso), Content::Iso9660);
        let mk = |label: &str| {
            let mut v = vec![0u8; 70_000];
            for (o, b) in ext4(label) {
                v[o..o + b.len()].copy_from_slice(&b);
            }
            classify(&v)
        };
        assert_eq!(mk("IYAKKI-DATA"), Content::Iyakki);
        assert_eq!(mk("backup"), Content::Other("ext4 filesystem “backup”".into()));
        assert_eq!(mk(""), Content::Other("ext4 filesystem".into()));
        let mut gpt = vec![0u8; 70_000];
        gpt[512..520].copy_from_slice(b"EFI PART");
        assert_eq!(classify(&gpt), Content::Other("GPT partition table".into()));
        let mut junk = vec![0u8; 70_000];
        junk[100] = 7;
        assert_eq!(classify(&junk), Content::Other("data of unknown type".into()));
        assert_eq!(classify(&[]), Content::Empty);
    }

    #[test]
    fn the_installers_marker_is_recognised_and_only_that_is_formatted() {
        let Some(mke2fs) = mke2fs() else { return eprintln!("mke2fs not installed; skipping") };
        let m = machine("pending");
        m.disk("vda", None, 32, &[]);
        m.disk("vda1", Some("vda"), 8, &[(0, b"MSDOS5.0"), (82, b"FAT32")]); // the boot partition: not ours to touch
        m.disk("vda2", Some("vda"), 16, &[(0, PENDING_MARKER)]);
        m.disk("vdb", None, 16, &[(0, PENDING_MARKER)]); // a whole disk with the marker: never auto-formatted (needs a partition)
        assert_eq!(classify(&PENDING_MARKER[..]), Content::Pending);
        assert_eq!(classify(b"IYAKKI-FORMAT-ME-v2\0"), Content::Other("data of unknown type".into()), "other versions and lookalikes are not ours");
        assert_eq!(eligibility(&list(&m.roots).into_iter().find(|d| d.name == "vda2").unwrap(), &list(&m.roots)), Ok(None));
        assert_eq!(find_pending(&m.roots).unwrap().name, "vda2");
        assert_eq!(format_pending(&m.roots, &mke2fs), Ok(Some("vda2".to_string())));
        assert_eq!(probe(&m.root.join("dev/vda2")).unwrap(), Content::Iyakki);
        assert_eq!(probe(&m.root.join("dev/vda1")).unwrap(), Content::Other("FAT filesystem".into()), "the boot partition is untouched");
        assert!(find_pending(&m.roots).is_none(), "once formatted it is an ordinary data volume");
        assert_eq!(format_pending(&m.roots, &mke2fs), Ok(None));
        assert_eq!(find_data(&m.roots).unwrap().name, "vda2");
        // a mounted pending partition is left alone
        let m2 = machine("pending2");
        m2.disk("vda", None, 32, &[]);
        m2.disk("vda2", Some("vda"), 16, &[(0, PENDING_MARKER)]);
        m2.mounted("vda2", "/mnt");
        assert!(find_pending(&m2.roots).is_none());
        assert!(format_pending(&m2.roots, Path::new("/bin/false")).unwrap().is_none());
        // a failing mke2fs is an error, not a silent skip
        let m3 = machine("pending3");
        m3.disk("vda", None, 32, &[]);
        m3.disk("vda2", Some("vda"), 16, &[(0, PENDING_MARKER)]);
        assert!(format_pending(&m3.roots, Path::new("/bin/false")).unwrap_err().contains("mke2fs failed"));
    }

    #[test]
    fn lists_disks_and_partitions_and_hides_the_irrelevant() {
        let m = machine("list");
        m.disk("vda", None, 8, &[]);
        m.disk("vda1", Some("vda"), 4, &with(&ext4("IYAKKI-DATA")));
        m.disk("loop0", None, 8, &[]);
        m.disk("sr0", None, 8, &[]);
        m.disk("zram0", None, 8, &[]);
        m.mounted("vda", "/mnt");
        let l = list(&m.roots);
        assert_eq!(l.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["vda", "vda1"], "no loop/optical/zram");
        assert_eq!((l[0].model.as_str(), l[0].size_bytes, l[0].parent.clone()), ("Test Disk", 8 * 1024 * 1024, None));
        assert_eq!(l[0].mounted_at.as_deref(), Some("/mnt"));
        assert_eq!(l[1].parent.as_deref(), Some("vda"));
        assert_eq!(l[1].content, Content::Iyakki);
        assert_eq!(human(1_500_000_000), "1.5 GB");
    }

    #[test]
    fn the_data_volume_of_the_disk_the_system_started_from_wins() {
        // an internal disk (nvme0n1, sorts first) that holds an old install, and the stick (sda) the system started from
        let m = machine("bootdisk");
        m.disk("nvme0n1", None, 64, &[]);
        m.disk("nvme0n1p1", Some("nvme0n1"), 8, &[(0, b"MSDOS5.0"), (82, b"FAT32")]);
        m.disk("nvme0n1p2", Some("nvme0n1"), 16, &with(&ext4("IYAKKI-DATA")));
        m.disk("sda", None, 64, &[]);
        m.disk("sda1", Some("sda"), 8, &[(0, b"MSDOS5.0"), (82, b"FAT32")]);
        m.disk("sda2", Some("sda"), 16, &[(0, PENDING_MARKER)]);
        assert_eq!(find_data(&m.roots).unwrap().name, "nvme0n1p2", "without a boot disk the first volume is used");
        m.mounted("sda1", "/.stage1/boot");
        assert!(find_data(&m.roots).is_none(), "the stick's own volume is waiting to be formatted: not the internal disk's");
        assert_eq!(find_pending(&m.roots).unwrap().name, "sda2");
        // formatted: now it is the stick's volume that is used
        m.disk("sda2", Some("sda"), 16, &with(&ext4("IYAKKI-DATA")));
        assert_eq!(find_data(&m.roots).unwrap().name, "sda2");
        // a boot disk with no data volume of its own: no other disk is searched; the person chooses with iyakki.data=
        let m2 = machine("bootdisk2");
        m2.disk("sda", None, 64, &[]);
        m2.disk("sda1", Some("sda"), 8, &[(0, b"MSDOS5.0"), (82, b"FAT32")]);
        m2.disk("sdb", None, 16, &[]);
        m2.disk("sdb1", Some("sdb"), 16, &with(&ext4("IYAKKI-DATA")));
        m2.mounted("sda1", "/.stage1/boot");
        assert!(find_data(&m2.roots).is_none(), "its own volume is missing: it does not take another disk's");
        assert_eq!(find_data_for(&m2.roots, Some("/dev/sdb1")).unwrap().name, "sdb1", "an explicit choice is honoured");
        assert!(find_data_for(&m2.roots, Some("/dev/sda1")).is_none(), "a choice that is not a data volume is not used");
        assert!(find_data_for(&m2.roots, Some("/dev/nope")).is_none());
        // the choice wins over the system's own volume too
        assert_eq!(find_data_for(&m.roots, Some("/dev/nvme0n1p2")).unwrap().name, "nvme0n1p2");
    }

    #[test]
    fn finds_the_data_volume_only_when_it_is_free() {
        let m = machine("find");
        assert!(find_data(&m.roots).is_none());
        m.disk("vdb", None, 8, &with(&ext4("IYAKKI-DATA")));
        assert_eq!(find_data(&m.roots).unwrap().name, "vdb");
        m.mounted("vdb", "/var/lib/iyakki");
        assert!(find_data(&m.roots).is_none(), "already mounted");
        let m2 = machine("find2");
        m2.disk("vdb", None, 8, &with(&ext4("other")));
        assert!(find_data(&m2.roots).is_none(), "a label that is not IYAKKI-DATA is not ours");
    }

    #[test]
    fn eligibility_protects_the_boot_medium_and_anything_in_use() {
        let m = machine("elig");
        m.disk("sda", None, 8, &[(0x8001, b"CD001")]);
        m.disk("sda1", Some("sda"), 2, &[(82, b"FAT32")]);
        m.disk("sdb", None, 8, &[]);
        m.disk("sdc", None, 8, &[(0, b"hello")]);
        m.disk("sdd", None, 8, &[]);
        m.disk("sdd1", Some("sdd"), 4, &with(&ext4("photos")));
        m.mounted("sdd1", "/media");
        let all = list(&m.roots);
        let get = |n: &str| all.iter().find(|d| d.name == n).unwrap().clone();
        assert!(eligibility(&get("sda"), &all).unwrap_err().contains("booted from"));
        assert!(eligibility(&get("sda1"), &all).unwrap_err().contains("part of the medium"), "the stick's own partitions are protected too");
        assert_eq!(eligibility(&get("sdb"), &all), Ok(None));
        assert_eq!(eligibility(&get("sdc"), &all), Ok(Some("data of unknown type".into())), "needs force");
        assert!(eligibility(&get("sdd"), &all).unwrap_err().contains("sdd1 is mounted"));
        assert!(eligibility(&get("sdd1"), &all).unwrap_err().contains("in use"));
    }

    struct Fake {
        calls: Mutex<Vec<String>>,
        fail_mount: bool,
    }
    impl Mounter for Fake {
        fn mount(&self, dev: &Path, target: &Path) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("mount {} {}", dev.file_name().unwrap().to_string_lossy(), target.display()));
            if self.fail_mount {
                return Err("boom".into());
            }
            std::fs::create_dir_all(target).map_err(|e| e.to_string())
        }
        fn unmount(&self, target: &Path) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("umount {}", target.display()));
            Ok(())
        }
    }

    #[test]
    fn mke2fs_options_are_fast_and_safe() {
        let s = |n| mke2fs_args(Path::new("/dev/x"), n).iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
        let small = s(8 << 30);
        assert!(small.contains("nodiscard,lazy_itable_init=1,lazy_journal_init=1,hash_seed=") && small.contains(" -U "), "{small}");
        assert!(!small.contains("-i "), "{small}");
        let u = fresh_uuid();
        assert!(u.len() == 36 && u.as_bytes()[14] == b'4' && u != fresh_uuid(), "{u}");
        assert!(!small.contains("^has_journal"), "the journal stays");
        let big = s(931 << 30);
        assert!(big.contains("-i 65536") && big.contains("nodiscard") && big.ends_with("/dev/x"), "{big}");
    }

    fn mke2fs() -> Option<PathBuf> {
        ["/usr/sbin/mke2fs", "/sbin/mke2fs"].iter().map(PathBuf::from).find(|p| p.exists())
    }

    #[test]
    fn persist_formats_verifies_and_carries_the_current_data_over() {
        let Some(mke2fs) = mke2fs() else { return eprintln!("mke2fs not installed; skipping") };
        let m = machine("persist");
        m.disk("vdb", None, 16, &[]);
        let seed = m.root.join("seed");
        std::fs::create_dir_all(seed.join("kb")).unwrap();
        std::fs::write(seed.join("users.json"), "{\"u\":1}").unwrap();
        std::fs::write(seed.join("kb/a.afkb"), vec![1u8; 5000]).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", seed.join("evil")).unwrap();
        let f = Fake { calls: Mutex::new(vec![]), fail_mount: false };
        let r = persist(&m.roots, &f, &mke2fs, &m.root, "/dev/vdb", false, Some(&seed)).unwrap();
        assert_eq!((r.device.as_str(), r.files_copied, r.bytes_copied), ("vdb", 2, 5007), "symlinks are not followed or copied");
        assert_eq!(probe(&m.root.join("dev/vdb")).unwrap(), Content::Iyakki, "a fresh IYAKKI-DATA volume");
        let calls = f.calls.lock().unwrap().clone();
        assert!(calls[0].starts_with("mount vdb") && calls[1].starts_with("umount"), "{calls:?}");
        // the seed copy landed in the scratch mount point (here a plain directory standing in for the new fs)
        // and it is found at the next boot
        assert_eq!(find_data(&m.roots).unwrap().name, "vdb");
    }

    #[test]
    fn persist_refuses_unsafe_requests_and_demands_force_for_data() {
        let Some(mke2fs) = mke2fs() else { return eprintln!("mke2fs not installed; skipping") };
        let m = machine("persist-refuse");
        m.disk("vdb", None, 16, &[(0, b"precious")]);
        m.disk("vdc", None, 16, &[]);
        m.mounted("vdc", "/mnt");
        m.disk("vdd", None, 16, &[(0x8001, b"CD001")]);
        let f = Fake { calls: Mutex::new(vec![]), fail_mount: false };
        let p = |d: &str, force| persist(&m.roots, &f, &mke2fs, &m.root, d, force, None);
        assert!(p("vdb", false).unwrap_err().contains("refusing to erase"));
        assert_eq!(probe(&m.root.join("dev/vdb")).unwrap(), Content::Other("data of unknown type".into()), "untouched");
        assert!(p("vdc", true).unwrap_err().contains("in use"), "force never overrides a mounted disk");
        assert!(p("vdd", true).unwrap_err().contains("booted from"), "…or the boot medium");
        for bad in ["", "../etc", "a/b", "nosuchdisk"] {
            assert!(p(bad, true).is_err(), "{bad:?}");
        }
        assert!(f.calls.lock().unwrap().is_empty());
        // with force, the data is replaced
        assert!(p("vdb", true).is_ok());
        assert_eq!(probe(&m.root.join("dev/vdb")).unwrap(), Content::Iyakki);
        // production mode insists on a real block device
        let mut real = m.roots.clone();
        real.require_block = true;
        assert!(persist(&real, &f, &mke2fs, &m.root, "vdc2", false, None).is_err());
        m.disk("vde", None, 16, &[]);
        assert!(persist(&real, &f, &mke2fs, &m.root, "vde", false, None).unwrap_err().contains("not a block device"));
    }

    #[test]
    fn a_failing_mount_is_reported_and_mke2fs_errors_are_surfaced() {
        let Some(mke2fs) = mke2fs() else { return eprintln!("mke2fs not installed; skipping") };
        let m = machine("persist-fail");
        m.disk("vdb", None, 16, &[]);
        let seed = m.root.join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        let f = Fake { calls: Mutex::new(vec![]), fail_mount: true };
        assert_eq!(persist(&m.roots, &f, &mke2fs, &m.root, "vdb", false, Some(&seed)).unwrap_err(), "boom");
        let f = Fake { calls: Mutex::new(vec![]), fail_mount: false };
        let bad = persist(&m.roots, &f, Path::new("/nonexistent/mke2fs"), &m.root, "vdb", true, None).unwrap_err();
        assert!(bad.contains("could not run"), "{bad}");
        // a mke2fs that exits with an error is reported, and the disk is not claimed to be ours
        let m2 = machine("persist-tiny");
        m2.disk("vdb", None, 1, &[]);
        let e = persist(&m2.roots, &f, Path::new("/bin/false"), &m2.root, "vdb", false, None).unwrap_err();
        assert!(e.contains("mke2fs failed"), "{e}");
        assert_eq!(probe(&m2.root.join("dev/vdb")).unwrap(), Content::Empty);
    }

    #[test]
    fn mount_data_mounts_the_volume_over_the_data_dir_or_stays_in_ram() {
        let m = machine("mountdata");
        let f = Fake { calls: Mutex::new(vec![]), fail_mount: false };
        let dir = m.root.join("var-lib-iyakki");
        assert_eq!(mount_data(&m.roots, &f, &dir), Ok(None));
        assert!(f.calls.lock().unwrap().is_empty(), "no volume, nothing mounted");
        m.disk("vdb", None, 8, &with(&ext4("IYAKKI-DATA")));
        assert_eq!(mount_data(&m.roots, &f, &dir), Ok(Some("vdb".into())));
        assert!(f.calls.lock().unwrap()[0].ends_with("var-lib-iyakki"));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        let bad = Fake { calls: Mutex::new(vec![]), fail_mount: true };
        assert!(mount_data(&m.roots, &bad, &dir).is_err());
    }

    #[test]
    fn data_location_reads_the_mount_table() {
        let m = machine("loc");
        let write = |s: &str| std::fs::write(m.root.join("mounts"), s).unwrap();
        write("rootfs / rootfs rw 0 0\ntmpfs /run tmpfs rw 0 0\n");
        assert_eq!(data_location(&m.roots, Path::new("/var/lib/iyakki")), DataLocation::Memory);
        write("rootfs / rootfs rw 0 0\n/dev/vdb /var/lib/iyakki ext4 rw,nosuid 0 0\n");
        assert_eq!(data_location(&m.roots, Path::new("/var/lib/iyakki")), DataLocation::Disk("/dev/vdb".into()));
        assert_eq!(data_location(&m.roots, Path::new("/var/lib/iyakki/kb")), DataLocation::Disk("/dev/vdb".into()), "subdirectories too");
        assert_eq!(data_location(&m.roots, Path::new("/var/lib/other")), DataLocation::Memory);
        write("overlay / overlay rw 0 0\n");
        assert_eq!(data_location(&m.roots, Path::new("/var/lib/iyakki")), DataLocation::Other("overlay".into()));
        write("");
        assert_eq!(data_location(&m.roots, Path::new("/var/lib/iyakki")), DataLocation::Other("unknown".into()));
        let (free, total) = space(Path::new("/tmp")).unwrap();
        assert!(total > 0 && free <= total);
    }

    #[test]
    fn install_settings_ride_in_the_pending_partition() {
        let dir = std::env::temp_dir().join(format!("iyakki-pconf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("part");
        let mut disk = vec![0u8; 64 * 1024];
        disk[..PENDING_MARKER.len()].copy_from_slice(PENDING_MARKER);
        let b = pending_conf_bytes("zram=auto\nswapfile=2G\n").unwrap();
        disk[PENDING_CONF_OFFSET as usize..PENDING_CONF_OFFSET as usize + b.len()].copy_from_slice(&b);
        std::fs::write(&f, &disk).unwrap();
        assert_eq!(classify(&disk), Content::Pending, "the marker still decides");
        assert_eq!(read_pending_conf(&f).as_deref(), Some("zram=auto\nswapfile=2G\n"));
        std::fs::write(&f, vec![0u8; 64 * 1024]).unwrap();
        assert_eq!(read_pending_conf(&f), None, "no settings: nothing");
        assert_eq!(read_pending_conf(&dir.join("missing")), None);
        assert!(pending_conf_bytes(&"x".repeat(9000)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
