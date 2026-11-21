//! Tests of the partition planner and the plan writer. Disks are sparse regular files in temp directories (made to look like a
//! Windows laptop, an MBR disk, a 4096-byte-sector disk); no test opens a block device, and `apply` refuses to in a test build.

use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use iyakki_storage::gpt::{self, Entry, Guid, Table};

use crate::apply::{self, ApplyEnv};
use crate::fatclone::{tests::mini_fat, Fat32};
use crate::fatwrite::Fat32Rw;
use crate::gpt_install::{TableChoice, TableKind};
use crate::install::tests::{template, tmp};
use crate::install::{read_source, SourceInfo, MIB};
use crate::planner::*;

const GIB: u64 = 1 << 30;
const EFI: Guid = gpt::TYPE_ESP;
const MSDATA: Guid = [0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7];
const MSR: Guid = [0x16, 0xE3, 0xC9, 0xE3, 0x5C, 0x0B, 0xB8, 0x4D, 0x81, 0x7D, 0xF9, 0x2D, 0xF0, 0x02, 0x15, 0xAE];

// ---------------------------------------------------------------------------------------------------------------------------
// fixtures

/// A temp directory that is removed when the test ends (the disk images are big).
struct Guard(PathBuf);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(tag: &str) -> (PathBuf, Guard) {
    let d = tmp(tag);
    (d.clone(), Guard(d))
}

#[derive(Clone)]
enum Fill {
    Ntfs,
    Ext4,
    Swap,
    Blank,
    Raw(Vec<u8>),
}

struct Ps {
    first_mib: u64,
    len_mib: u64,
    ty: Guid,
    name: &'static str,
    fill: Fill,
}

fn ps(first_mib: u64, len_mib: u64, ty: Guid, name: &'static str, fill: Fill) -> Ps {
    Ps { first_mib, len_mib, ty, name, fill }
}

/// Recognisable bytes at the start (after the signature area), in the middle and at the very end of a partition, so that any
/// stray write into it is noticed.
fn decorate(f: &std::fs::File, off: u64, len: u64, seed: u8, fill: &Fill) {
    let pat = |n: usize| -> Vec<u8> { (0..n).map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed)).collect() };
    match fill {
        Fill::Raw(b) => f.write_all_at(b, off).unwrap(),
        Fill::Ntfs => f.write_all_at(b"\xEB\x52\x90NTFS    ", off).unwrap(),
        Fill::Swap => f.write_all_at(b"SWAPSPACE2", off + 4086).unwrap(),
        Fill::Ext4 => {
            f.write_all_at(&[0x53, 0xEF], off + 1024 + 56).unwrap();
            f.write_all_at(&0x40u32.to_le_bytes(), off + 1024 + 0x60).unwrap();
            f.write_all_at(b"rootfs", off + 1024 + 120).unwrap();
        }
        Fill::Blank => {}
    }
    if !matches!(fill, Fill::Raw(_) | Fill::Blank) && len > 3 * MIB {
        f.write_all_at(&pat(4096), off + 8192).unwrap();
        f.write_all_at(&pat(4096), off + len / 2).unwrap();
        f.write_all_at(&pat(4096), off + len - 4096).unwrap();
    }
}

/// A GPT disk of `size` bytes with `ss`-byte sectors holding `parts`.
fn gpt_disk(dir: &Path, name: &str, size: u64, ss: u32, parts: &[Ps]) -> (PathBuf, Table) {
    let path = dir.join(name);
    let f = std::fs::File::create(&path).unwrap();
    f.set_len(size).unwrap();
    let mut t = Table { sector_size: ss, total_sectors: size / ss as u64, disk_guid: gpt::guid_from_str("AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE").unwrap(), entries: vec![] };
    let k = MIB / ss as u64;
    for (i, p) in parts.iter().enumerate() {
        t.entries.push(Entry { index: i as u32 + 1, type_guid: p.ty, guid: [i as u8 + 0x40; 16], first_lba: p.first_mib * k, last_lba: (p.first_mib + p.len_mib) * k - 1, attrs: if i == 1 { 1 << 63 } else { 0 }, name: p.name.into() });
        decorate(&f, p.first_mib * MIB, p.len_mib * MIB, i as u8 + 1, &p.fill);
    }
    t.check().unwrap();
    // the foreign boot code of the protective MBR must survive an install
    let boot_code: Vec<u8> = (0..440).map(|i| (i % 250) as u8 + 1).collect();
    let mut mbr = t.protective_mbr(&boot_code, [9, 8, 7, 6]);
    mbr[0] = 0xEB;
    f.write_all_at(&mbr, 0).unwrap();
    let ent = t.entries_bytes();
    f.write_all_at(&t.header_bytes(false), ss as u64).unwrap();
    f.write_all_at(&ent, 2 * ss as u64).unwrap();
    f.write_all_at(&ent, t.backup_entries_lba() * ss as u64).unwrap();
    f.write_all_at(&t.header_bytes(true), t.last_lba() * ss as u64).unwrap();
    (path, t)
}

/// An MBR disk: (start MiB, length MiB, type, fill).
fn mbr_disk(dir: &Path, name: &str, size: u64, parts: &[(u64, u64, u8, Fill)]) -> PathBuf {
    let path = dir.join(name);
    let f = std::fs::File::create(&path).unwrap();
    f.set_len(size).unwrap();
    let mut s = vec![0u8; 512];
    for (i, b) in s[..440].iter_mut().enumerate() {
        *b = (i % 250) as u8 + 1;
    }
    s[440..444].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
    for (i, (start, len, ty, fill)) in parts.iter().enumerate() {
        let e = 446 + 16 * i;
        s[e] = if i == 0 { 0x80 } else { 0 };
        s[e + 4] = *ty;
        s[e + 8..e + 12].copy_from_slice(&((start * MIB / 512) as u32).to_le_bytes());
        s[e + 12..e + 16].copy_from_slice(&((len * MIB / 512) as u32).to_le_bytes());
        decorate(&f, start * MIB, len * MIB, i as u8 + 1, fill);
    }
    s[510] = 0x55;
    s[511] = 0xAA;
    f.write_all_at(&s, 0).unwrap();
    path
}

fn model_of(path: &Path, name: &str, size: u64, ss: u32, mounts: &str, usb: bool) -> DiskModel {
    let meta = DiskMeta { name: name.into(), path: path.display().to_string(), model: "Test disk".into(), serial: "T1".into(), removable: usb, usb, ..Default::default() };
    apply::model_of_file(path, &meta, size, ss, mounts, None).unwrap()
}

/// A template with a real FAT32 boot volume (kernel, RAM disk, boot menu).
fn src(dir: &Path) -> SourceInfo {
    src_with(dir, &[("BOOT/BZIMAGE", pseudo(700_001, 3)), ("BOOT/INITRD", pseudo(1_500_000, 4)), ("BOOT/GRUB/GRUB.CFG", b"menuentry \"IYAKKI\" {\n\tlinux /boot/bzImage console=tty0 quiet\n\tinitrd /boot/initrd\n}\n".to_vec())])
}

/// The same template for the aarch64 image: `Image` (an EFI stub) and `rootfs.cpio.gz` at the root of the boot volume, no GRUB.
fn src_arm(dir: &Path) -> SourceInfo {
    src_with(dir, &[("Image", pseudo(900_001, 5)), ("rootfs.cpio.gz", pseudo(1_200_000, 6))])
}

fn src_with(dir: &Path, files: &[(&str, Vec<u8>)]) -> SourceInfo {
    let fat = mini_fat(512, 1, 40 * MIB, files);
    let path = dir.join("t.img");
    let mut t = template(&path);
    let p1_sectors = (fat.len() / 512) as u32;
    let p2_start = 2048 + p1_sectors;
    t[446 + 12..446 + 16].copy_from_slice(&p1_sectors.to_le_bytes());
    t[462 + 8..462 + 12].copy_from_slice(&p2_start.to_le_bytes());
    t.truncate(2048 * 512);
    t.extend_from_slice(&fat);
    t.resize((p2_start as usize + 2048) * 512, 0);
    std::fs::write(&path, &t).unwrap();
    read_source(&path).unwrap()
}

fn pseudo(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed).wrapping_add((i >> 8) as u8)).collect()
}

fn spec(s: &SourceInfo) -> SourceSpec {
    apply::source_spec(s).unwrap()
}

/// A 40 MiB FAT32 volume like another system's EFI System Partition, always hand-made (a host `mkfs.fat` writes its FSInfo sector
/// differently from host to host: KI-706), with the other system's loaders added with our own writer.
fn foreign_esp(dir: &Path) -> Vec<u8> {
    let p = dir.join("esp.img");
    let f = std::fs::File::create(&p).unwrap();
    f.set_len(40 * MIB).unwrap();
    drop(f);
    let mut img = mini_fat(512, 1, 40 * MIB, &[]);
    img[82..90].copy_from_slice(b"FAT32   ");
    // what a real formatter also writes: the extended boot signature (so the label in the boot sector counts), a backup of the boot
    // sector (sector 6) and the FSInfo sector (sector 1) with the TRUE free-cluster count (the writer adjusts it by what it allocates)
    img[66] = 0x29;
    img[48..50].copy_from_slice(&1u16.to_le_bytes());
    img[50..52].copy_from_slice(&6u16.to_le_bytes());
    let (reserved, fatsz, total) = (u16::from_le_bytes([img[14], img[15]]) as usize, u32::from_le_bytes(img[36..40].try_into().unwrap()) as usize, u32::from_le_bytes(img[32..36].try_into().unwrap()) as usize);
    let clusters = (total - reserved - 2 * fatsz) / img[13] as usize;
    let entry = |i: usize| u32::from_le_bytes(img[reserved * 512 + 4 * i..][..4].try_into().unwrap()) & 0x0FFF_FFFF;
    let free = (2..clusters + 2).filter(|i| entry(*i) == 0).count() as u32;
    let next = (2..clusters + 2).find(|i| entry(*i) == 0).unwrap() as u32;
    let mut fsinfo = vec![0u8; 512];
    fsinfo[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    fsinfo[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    fsinfo[488..492].copy_from_slice(&free.to_le_bytes());
    fsinfo[492..496].copy_from_slice(&next.to_le_bytes());
    fsinfo[508..512].copy_from_slice(&[0, 0, 0x55, 0xAA]);
    img[512..1024].copy_from_slice(&fsinfo);
    let boot = img[..512].to_vec();
    img[6 * 512..7 * 512].copy_from_slice(&boot);
    img[7 * 512..8 * 512].copy_from_slice(&fsinfo);
    // the other system's loaders, added with our own writer (the same code the real install uses)
    let f = std::fs::OpenOptions::new().read(true).write(true).open(&p).unwrap();
    f.write_all_at(&img, 0).unwrap();
    {
        let mut rw = Fat32Rw::open(&f, 0, 40 * MIB).unwrap();
        let (d, _) = rw.mkdir_p(&["EFI", "MSFT", "BOOT"]).unwrap();
        rw.create_file(d, "BOOTMGFW.EFI", 300_000, &mut std::io::Cursor::new(pseudo(300_000, 9)), &mut |_| {}).unwrap();
        let (d, _) = rw.mkdir_p(&["EFI", "BOOT"]).unwrap();
        rw.create_file(d, "BOOTX64.EFI", 123_456, &mut std::io::Cursor::new(pseudo(123_456, 8)), &mut |_| {}).unwrap();
        rw.flush().unwrap();
        rw.write_fsinfo().unwrap();
    }
    img = std::fs::read(&p).unwrap();
    img
}

/// A Windows-like laptop disk: ESP 100 MiB, reserved 16 MiB, "C:" NTFS 12 GiB, recovery 1 GiB, then free space to the end.
fn windows_disk(dir: &Path, size: u64, ss: u32) -> (PathBuf, Table) {
    let esp = foreign_esp(dir);
    gpt_disk(
        dir,
        "win.img",
        size,
        ss,
        &[ps(1, 100, EFI, "EFI system partition", Fill::Raw(esp)), ps(101, 16, MSR, "Microsoft reserved", Fill::Blank), ps(117, 12 * 1024, MSDATA, "Basic data partition", Fill::Ntfs), ps(117 + 12 * 1024, 1024, gpt::guid_from_str("DE94BBA4-06D1-4D40-A16A-BFD50179D6AC").unwrap(), "Windows RE", Fill::Ntfs)],
    )
}

/// A hash of the whole file that visits only the parts that hold data (`SEEK_DATA`/`SEEK_HOLE`): sparse images stay cheap in a
/// debug build, and a write into a hole changes the hash.
fn whole_file_hash(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    use std::os::fd::AsRawFd;
    let f = std::fs::File::open(path).unwrap();
    let len = f.metadata().unwrap().len();
    let fd = f.as_raw_fd();
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut off: i64 = 0;
    loop {
        // SAFETY: lseek on an open descriptor.
        let data = unsafe { libc::lseek(fd, off, libc::SEEK_DATA) };
        if data < 0 {
            break;
        }
        let hole = unsafe { libc::lseek(fd, data, libc::SEEK_HOLE) };
        let end = if hole < 0 { len as i64 } else { hole };
        let mut at = data;
        while at < end {
            let n = ((end - at) as usize).min(buf.len());
            f.read_exact_at(&mut buf[..n], at as u64).unwrap();
            if buf[..n].iter().any(|b| *b != 0) {
                h.update(at.to_le_bytes());
                h.update(&buf[..n]);
            }
            at += n as i64;
        }
        off = end;
        if off as u64 >= len {
            break;
        }
    }
    format!("{:x}", h.finalize())
}

fn bytes_at(path: &Path, off: u64, len: usize) -> Vec<u8> {
    let mut v = vec![0u8; len];
    std::fs::File::open(path).unwrap().read_exact_at(&mut v, off).unwrap();
    v
}

fn run_apply(path: &Path, model: &DiskModel, req: &Request, s: &SourceInfo, digest: Option<&str>, env: &ApplyEnv) -> Result<apply::ApplyReport, String> {
    apply::apply(path, model, req, s, digest, true, env, &mut |_| {})
}

fn tool(cmd: &str, args: &[&str]) -> Option<(bool, String)> {
    match std::process::Command::new(cmd).args(args).output() {
        Ok(o) => Some((o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))),
        Err(_) => {
            eprintln!("{cmd} is not installed: skipped");
            None
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// reading disks

#[test]
fn filesystems_are_recognised_by_signature() {
    let (dir, _guard) = scratch("sniff");
    let f = std::fs::File::create(dir.join("x")).unwrap();
    f.set_len(80 * MIB).unwrap();
    let check = |write: &dyn Fn(&std::fs::File), want: &str| {
        f.set_len(0).unwrap();
        f.set_len(80 * MIB).unwrap();
        write(&f);
        let mut r = std::fs::File::open(dir.join("x")).unwrap();
        assert_eq!(sniff_fs(&mut r, 0, 80 * MIB), want);
    };
    check(&|_| {}, "");
    check(&|f| f.write_all_at(b"\xEB\x52\x90NTFS    ", 0).unwrap(), "ntfs");
    check(&|f| f.write_all_at(b"\xEB\x76\x90EXFAT   ", 0).unwrap(), "exfat");
    check(&|f| f.write_all_at(b"FAT32   ", 82).unwrap(), "fat32");
    check(&|f| f.write_all_at(b"FAT16   ", 54).unwrap(), "fat16");
    check(&|f| f.write_all_at(b"XFSB", 0).unwrap(), "xfs");
    check(&|f| f.write_all_at(b"LUKS\xba\xbe", 0).unwrap(), "luks");
    check(&|f| f.write_all_at(b"SWAPSPACE2", 4086).unwrap(), "swap");
    check(&|f| f.write_all_at(b"_BHRfS_M", 0x10040).unwrap(), "btrfs");
    check(&|f| f.write_all_at(&iyakki_storage::PENDING_MARKER[..], 0).unwrap(), "iyakki-pending");
    check(
        &|f| {
            f.write_all_at(&[0x53, 0xEF], 1080).unwrap();
            f.write_all_at(&0x40u32.to_le_bytes(), 1024 + 0x60).unwrap();
        },
        "ext4",
    );
    check(&|f| f.write_all_at(&[0x53, 0xEF], 1080).unwrap(), "ext2");
}

#[test]
fn a_windows_disk_is_read_with_types_labels_filesystems_and_free_space() {
    let (dir, _guard) = scratch("win");
    let (path, _) = windows_disk(&dir, 40 * GIB, 512);
    let m = model_of(&path, "sda", 40 * GIB, 512, "/dev/sda3 /media/user/OS ntfs rw 0 0\n", false);
    assert_eq!(m.table, Some(TableKind::Gpt));
    assert_eq!(m.parts.len(), 4);
    let esp = &m.parts[0];
    assert!(esp.is_esp() && esp.fs == "fat32" && esp.label == "EFI system partition" && esp.name == "sda1");
    assert!(esp.fat_free.is_some_and(|f| f > 30 * MIB), "FSInfo free space {:?}", esp.fat_free);
    assert_eq!(m.parts[2].fs, "ntfs");
    assert_eq!(m.parts[2].mounts, ["/media/user/OS"]);
    assert!(!m.running_system, "an ordinary mount does not make it the system disk");
    let g = m.free_gaps(TableKind::Gpt);
    assert_eq!(g.len(), 1, "{g:?}");
    assert_eq!(g[0].first_lba, (117 + 12 * 1024 + 1024) * MIB / 512);
    assert_eq!(g[0].first_lba % 2048, 0, "1 MiB aligned");
    // the gap stops before the backup table: 33 sectors of entries + header at the end
    let end = (g[0].first_lba + g[0].sectors) * 512;
    assert!(end <= 40 * GIB - 33 * 512 && end % MIB == 0, "{end}");
    assert!(40 * GIB - end < 2 * MIB);
    // the running system
    let m2 = model_of(&path, "sda", 40 * GIB, 512, "/dev/sda3 / ext4 rw 0 0\n", false);
    assert!(m2.running_system);
    let m3 = model_of(&path, "sda", 40 * GIB, 512, "/dev/sda3 /var/lib/iyakki ext4 rw 0 0\n", false);
    assert!(m3.running_system, "the IYAKKI data volume counts");
    // the fingerprint follows the table
    assert_eq!(m.fingerprint, model_of(&path, "sda", 40 * GIB, 512, "", false).fingerprint);
    let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.write_all_at(&[0xFF; 16], 2 * 512 + 128 * 4).unwrap();
    assert_ne!(m.fingerprint, model_of(&path, "sda", 40 * GIB, 512, "", false).fingerprint);
}

#[test]
fn gaps_follow_unaligned_foreign_partitions_and_4096_byte_sectors() {
    let (dir, _guard) = scratch("gaps");
    // an old Windows XP style MBR: first partition at sector 63
    let path = dir.join("xp.img");
    let f = std::fs::File::create(&path).unwrap();
    f.set_len(10 * GIB).unwrap();
    let mut s = vec![0u8; 512];
    s[446 + 4] = 0x07;
    s[446 + 8..446 + 12].copy_from_slice(&63u32.to_le_bytes());
    s[446 + 12..446 + 16].copy_from_slice(&(2 * GIB / 512 - 63).to_le_bytes_u32());
    s[510] = 0x55;
    s[511] = 0xAA;
    f.write_all_at(&s, 0).unwrap();
    let m = model_of(&path, "sdb", 10 * GIB, 512, "", false);
    assert_eq!(m.table, Some(TableKind::Mbr));
    let g = m.free_gaps(TableKind::Mbr);
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].first_lba, 2 * GIB / 512, "starts at the next MiB after the partition's end");
    assert_eq!((g[0].first_lba + g[0].sectors) * 512, 10 * GIB);
    // 4096-byte sectors: the same disk in 4 KiB units
    let (p4, _) = gpt_disk(&dir, "k4.img", 8 * GIB, 4096, &[ps(1, 1024, MSDATA, "data", Fill::Ntfs)]);
    let m4 = model_of(&p4, "nvme1n1", 8 * GIB, 4096, "", false);
    let g4 = m4.free_gaps(TableKind::Gpt);
    assert_eq!(g4[0].first_lba, 1025 * MIB / 4096);
    assert_eq!(g4[0].first_lba % 256, 0);
    assert_eq!(m4.parts[0].name, "nvme1n1p1");
    let end = (g4[0].first_lba + g4[0].sectors) * 4096;
    assert!(end <= 8 * GIB - 5 * 4096 && end % MIB == 0, "backup table (4 sectors + header) is left alone");
}

trait ToLe {
    fn to_le_bytes_u32(self) -> [u8; 4];
}
impl ToLe for u64 {
    fn to_le_bytes_u32(self) -> [u8; 4] {
        (self as u32).to_le_bytes()
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// the plan and its rules

fn free_req() -> Request {
    Request { mode: Mode::FreeSpace, ..Request::default() }
}

#[test]
fn free_space_mode_adds_two_partitions_in_the_gap_and_touches_nothing_else() {
    let (dir, _guard) = scratch("free");
    let s = src(&dir);
    let (path, _) = windows_disk(&dir, 40 * GIB, 512);
    let m = model_of(&path, "sda", 40 * GIB, 512, "", false);
    let plan = build_plan(&m, &free_req(), &spec(&s), true).unwrap();
    assert_eq!(plan.steps.iter().filter(|st| st.action == Action::Keep).count(), 4, "all four existing partitions are kept");
    assert!(plan.destroyed().is_empty());
    let creates: Vec<&Step> = plan.steps.iter().filter(|st| st.action == Action::Create).collect();
    assert_eq!(creates.len(), 3, "BIOS boot, boot and data");
    assert_eq!((creates[0].role, creates[0].bytes), (Some(Role::Bios), MIB), "the BIOS boot partition comes first");
    let gap = m.largest_gap(TableKind::Gpt).unwrap();
    assert_eq!(creates[0].index, 5, "the next free slot");
    assert_eq!(creates[0].first_lba, gap.first_lba);
    let creates: Vec<&Step> = creates.into_iter().skip(1).collect();
    assert_eq!(creates[0].index, 6);
    let gap = crate::planner::Gap { first_lba: gap.first_lba + MIB / 512, sectors: gap.sectors - MIB / 512 };
    assert_eq!(creates[0].bytes, spec(&s).boot_required(512), "boot size comes from the image");
    assert_eq!(creates[1].first_lba, gap.first_lba + creates[0].sectors);
    assert_eq!(creates[1].first_lba + creates[1].sectors, gap.first_lba + gap.sectors, "data takes the rest of the gap");
    assert!(creates.iter().all(|c| c.first_lba % 2048 == 0 && c.sectors % 2048 == 0));
    let text = plan.render();
    for want in ["keep", "create", "sda3", "ntfs", "IYAKKI-BOOT", "IYAKKI-DATA", "Nothing that exists now is erased", "Free space only"] {
        assert!(text.contains(want), "{want} in\n{text}");
    }
    let json = plan.to_json().pretty();
    assert!(json.contains("\"destroys\": []") && json.contains("iyakki-install-plan-1"));
    // the digest follows the plan
    assert_eq!(plan.digest(), build_plan(&m, &free_req(), &spec(&s), true).unwrap().digest());
    let custom = Request { mode: Mode::Custom, data_mib: Some(2048), boot_mib: Some(64), ..Request::default() };
    let p2 = build_plan(&m, &custom, &spec(&s), true).unwrap();
    assert_ne!(p2.digest(), plan.digest());
    let c2: Vec<&Step> = p2.steps.iter().filter(|st| st.action == Action::Create).collect();
    assert_eq!((c2[0].bytes, c2[1].bytes, c2[2].bytes), (MIB, 64 * MIB, 2048 * MIB));
}

#[test]
fn the_rules_refuse_what_would_hurt_another_system() {
    let (dir, _guard) = scratch("rules");
    let s = src(&dir);
    let sp = spec(&s);
    let (path, _) = windows_disk(&dir, 40 * GIB, 512);
    let base = model_of(&path, "sda", 40 * GIB, 512, "", false);
    // the running system's disk: refused unless the person confirmed; whole-disk never
    let running = model_of(&path, "sda", 40 * GIB, 512, "/dev/sda3 / ext4 rw 0 0\n", false);
    let e = build_plan(&running, &free_req(), &sp, true).unwrap_err();
    assert!(e.contains("running system") && e.contains("--confirm-running-disk"), "{e}");
    let ok = Request { confirm_running_disk: true, ..free_req() };
    let p = build_plan(&running, &ok, &sp, true).unwrap();
    assert!(p.running_disk && p.warnings.iter().any(|w| w.contains("running right now")));
    let e = build_plan(&running, &Request { confirm_running_disk: true, ..Request::default() }, &sp, true).unwrap_err();
    assert!(e.contains("erasing it is refused"), "{e}");
    // the boot stick is never a target
    let mut stick = base.clone();
    stick.booted_from = true;
    assert!(build_plan(&stick, &ok, &sp, true).unwrap_err().contains("never a target"));
    // no shrinking: a custom size larger than the gap is refused, and says why
    let gap_mib = base.largest_gap(TableKind::Gpt).unwrap().bytes(512) / MIB;
    let big = Request { mode: Mode::Custom, data_mib: Some(gap_mib + 10), ..Request::default() };
    let e = build_plan(&base, &big, &sp, true).unwrap_err();
    assert!(e.contains("do not fit") && e.contains("nothing existing is shrunk"), "{e}");
    // no free space at all: points to the OS tool
    let (full, _) = gpt_disk(&dir, "full.img", 4 * GIB, 512, &[ps(1, 4000, MSDATA, "all", Fill::Ntfs)]);
    let mfull = model_of(&full, "sdc", 4 * GIB, 512, "", false);
    let e = build_plan(&mfull, &free_req(), &sp, true).unwrap_err();
    assert!(e.contains("shrink a partition") && e.contains("never resizes"), "{e}");
    // too small a gap
    let (tiny, _) = gpt_disk(&dir, "tiny.img", 4 * GIB, 512, &[ps(1, 4000, MSDATA, "most", Fill::Ntfs)]);
    let e = build_plan(&model_of(&tiny, "sdc", 4 * GIB, 512, "", false), &free_req(), &sp, true).unwrap_err();
    assert!(e.contains("too small"), "{e}");
    // a filesystem on the whole disk (no table)
    let raw = dir.join("raw.img");
    let f = std::fs::File::create(&raw).unwrap();
    f.set_len(4 * GIB).unwrap();
    f.write_all_at(b"\xEB\x52\x90NTFS    ", 0).unwrap();
    let e = build_plan(&model_of(&raw, "sdd", 4 * GIB, 512, "", false), &free_req(), &sp, true).unwrap_err();
    assert!(e.contains("no partition table"), "{e}");
    // swap, read only, 1024-byte sectors
    let mut ro = base.clone();
    ro.read_only = true;
    assert!(build_plan(&ro, &free_req(), &sp, true).unwrap_err().contains("write-protected"));
    let mut odd = base.clone();
    odd.sector = 1024;
    assert!(build_plan(&odd, &free_req(), &sp, true).unwrap_err().contains("1024-byte"));
    // a GPT whose entry array is not 128 x 128 is not rewritten
    let mut weird = base.clone();
    weird.unusual_gpt = true;
    assert!(build_plan(&weird, &free_req(), &sp, true).unwrap_err().contains("unusual"));
}

#[test]
fn mbr_disks_have_four_primary_slots_and_no_extended_games() {
    let (dir, _guard) = scratch("mbr-rules");
    let s = src(&dir);
    let sp = spec(&s);
    let full = mbr_disk(&dir, "four.img", 20 * GIB, &[(1, 1000, 0x07, Fill::Ntfs), (1001, 1000, 0x83, Fill::Ext4), (2001, 1000, 0x83, Fill::Ext4), (3001, 1000, 0x82, Fill::Swap)]);
    let m = model_of(&full, "sde", 20 * GIB, 512, "", false);
    let e = build_plan(&m, &free_req(), &sp, true).unwrap_err();
    assert!(e.contains("4 primary partitions") && e.contains("reuse"), "{e}");
    // with three used, two are needed: one slot left is not enough, but keeping the ESP needs only one
    let three = mbr_disk(&dir, "three.img", 20 * GIB, &[(1, 100, 0xEF, Fill::Raw(foreign_esp(&dir))), (101, 1000, 0x07, Fill::Ntfs), (1101, 1000, 0x83, Fill::Ext4)]);
    let m3 = model_of(&three, "sdf", 20 * GIB, 512, "", false);
    assert_eq!(m3.parts.len(), 3);
    let e = build_plan(&m3, &free_req(), &sp, true).unwrap_err();
    assert!(e.contains("uses 3 of the 4"), "{e}");
    let keep = build_plan(&m3, &Request { keep_esp: true, ..free_req() }, &sp, true).unwrap();
    assert_eq!(keep.steps.iter().filter(|s| s.action == Action::Create).count(), 1, "keeping the ESP needs one slot");
}

#[test]
fn a_too_small_efi_partition_moves_boot_to_a_partition_of_its_own_and_says_so() {
    let (dir, _guard) = scratch("wiz-esp-full");
    let s = src(&dir);
    let (path, _) = windows_disk(&dir, 40 * GIB, 512);
    let mut m = model_of(&path, "sda", 40 * GIB, 512, "", false);
    m.parts[0].fat_free = Some(2 * MIB);
    let mut w = Wizard::new(vec![m], spec(&s), true);
    w.handle(WKey::Enter);
    // free-space mode with the existing EFI System Partition kept: it has no room
    w.handle(WKey::Char('e'));
    assert!(w.req.keep_esp);
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Choose);
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Plan, "{:?}", w.error);
    let p = w.plan.as_ref().unwrap();
    assert!(!p.keep_esp, "the plan has a boot partition of its own");
    assert_eq!(p.steps.iter().filter(|s| s.action == Action::Create).count(), 3, "BIOS boot, boot and data are all created");
    assert!(p.warnings[0].contains("free but") && p.warnings[0].contains("own boot partition"), "{:?}", p.warnings);
    assert!(p.destroyed().is_empty(), "the other system's partitions are untouched");
    assert!(!w.req.keep_esp);
}

#[test]
fn reuse_mode_formats_only_the_two_chosen_partitions() {
    let (dir, _guard) = scratch("reuse");
    let s = src(&dir);
    let sp = spec(&s);
    let (path, _) = gpt_disk(
        &dir,
        "reuse.img",
        20 * GIB,
        512,
        &[ps(1, 100, EFI, "EFI", Fill::Raw(foreign_esp(&dir))), ps(101, 600, MSDATA, "old-recovery", Fill::Ntfs), ps(701, 4000, MSDATA, "spare", Fill::Ext4), ps(4701, 2000, MSDATA, "windows", Fill::Ntfs)],
    );
    let m = model_of(&path, "sda", 20 * GIB, 512, "/dev/sda4 /media/u/windows ntfs rw 0 0\n", false);
    let req = Request { mode: Mode::Reuse, boot_part: Some(2), data_part: Some(3), ..Request::default() };
    let p = build_plan(&m, &req, &sp, true).unwrap();
    assert_eq!(p.destroyed(), ["sda2", "sda3"]);
    let fmt: Vec<&Step> = p.steps.iter().filter(|s| s.action == Action::Format).collect();
    assert!(fmt[0].loses.contains("ntfs") && fmt[1].loses.contains("ext4") && fmt[1].loses.contains("spare"), "{:?}", fmt);
    assert_eq!(p.steps.iter().filter(|s| s.action == Action::Keep).count(), 2);
    assert!(p.unmount.is_empty(), "the mounted partition is kept, so it is not unmounted");
    // refusals
    let same = Request { data_part: Some(2), ..req.clone() };
    assert!(build_plan(&m, &same, &sp, true).unwrap_err().contains("different"));
    let small = Request { boot_part: Some(1), data_part: Some(3), ..req.clone() };
    let big_image = SourceSpec { boot_bytes: 512 * MIB, ..sp.clone() };
    assert!(build_plan(&m, &small, &big_image, true).unwrap_err().contains("this image needs"), "the PC image needs 512 MB for boot");
    let missing = Request { data_part: Some(9), ..req.clone() };
    assert!(build_plan(&m, &missing, &sp, true).unwrap_err().contains("does not exist"));
    let no_data = Request { data_part: None, ..req.clone() };
    assert!(build_plan(&m, &no_data, &sp, true).unwrap_err().contains("DATA"));
    // choosing the mounted partition: it must be unmounted, listed in the plan; its running-system mount is never allowed
    let mounted = Request { data_part: Some(4), ..req.clone() };
    let pm = build_plan(&m, &mounted, &sp, true).unwrap();
    assert_eq!(pm.unmount, ["sda4"]);
    let sys = model_of(&path, "sda", 20 * GIB, 512, "/dev/sda4 / ext4 rw 0 0\n", false);
    let e = build_plan(&sys, &Request { confirm_running_disk: true, ..mounted.clone() }, &sp, true).unwrap_err();
    assert!(e.contains("running system"), "{e}");
    // the only ESP: formatting it is flagged
    let esp_req = Request { boot_part: Some(1), data_part: Some(3), ..req.clone() };
    let pe = build_plan(&m, &esp_req, &sp, true).unwrap();
    assert!(pe.warnings.iter().any(|w| w.contains("only EFI System Partition") && w.contains("--keep-esp")));
}

#[test]
fn keep_esp_needs_a_free_unmounted_fat32_esp_with_room() {
    let (dir, _guard) = scratch("keep");
    let s = src(&dir);
    let sp = spec(&s);
    let (path, _) = windows_disk(&dir, 40 * GIB, 512);
    let m = model_of(&path, "sda", 40 * GIB, 512, "", false);
    let req = Request { keep_esp: true, ..free_req() };
    let p = build_plan(&m, &req, &sp, true).unwrap();
    assert!(p.keep_esp && p.boot_index == 1);
    assert_eq!(p.steps.iter().filter(|s| s.action == Action::Create).count(), 1, "only the data partition is created");
    assert!(p.steps.iter().any(|s| s.action == Action::AddFiles && s.part == "sda1"));
    assert!(p.destroyed().is_empty());
    assert!(p.warnings.iter().any(|w| w.contains("EFI/IYAKKI")));
    // mounted ESP
    let mounted = model_of(&path, "sda", 40 * GIB, 512, "/dev/sda1 /boot/efi vfat rw 0 0\n", false);
    let e = build_plan(&mounted, &Request { confirm_running_disk: true, ..req.clone() }, &sp, true).unwrap_err();
    assert!(e.contains("mounted") && e.contains("boot-time install mode"), "{e}");
    // not enough room
    let mut full = m.clone();
    full.parts[0].fat_free = Some(2 * MIB);
    let e = build_plan(&full, &req, &sp, true).unwrap_err();
    assert!(e.contains("free but the IYAKKI kernel"), "{e}");
    // not FAT32
    let mut ntfs = m.clone();
    ntfs.parts[0].fs = "ntfs".into();
    assert!(build_plan(&ntfs, &req, &sp, true).unwrap_err().contains("FAT32"));
    // no ESP, or two
    let (none, _) = gpt_disk(&dir, "noesp.img", 8 * GIB, 512, &[ps(1, 1000, MSDATA, "d", Fill::Ntfs)]);
    assert!(build_plan(&model_of(&none, "sdb", 8 * GIB, 512, "", false), &req, &sp, true).unwrap_err().contains("no EFI System Partition"));
    let mut two = m.clone();
    two.parts[1].type_guid = Some(EFI);
    assert!(build_plan(&two, &req, &sp, true).unwrap_err().contains("2 EFI System Partitions"));
}

#[test]
fn the_request_round_trips_through_the_plan_file() {
    let r = Request { mode: Mode::Reuse, gap_start: Some(4096), boot_part: Some(2), data_part: Some(3), keep_esp: true, boot_mib: Some(600), data_mib: None, table: TableChoice::Gpt, confirm_running_disk: true, delete_parts: vec![4, 5], zram: "4G".into(), swapfile: "2G".into(), hostname: Some("box".into()), ..Request::default() };
    let j = r.to_json();
    assert_eq!(Request::from_json(&crate::json::Json::parse(&j.pretty()).unwrap()).unwrap(), r);
    assert!(Request::from_json(&crate::json::Json::parse("{\"mode\":\"nope\"}").unwrap()).is_err());
    assert!(Request::from_json(&crate::json::Json::parse("{\"mode\":\"reuse\",\"zram\":\"banana\"}").unwrap()).is_err());
    assert!(Mode::parse("custom").is_ok() && Mode::parse("whole").is_ok());
}

#[test]
fn the_confirmation_is_the_name_and_the_size() {
    let (dir, _guard) = scratch("confirm");
    let s = src(&dir);
    let (path, _) = windows_disk(&dir, 40 * GIB, 512);
    let m = model_of(&path, "sda", 40 * GIB, 512, "", false);
    let p = build_plan(&m, &free_req(), &spec(&s), true).unwrap();
    assert!(confirmation_ok(&p, "sda", "42.9 GB"));
    assert!(confirmation_ok(&p, &format!(" {} ", p.disk_path), "42.9GB"));
    assert!(!confirmation_ok(&p, "sdb", "42.9 GB") && !confirmation_ok(&p, "sda", "40 GB") && !confirmation_ok(&p, "", ""));
}

// ---------------------------------------------------------------------------------------------------------------------------
// writing

fn parsed(path: &Path, ss: u32, size: u64) -> gpt::Parsed {
    let mut h = std::fs::File::open(path).unwrap();
    gpt::read_gpt(&mut h, ss, size / ss as u64).unwrap()
}

/// A fingerprint of each old partition: its first and last MiB and the middle (the fixtures put recognisable bytes there).
fn old_partition_bytes(path: &Path, t: &Table, ss: u32) -> Vec<(u32, String)> {
    use sha2::{Digest, Sha256};
    let f = std::fs::File::open(path).unwrap();
    t.entries
        .iter()
        .map(|e| {
            let (off, len) = (e.first_lba * ss as u64, e.sectors() * ss as u64);
            let mut h = Sha256::new();
            let mut take = |o: u64, n: u64| {
                let mut buf = vec![0u8; n as usize];
                f.read_exact_at(&mut buf, off + o).unwrap();
                h.update(&buf);
            };
            take(0, len.min(MIB));
            if len > 3 * MIB {
                take(len / 2, 4096);
                take(len - MIB, MIB);
            }
            (e.index, format!("{:x}", h.finalize()))
        })
        .collect()
}

#[test]
fn free_space_install_on_a_windows_disk_changes_only_the_table_tail_and_the_new_partitions() {
    let (dir, _guard) = scratch("apply-free");
    let s = src(&dir);
    let (path, old) = gpt_disk(
        &dir,
        "w2.img",
        6 * GIB,
        512,
        &[ps(1, 100, EFI, "EFI system partition", Fill::Raw(foreign_esp(&dir))), ps(101, 16, MSR, "Microsoft reserved", Fill::Blank), ps(117, 2048, MSDATA, "Basic data partition", Fill::Ntfs), ps(2165, 512, gpt::guid_from_str("DE94BBA4-06D1-4D40-A16A-BFD50179D6AC").unwrap(), "Windows RE", Fill::Ntfs)],
    );
    let before = old_partition_bytes(&path, &old, 512);
    let sector0 = bytes_at(&path, 0, 512);
    let m = model_of(&path, "sda", 6 * GIB, 512, "", true);
    let plan = build_plan(&m, &free_req(), &spec(&s), true).unwrap();
    let rep = run_apply(&path, &m, &free_req(), &s, Some(&plan.digest()), &ApplyEnv::default()).unwrap();
    assert!(rep.verified);
    let p = parsed(&path, 512, 6 * GIB);
    assert!(p.primary_ok && p.backup_ok && p.problems.is_empty(), "{:?}", p.problems);
    assert_eq!(p.table.disk_guid, old.disk_guid, "the disk keeps its identity");
    assert_eq!(p.table.entries.len(), 7);
    for e in &old.entries {
        assert_eq!(p.table.entries.iter().find(|x| x.index == e.index).unwrap(), e, "entry {} is byte for byte the same", e.index);
    }
    assert_eq!(old_partition_bytes(&path, &old, 512), before, "old partitions: not one byte changed");
    assert_eq!(bytes_at(&path, 0, 512), sector0, "the protective MBR and the other system's boot code stay");
    assert_eq!(p.table.entries[4].name, "IYAKKI-BIOS");
    let (boot, data) = (&p.table.entries[5], &p.table.entries[6]);
    assert_eq!((boot.name.as_str(), boot.type_guid), ("IYAKKI-BOOT", EFI));
    assert_eq!((data.name.as_str(), data.type_guid), ("IYAKKI-DATA", gpt::TYPE_LINUX_DATA));
    assert!(boot.first_lba % 2048 == 0 && data.first_lba % 2048 == 0 && data.first_lba == boot.last_lba + 1);
    assert!(data.last_lba <= p.table.last_usable());
    // the backup header is at the very end
    assert_eq!(bytes_at(&path, 6 * GIB - 512, 8), b"EFI PART");
    // the boot volume is a real FAT32 with the image's files
    let f = std::fs::File::open(&path).unwrap();
    let mut h = f.try_clone().unwrap();
    let items = Fat32::open(&mut h, boot.first_lba * 512).unwrap().walk().unwrap();
    for want in ["/BOOT/BZIMAGE", "/BOOT/INITRD", "/BOOT/GRUB/GRUB.CFG"] {
        assert!(items.iter().any(|i| i.path == want), "{want}");
    }
    // the data partition is marked for the first boot
    assert_eq!(iyakki_storage::classify(&bytes_at(&path, data.first_lba * 512, 8192)), iyakki_storage::Content::Pending);
    // a second run sees the new partitions as existing ones and plans in the (now smaller) free space
    let m2 = model_of(&path, "sda", 6 * GIB, 512, "", true);
    assert_eq!(m2.parts.len(), 7);
    assert!(tool("sfdisk", &["--verify", path.to_str().unwrap()]).is_none_or(|(ok, out)| ok && !out.contains("overlap")));
    assert!(tool("sgdisk", &["-v", path.to_str().unwrap()]).is_none_or(|(_, out)| out.contains("No problems found")), "sgdisk");
}

#[test]
fn a_dry_run_and_a_changed_plan_write_nothing_and_a_mount_stops_the_install() {
    let (dir, _guard) = scratch("apply-guards");
    let s = src(&dir);
    let (path, _) = gpt_disk(&dir, "g.img", 4 * GIB, 512, &[ps(1, 1000, MSDATA, "d", Fill::Ntfs)]);
    let h0 = whole_file_hash(&path);
    let m = model_of(&path, "sda", 4 * GIB, 512, "", true);
    let plan = build_plan(&m, &free_req(), &spec(&s), true).unwrap();
    let r = run_apply(&path, &m, &free_req(), &s, Some(&plan.digest()), &ApplyEnv { dry_run: true, ..Default::default() }).unwrap();
    assert_eq!(r.written, 0);
    assert_eq!(whole_file_hash(&path), h0);
    // the person confirmed a different plan
    let e = run_apply(&path, &m, &free_req(), &s, Some("deadbeef"), &ApplyEnv::default()).unwrap_err();
    assert!(e.contains("digests differ"), "{e}");
    // the disk changed after the plan: a new model gives a different fingerprint
    let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.write_all_at(&[1], 3 * 512 + 56).unwrap();
    assert_eq!(whole_file_hash(&path) == h0, false);
    // a mount that appeared in /proc/mounts while the plan waited
    let (p2, _) = gpt_disk(&dir, "g2.img", 4 * GIB, 512, &[ps(1, 1000, MSDATA, "d", Fill::Ntfs)]);
    let m2 = model_of(&p2, "sda", 4 * GIB, 512, "", true);
    let req = Request { mode: Mode::Reuse, data_part: Some(1), boot_part: None, keep_esp: false, ..Request::default() };
    assert!(build_plan(&m2, &req, &spec(&s), true).unwrap_err().contains("BOOT"));
    let two = Request { mode: Mode::FreeSpace, ..Request::default() };
    let h1 = whole_file_hash(&p2);
    let plan2 = build_plan(&m2, &two, &spec(&s), true).unwrap();
    assert!(plan2.destroyed().is_empty());
    assert_eq!(whole_file_hash(&p2), h1, "planning writes nothing");
    // reuse + mount
    let (p3, _) = gpt_disk(&dir, "g3.img", 4 * GIB, 512, &[ps(1, 600, MSDATA, "b", Fill::Ntfs), ps(601, 1000, MSDATA, "d", Fill::Ext4)]);
    let m3 = model_of(&p3, "sda", 4 * GIB, 512, "", true);
    let reuse = Request { mode: Mode::Reuse, boot_part: Some(1), data_part: Some(2), ..Request::default() };
    let h3 = whole_file_hash(&p3);
    let e = run_apply(&p3, &m3, &reuse, &s, None, &ApplyEnv { proc_mounts: "/dev/sda2 /media/x ext4 rw 0 0\n".into(), ..Default::default() }).unwrap_err();
    assert!(e.contains("mounted") && e.contains("Nothing was written"), "{e}");
    assert_eq!(whole_file_hash(&p3), h3);
}

#[test]
fn reuse_install_formats_exactly_the_chosen_partitions() {
    let (dir, _guard) = scratch("apply-reuse");
    let s = src(&dir);
    let (path, old) = gpt_disk(
        &dir,
        "r.img",
        4 * GIB,
        512,
        &[ps(1, 100, EFI, "EFI", Fill::Raw(foreign_esp(&dir))), ps(101, 600, MSDATA, "old-recovery", Fill::Ntfs), ps(701, 1500, MSDATA, "spare", Fill::Ext4), ps(2201, 1000, MSDATA, "windows", Fill::Ntfs)],
    );
    let before = old_partition_bytes(&path, &old, 512);
    let sector0 = bytes_at(&path, 0, 512);
    let m = model_of(&path, "sda", 4 * GIB, 512, "", true);
    let req = Request { mode: Mode::Reuse, boot_part: Some(2), data_part: Some(3), ..Request::default() };
    let plan = build_plan(&m, &req, &spec(&s), true).unwrap();
    run_apply(&path, &m, &req, &s, Some(&plan.digest()), &ApplyEnv::default()).unwrap();
    let after = old_partition_bytes(&path, &old, 512);
    assert_eq!(after[0], before[0], "ESP untouched");
    assert_eq!(after[3], before[3], "windows untouched");
    assert_ne!(after[1], before[1]);
    assert_ne!(after[2], before[2]);
    assert_eq!(bytes_at(&path, 0, 512), sector0);
    let p = parsed(&path, 512, 4 * GIB);
    assert!(p.primary_ok && p.backup_ok);
    let e = |i: u32| p.table.entries.iter().find(|x| x.index == i).unwrap().clone();
    assert_eq!((e(2).name.as_str(), e(2).type_guid), ("IYAKKI-BOOT", EFI));
    assert_eq!((e(3).name.as_str(), e(3).type_guid), ("IYAKKI-DATA", gpt::TYPE_LINUX_DATA));
    assert_ne!(e(2).guid, old.entries[1].guid, "a fresh GUID: nothing that remembered the old one finds it");
    assert_eq!((e(2).first_lba, e(2).last_lba), (old.entries[1].first_lba, old.entries[1].last_lba), "same place and size");
    assert_eq!(e(1), old.entries[0]);
    assert_eq!(e(4), old.entries[3]);
    let mut h = std::fs::File::open(&path).unwrap();
    assert!(Fat32::open(&mut h, e(2).first_lba * 512).unwrap().walk().unwrap().iter().any(|i| i.path == "/BOOT/BZIMAGE"));
    assert_eq!(iyakki_storage::classify(&bytes_at(&path, e(3).first_lba * 512, 8192)), iyakki_storage::Content::Pending);
    assert_eq!(bytes_at(&path, e(3).last_lba * 512 + 512 - 4096, 4096), vec![0u8; 4096], "the old filesystem's tail is wiped");
}

#[test]
fn an_interrupted_install_leaves_the_old_table_and_the_old_partitions_intact() {
    let (dir, _guard) = scratch("apply-crash");
    let s = src(&dir);
    let (path, old) = gpt_disk(&dir, "c.img", 4 * GIB, 512, &[ps(1, 100, EFI, "EFI", Fill::Raw(foreign_esp(&dir))), ps(101, 1000, MSDATA, "windows", Fill::Ntfs)]);
    let first = 17 * 512;
    let region = |p: &Path| (bytes_at(p, 0, 512), bytes_at(p, 512, 512), bytes_at(p, 1024, 16384), bytes_at(p, 4 * GIB - 17 * 512, 17 * 512));
    let tables = region(&path);
    let parts = old_partition_bytes(&path, &old, 512);
    let m = model_of(&path, "sda", 4 * GIB, 512, "", true);
    let _ = first;
    // the writer is stopped (a panic stands in for the power going) at the first progress report of each phase
    for stop_after in [1u32, 3, 6] {
        let mut n = 0u32;
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            apply::apply(&path, &m, &free_req(), &s, None, true, &ApplyEnv::default(), &mut |_| {
                n += 1;
                if n == stop_after {
                    panic!("power loss");
                }
            })
        }));
        assert!(r.is_err());
        assert_eq!(region(&path), tables, "stopped after report {stop_after}: the tables are still the old ones");
        assert_eq!(old_partition_bytes(&path, &old, 512), parts, "stopped after report {stop_after}: old partitions are intact");
        let again = model_of(&path, "sda", 4 * GIB, 512, "", true);
        assert_eq!(again.fingerprint, m.fingerprint);
        assert_eq!(again.parts.len(), 2);
    }
}

#[test]
fn mbr_free_space_install_keeps_boot_code_and_existing_entries() {
    let (dir, _guard) = scratch("apply-mbr");
    let s = src(&dir);
    let path = mbr_disk(&dir, "m.img", 4 * GIB, &[(1, 1000, 0x07, Fill::Ntfs), (1001, 500, 0x83, Fill::Ext4)]);
    let s0 = bytes_at(&path, 0, 512);
    let m = model_of(&path, "sda", 4 * GIB, 512, "", true);
    assert_eq!(m.table, Some(TableKind::Mbr));
    let plan = build_plan(&m, &free_req(), &spec(&s), true).unwrap();
    assert_eq!(plan.steps.iter().filter(|st| st.action == Action::Create).count(), 2);
    run_apply(&path, &m, &free_req(), &s, Some(&plan.digest()), &ApplyEnv::default()).unwrap();
    let n0 = bytes_at(&path, 0, 512);
    assert_eq!(n0[..446], s0[..446], "boot code and signature: unchanged");
    assert_eq!(n0[446..478], s0[446..478], "the two existing entries: unchanged");
    assert_eq!((n0[478 + 4], n0[494 + 4]), (0xEF, 0x83));
    assert_eq!(&n0[510..], &[0x55, 0xAA]);
    let start = u32::from_le_bytes(n0[478 + 8..478 + 12].try_into().unwrap()) as u64;
    assert_eq!(start % 2048, 0);
    assert!(start >= 1501 * 2048);
    let mut h = std::fs::File::open(&path).unwrap();
    assert!(Fat32::open(&mut h, start * 512).unwrap().walk().is_ok());
    // a new run sees four partitions and refuses to add more
    let m2 = model_of(&path, "sda", 4 * GIB, 512, "", true);
    assert_eq!(m2.parts.len(), 4);
    assert!(build_plan(&m2, &free_req(), &spec(&s), true).is_err(), "no free space and no free slot is left");
    assert!(tool("sfdisk", &["--verify", path.to_str().unwrap()]).is_none_or(|(ok, _)| ok));
}

#[test]
fn a_4096_byte_sector_disk_gets_a_rebuilt_boot_volume_in_free_space() {
    let (dir, _guard) = scratch("apply-4k");
    let s = src(&dir);
    let (path, old) = gpt_disk(&dir, "k.img", 3 * GIB, 4096, &[ps(1, 1024, MSDATA, "other", Fill::Ntfs)]);
    let before = old_partition_bytes(&path, &old, 4096);
    let m = model_of(&path, "nvme1n1", 3 * GIB, 4096, "", false);
    let plan = build_plan(&m, &free_req(), &spec(&s), true).unwrap();
    let boot = plan.steps.iter().find(|s| s.role == Some(Role::Boot)).unwrap();
    assert_eq!(boot.bytes, 320 * MIB, "4096-byte sectors: the boot volume is rebuilt at 320 MiB");
    assert!(plan.internal);
    run_apply(&path, &m, &free_req(), &s, Some(&plan.digest()), &ApplyEnv::default()).unwrap();
    assert_eq!(old_partition_bytes(&path, &old, 4096), before);
    let p = parsed(&path, 4096, 3 * GIB);
    assert!(p.primary_ok && p.backup_ok);
    assert_eq!(p.table.entries.len(), 4);
    let b = p.table.entries.iter().find(|e| e.name == "IYAKKI-BOOT").unwrap();
    let mut h = std::fs::File::open(&path).unwrap();
    assert!(Fat32::open(&mut h, b.first_lba * 4096).unwrap().walk().unwrap().iter().any(|i| i.path == "/BOOT/INITRD"));
    assert_eq!(bytes_at(&path, 3 * GIB - 4096, 8), b"EFI PART");
}

#[test]
fn keep_esp_adds_only_our_folder_and_our_boot_entry() {
    let (dir, _guard) = scratch("apply-keep");
    let s = src(&dir);
    let (path, old) = gpt_disk(&dir, "k.img", 4 * GIB, 512, &[ps(1, 100, EFI, "EFI system partition", Fill::Raw(foreign_esp(&dir))), ps(101, 1000, MSDATA, "windows", Fill::Ntfs)]);
    let before = old_partition_bytes(&path, &old, 512);
    let esp_before: Vec<_> = {
        let mut h = std::fs::File::open(&path).unwrap();
        Fat32::open(&mut h, MIB).unwrap().walk().unwrap()
    };
    let efivars = dir.join("efivars");
    std::fs::create_dir_all(&efivars).unwrap();
    let guid = efi_guid();
    let w = |n: &str, v: &[u8]| std::fs::write(efivars.join(format!("{n}-{guid}")), v).unwrap();
    w("Boot0000", &[7, 0, 0, 0, 1, 0, 0, 0, 0, 0, b'W', 0, 0, 0]);
    w("BootOrder", &[7, 0, 0, 0, 0, 0]);
    let m = model_of(&path, "sda", 4 * GIB, 512, "", false);
    let req = Request { keep_esp: true, ..free_req() };
    let plan = build_plan(&m, &req, &spec(&s), true).unwrap();
    assert!(plan.internal && plan.keep_esp);
    let env = ApplyEnv { efivars: Some(efivars.clone()), ..Default::default() };
    let rep = run_apply(&path, &m, &req, &s, Some(&plan.digest()), &env).unwrap();
    assert!(rep.boot_entry.as_deref() == Some("Boot0001"), "{rep:?}");
    // the other system's files: same entries, same bytes
    let mut h = std::fs::File::open(&path).unwrap();
    let mut vol = Fat32::open(&mut h, MIB).unwrap();
    let after = vol.walk().unwrap();
    for it in &esp_before {
        assert!(after.contains(it), "{} is unchanged", it.path);
    }
    for it in after.iter().filter(|i| !esp_before.contains(i)) {
        assert!(it.path.starts_with("/EFI/IYAKKI"), "only our folder is new: {}", it.path);
    }
    let kernel = after.iter().find(|i| i.path == "/EFI/IYAKKI/KERNEL.EFI").unwrap().clone();
    assert_eq!(vol.read_file(&kernel).unwrap(), pseudo(700_001, 3), "the kernel arrived intact");
    let initrd = after.iter().find(|i| i.path == "/EFI/IYAKKI/INITRD").unwrap().clone();
    assert_eq!(vol.read_file(&initrd).unwrap(), pseudo(1_500_000, 4));
    let loader = esp_before.iter().find(|i| i.path.ends_with("BOOTMGFW.EFI")).unwrap().clone();
    assert_eq!(vol.read_file(&loader).unwrap(), pseudo(300_000, 9));
    // only the data partition is new, and no old partition other than the ESP's FAT changed
    let now = old_partition_bytes(&path, &old, 512);
    assert_eq!(now[1], before[1], "windows partition untouched");
    let p = parsed(&path, 512, 4 * GIB);
    assert_eq!(p.table.entries.len(), 3);
    assert_eq!(p.table.entries[0], old.entries[0], "the ESP's table entry is unchanged");
    // the firmware: ours appended, the other entry untouched
    assert_eq!(std::fs::read(efivars.join(format!("Boot0000-{guid}"))).unwrap(), [7, 0, 0, 0, 1, 0, 0, 0, 0, 0, b'W', 0, 0, 0]);
    assert_eq!(std::fs::read(efivars.join(format!("BootOrder-{guid}"))).unwrap(), [7, 0, 0, 0, 0, 0, 1, 0]);
    let entry = std::fs::read(efivars.join(format!("Boot0001-{guid}"))).unwrap();
    assert!(entry.windows(2).any(|w| w == b"K\0"), "the kernel stub path is in the entry");
    // the volume is clean for the real checker
    let esp_img = dir.join("esp-after.img");
    std::fs::write(&esp_img, bytes_at(&path, MIB, (40 * MIB) as usize)).unwrap();
    if let Some((ok, out)) = tool("fsck.vfat", &["-n", esp_img.to_str().unwrap()]) {
        assert!(ok, "fsck.vfat:\n{out}");
    }
    // a second install finds our folder and stops without changing anything
    let m2 = model_of(&path, "sda", 4 * GIB, 512, "", false);
    let h = whole_file_hash(&path);
    let e = run_apply(&path, &m2, &Request { keep_esp: true, ..free_req() }, &s, None, &env);
    // (the new partitions leave too little free space for a second data partition, or the folder exists: either stops it)
    assert!(e.is_err(), "{e:?}");
    assert_eq!(whole_file_hash(&path), h, "nothing changed");
}

#[test]
fn an_aarch64_image_installs_beside_another_system_and_on_a_whole_disk_with_its_own_boot_files() {
    let (dir, _guard) = scratch("apply-arm");
    let s = src_arm(&dir);
    assert_eq!(crate::imagearch::boot_files(["/Image", "/rootfs.cpio.gz"]).unwrap().arch, crate::imagearch::Arch::Aarch64);
    // the long-named RAM disk counts as payload when the space is worked out (a reader of short aliases would miss it)
    assert_eq!(spec(&s).payload_bytes, 900_001 + 1_200_000);
    // beside another system: our folder on its ESP holds the aarch64 kernel stub and RAM disk, nothing else changes
    let (path, old) = gpt_disk(&dir, "ka.img", 4 * GIB, 512, &[ps(1, 100, EFI, "EFI system partition", Fill::Raw(foreign_esp(&dir))), ps(101, 1000, MSDATA, "windows", Fill::Ntfs)]);
    let before = old_partition_bytes(&path, &old, 512);
    let esp_before: Vec<_> = {
        let mut h = std::fs::File::open(&path).unwrap();
        Fat32::open(&mut h, MIB).unwrap().walk().unwrap()
    };
    let efivars = dir.join("efivars");
    std::fs::create_dir_all(&efivars).unwrap();
    let m = model_of(&path, "sda", 4 * GIB, 512, "", false);
    let req = Request { keep_esp: true, ..free_req() };
    let plan = build_plan(&m, &req, &spec(&s), true).unwrap();
    assert!(plan.internal && plan.keep_esp);
    let env = ApplyEnv { efivars: Some(efivars), ..Default::default() };
    run_apply(&path, &m, &req, &s, Some(&plan.digest()), &env).unwrap();
    let mut h = std::fs::File::open(&path).unwrap();
    let mut vol = Fat32::open(&mut h, MIB).unwrap();
    let after = vol.walk().unwrap();
    for it in &esp_before {
        assert!(after.contains(it), "{} is unchanged", it.path);
    }
    let kernel = after.iter().find(|i| i.path == "/EFI/IYAKKI/KERNEL.EFI").unwrap().clone();
    assert_eq!(vol.read_file(&kernel).unwrap(), pseudo(900_001, 5), "the aarch64 kernel stub arrived intact");
    let initrd = after.iter().find(|i| i.path == "/EFI/IYAKKI/INITRD").unwrap().clone();
    assert_eq!(vol.read_file(&initrd).unwrap(), pseudo(1_200_000, 6));
    assert_eq!(old_partition_bytes(&path, &old, 512)[1], before[1], "the other system's partition is untouched");
    // a whole disk: the boot volume of the new disk keeps both files under their long names
    let (path, _) = gpt_disk(&dir, "wa.img", 5 * GIB, 512, &[ps(1, 1000, MSDATA, "old", Fill::Ntfs)]);
    let m = model_of(&path, "sda", 5 * GIB, 512, "", true);
    let req = Request { mode: Mode::WholeDisk, table: TableChoice::Gpt, ..Request::default() };
    let plan = build_plan(&m, &req, &spec(&s), true).unwrap();
    let rep = run_apply(&path, &m, &req, &s, Some(&plan.digest()), &ApplyEnv::default()).unwrap();
    assert!(rep.verified);
    let p = parsed(&path, 512, 5 * GIB);
    let b = p.table.entries.iter().find(|e| e.name == "IYAKKI-BOOT").unwrap();
    let mut h = std::fs::File::open(&path).unwrap();
    let mut vol = Fat32::open(&mut h, b.first_lba * 512).unwrap();
    let items = vol.walk_long().unwrap();
    let img = items.iter().find(|i| i.path == "/Image").expect("the kernel stub keeps its name").clone();
    assert_eq!(vol.read_file(&img).unwrap(), pseudo(900_001, 5));
    let rd = items.iter().find(|i| i.path == "/rootfs.cpio.gz").expect("the RAM disk keeps its long name").clone();
    assert_eq!(vol.read_file(&rd).unwrap(), pseudo(1_200_000, 6));
}

#[test]
fn an_aarch64_image_in_free_space_gets_a_firmware_entry_for_its_own_loader() {
    let (dir, _guard) = scratch("apply-arm-entry");
    let s = src_arm(&dir);
    let (path, _) = gpt_disk(&dir, "ke.img", 4 * GIB, 512, &[ps(1, 1000, MSDATA, "windows", Fill::Ntfs)]);
    let efivars = dir.join("efivars");
    std::fs::create_dir_all(&efivars).unwrap();
    let m = model_of(&path, "sda", 4 * GIB, 512, "", false);
    let req = free_req();
    let plan = build_plan(&m, &req, &spec(&s), true).unwrap();
    assert!(!plan.keep_esp);
    let env = ApplyEnv { efivars: Some(efivars.clone()), ..Default::default() };
    let rep = run_apply(&path, &m, &req, &s, Some(&plan.digest()), &env).unwrap();
    let name = rep.boot_entry.clone().unwrap_or_else(|| panic!("a boot entry was added: {:?}", rep.notes));
    let entry = std::fs::read(efivars.join(format!("{name}-{}", efi_guid()))).unwrap();
    let utf16 = |s: &str| s.encode_utf16().flat_map(|c| c.to_le_bytes()).collect::<Vec<u8>>();
    let has = |needle: &[u8]| entry.windows(needle.len()).any(|w| w == needle);
    assert!(has(&utf16("\\EFI\\BOOT\\BOOTAA64.EFI")), "the entry names the aarch64 loader");
    assert!(!has(&utf16("BOOTX64")), "and not the x86 one");
}

fn efi_guid() -> &'static str {
    crate::efi::GLOBAL_GUID
}

#[test]
fn whole_disk_mode_still_makes_the_known_layout() {
    let (dir, _guard) = scratch("apply-whole");
    let s = src(&dir);
    let (path, _) = gpt_disk(&dir, "w.img", 5 * GIB, 512, &[ps(1, 1000, MSDATA, "old", Fill::Ntfs), ps(1001, 500, MSDATA, "old2", Fill::Ext4)]);
    let m = model_of(&path, "sda", 5 * GIB, 512, "", true);
    let req = Request { mode: Mode::WholeDisk, table: TableChoice::Gpt, ..Request::default() };
    let plan = build_plan(&m, &req, &spec(&s), true).unwrap();
    assert_eq!(plan.destroyed(), ["sda1", "sda2"]);
    assert_eq!(plan.steps.iter().filter(|s| s.action == Action::Create).count(), 3);
    assert!(plan.new_table && plan.table_after == TableKind::Gpt);
    let rep = run_apply(&path, &m, &req, &s, Some(&plan.digest()), &ApplyEnv::default()).unwrap();
    assert!(rep.verified);
    let p = parsed(&path, 512, 5 * GIB);
    assert_eq!(p.table.entries.len(), 3);
    assert_eq!(p.table.entries[1].name, "IYAKKI-BOOT");
    let mbr = Request { table: TableChoice::Mbr, ..req };
    let pm = build_plan(&m, &mbr, &spec(&s), true).unwrap();
    assert_eq!(pm.table_after, TableKind::Mbr);
    assert_eq!(pm.steps.iter().filter(|s| s.action == Action::Create).count(), 2);
}

#[test]
fn a_blank_disk_in_free_space_mode_gets_a_fresh_table() {
    let (dir, _guard) = scratch("apply-blank");
    let s = src(&dir);
    let path = dir.join("blank.img");
    std::fs::File::create(&path).unwrap().set_len(2 * GIB).unwrap();
    let m = model_of(&path, "sda", 2 * GIB, 512, "", true);
    assert!(m.table.is_none() && !m.raw_data);
    let req = Request { table: TableChoice::Gpt, ..free_req() };
    let plan = build_plan(&m, &req, &spec(&s), true).unwrap();
    assert!(plan.new_table);
    run_apply(&path, &m, &req, &s, Some(&plan.digest()), &ApplyEnv::default()).unwrap();
    let p = parsed(&path, 512, 2 * GIB);
    assert!(p.primary_ok && p.backup_ok);
    assert_eq!(p.table.entries.len(), 3, "BIOS boot, boot and data");
    assert!(tool("sgdisk", &["-v", path.to_str().unwrap()]).is_none_or(|(_, out)| out.contains("No problems found")));
}

// ---------------------------------------------------------------------------------------------------------------------------
// the FAT writer

#[test]
fn the_fat_writer_extends_a_full_directory_matches_long_names_and_refuses_to_overwrite() {
    let (dir, _guard) = scratch("fatw");
    let img = foreign_esp(&dir);
    let p = dir.join("v.img");
    std::fs::write(&p, &img).unwrap();
    let f = std::fs::OpenOptions::new().read(true).write(true).open(&p).unwrap();
    let mut rw = Fat32Rw::open(&f, 0, img.len() as u64).unwrap();
    let free0 = rw.free_bytes();
    let (d, existed) = rw.mkdir_p(&["EFI", "BOOT"]).unwrap();
    assert!(existed, "an existing path is reused, case-insensitively");
    let (d2, existed2) = rw.mkdir_p(&["efi", "boot"]).unwrap();
    assert_eq!((d, existed2), (d2, true));
    // fill a directory well past one cluster (512 bytes = 16 entries) so that it must grow
    let (many, _) = rw.mkdir_p(&["EFI", "MANY"]).unwrap();
    for i in 0..40 {
        rw.create_file(many, &format!("F{i:03}.BIN"), 10 + i as u64, &mut std::io::Cursor::new(vec![i as u8; 10 + i]), &mut |_| {}).unwrap();
    }
    assert!(rw.create_file(many, "F005.BIN", 1, &mut std::io::Cursor::new(vec![1]), &mut |_| {}).unwrap_err().contains("already exists"));
    assert!(rw.create_file(many, "bad name", 1, &mut std::io::Cursor::new(vec![1]), &mut |_| {}).is_err());
    rw.flush().unwrap();
    rw.write_fsinfo().unwrap();
    assert!(rw.free_bytes() < free0);
    drop(rw);
    let mut h = std::fs::File::open(&p).unwrap();
    let mut vol = Fat32::open(&mut h, 0).unwrap();
    let items = vol.walk().unwrap();
    let bins = items.iter().filter(|i| i.path.starts_with("/EFI/MANY/F")).count();
    assert_eq!(bins, 40);
    let f17 = items.iter().find(|i| i.path == "/EFI/MANY/F017.BIN").unwrap().clone();
    assert_eq!(vol.read_file(&f17).unwrap(), vec![17u8; 27]);
    if let Some((ok, out)) = tool("fsck.vfat", &["-n", p.to_str().unwrap()]) {
        assert!(ok, "fsck.vfat:\n{out}");
    }
    assert!(crate::fatwrite::short_name("KERNEL.EFI").is_ok());
    assert!(crate::fatwrite::short_name("kernel.efi").is_err() && crate::fatwrite::short_name("TOOLONGNAME.EFI").is_err());
}

#[test]
fn the_fat_writer_creates_long_names_with_short_aliases_and_the_reader_and_fsck_agree() {
    let (dir, _guard) = scratch("fatlfn");
    let img = foreign_esp(&dir);
    let p = dir.join("v.img");
    std::fs::write(&p, &img).unwrap();
    let f = std::fs::OpenOptions::new().read(true).write(true).open(&p).unwrap();
    let mut rw = Fat32Rw::open(&f, 0, img.len() as u64).unwrap();
    let (d, _) = rw.mkdir_p(&["EFI", "Long Dir".replace(' ', "-").as_str()]).unwrap();
    let names = ["rootfs.cpio.gz", "rootfs.cpio.xz", "Image", "vmlinuz-6.12.112", "rootfs.cpio.gz.sig", "a.b.c.d", "UPPER.LONGEXT"];
    for (i, n) in names.iter().enumerate() {
        rw.create_file(d, n, 20 + i as u64, &mut std::io::Cursor::new(vec![i as u8 + 1; 20 + i]), &mut |_| {}).unwrap();
    }
    // lookups are case-insensitive on the long name, and a second file of the same name is refused
    assert!(rw.find(d, "ROOTFS.CPIO.GZ").unwrap().is_some() && rw.find(d, "image").unwrap().is_some());
    assert!(rw.create_file(d, "Rootfs.Cpio.Gz", 1, &mut std::io::Cursor::new(vec![1]), &mut |_| {}).unwrap_err().contains("already exists"));
    // names that are not creatable
    for bad in ["two words", ".hidden", "trailing.", "star*name", "a/b", ""] {
        assert!(!crate::fatwrite::can_create(bad), "{bad}");
        assert!(rw.create_file(d, bad, 1, &mut std::io::Cursor::new(vec![1]), &mut |_| {}).is_err(), "{bad}");
    }
    assert!(crate::fatwrite::can_create("rootfs.cpio.gz") && crate::fatwrite::can_create("KERNEL.EFI"));
    rw.flush().unwrap();
    rw.write_fsinfo().unwrap();
    drop(rw);
    let mut h = std::fs::File::open(&p).unwrap();
    let mut vol = Fat32::open(&mut h, 0).unwrap();
    let items = vol.walk_long().unwrap();
    for (i, n) in names.iter().enumerate() {
        let it = items.iter().find(|x| x.path == format!("/EFI/LONG-DIR/{n}")).unwrap_or_else(|| panic!("{n} missing: {:?}", items.iter().map(|x| x.path.clone()).collect::<Vec<_>>())).clone();
        assert_eq!(vol.read_file(&it).unwrap(), vec![i as u8 + 1; 20 + i], "{n}");
    }
    // the short aliases are distinct and well formed: aliases differ in the extension, so each gets ~1
    let shorts: Vec<String> = vol.walk().unwrap().into_iter().filter(|x| x.path.starts_with("/EFI/LONG-DIR/") && !x.dir).map(|x| x.path.rsplit('/').next().unwrap().to_string()).collect();
    assert_eq!(shorts.len(), names.len());
    assert!(shorts.contains(&"ROOTFS~1.GZ".to_string()) && shorts.contains(&"ROOTFS~1.XZ".to_string()) && shorts.contains(&"IMAGE".to_string()), "{shorts:?}");
    assert!(shorts.iter().collect::<std::collections::BTreeSet<_>>().len() == shorts.len());
    if let Some((ok, out)) = tool("fsck.vfat", &["-n", p.to_str().unwrap()]) {
        assert!(ok, "fsck.vfat:\n{out}");
    }
}

#[test]
fn long_name_entries_carry_the_checksum_of_their_alias_and_a_directory_grows_for_them() {
    let (dir, _guard) = scratch("fatlfn2");
    let img = foreign_esp(&dir);
    let p = dir.join("v.img");
    std::fs::write(&p, &img).unwrap();
    let f = std::fs::OpenOptions::new().read(true).write(true).open(&p).unwrap();
    let mut rw = Fat32Rw::open(&f, 0, img.len() as u64).unwrap();
    let (d, _) = rw.mkdir_p(&["EFI", "MANY2"]).unwrap();
    // 30 names of 2 long-name entries each in a directory that starts with one cluster (16 slots): the directory must grow and no
    // name may be split over two clusters
    for i in 0..30 {
        rw.create_file(d, &format!("long-file-name-{i:03}.bin"), 1, &mut std::io::Cursor::new(vec![i as u8]), &mut |_| {}).unwrap();
    }
    rw.flush().unwrap();
    rw.write_fsinfo().unwrap();
    drop(rw);
    let mut h = std::fs::File::open(&p).unwrap();
    let mut vol = Fat32::open(&mut h, 0).unwrap();
    let items = vol.walk_long().unwrap();
    assert_eq!(items.iter().filter(|x| x.path.starts_with("/EFI/MANY2/long-file-name-")).count(), 30, "every long name is read back whole (the reader refuses a wrong checksum or order)");
    if let Some((ok, out)) = tool("fsck.vfat", &["-n", p.to_str().unwrap()]) {
        assert!(ok, "fsck.vfat:\n{out}");
    }
}

#[test]
fn replacing_a_file_keeps_the_old_one_until_the_new_is_whole_frees_its_clusters_and_leaves_the_rest_alone() {
    let (dir, _guard) = scratch("fatrep");
    let img = foreign_esp(&dir);
    let p = dir.join("v.img");
    std::fs::write(&p, &img).unwrap();
    let f = std::fs::OpenOptions::new().read(true).write(true).open(&p).unwrap();
    let mut rw = Fat32Rw::open(&f, 0, img.len() as u64).unwrap();
    let (d, _) = rw.mkdir_p(&["IYAKKI"]).unwrap();
    let cb = rw.cluster_bytes() as usize;
    rw.create_file(d, "KERNEL.BIN", (3 * cb) as u64, &mut std::io::Cursor::new(vec![1u8; 3 * cb]), &mut |_| {}).unwrap();
    rw.create_file(d, "KEEP.TXT", 5, &mut std::io::Cursor::new(b"keep!".to_vec()), &mut |_| {}).unwrap();
    rw.flush().unwrap();
    let free_before = rw.free_bytes();
    // a bigger file, then a smaller one, then an empty one: the free space follows
    rw.replace_file(d, "KERNEL.BIN", (5 * cb + 7) as u64, &mut std::io::Cursor::new(vec![2u8; 5 * cb + 7]), &mut |_| {}).unwrap();
    assert_eq!(rw.free_bytes(), free_before - 3 * cb as u64, "6 clusters now, 3 before");
    rw.replace_file(d, "kernel.bin", 10, &mut std::io::Cursor::new(vec![3u8; 10]), &mut |_| {}).unwrap();
    assert_eq!(rw.free_bytes(), free_before + 2 * cb as u64, "one cluster now");
    // refusals: missing name, a folder, a short source (the old file stays whole)
    assert!(rw.replace_file(d, "NOPE.BIN", 1, &mut std::io::Cursor::new(vec![0]), &mut |_| {}).unwrap_err().contains("nothing to replace"));
    assert!(rw.replace_file(rw.root_cluster(), "IYAKKI", 1, &mut std::io::Cursor::new(vec![0]), &mut |_| {}).unwrap_err().contains("folder"));
    assert!(rw.replace_file(d, "KERNEL.BIN", 100, &mut std::io::Cursor::new(vec![0u8; 3]), &mut |_| {}).is_err());
    assert_eq!(rw.free_bytes(), free_before + 2 * cb as u64, "the failed attempt gave its clusters back");
    rw.flush().unwrap();
    rw.write_fsinfo().unwrap();
    drop(rw);
    let mut h = std::fs::File::open(&p).unwrap();
    let mut vol = Fat32::open(&mut h, 0).unwrap();
    let items = vol.walk().unwrap();
    let k = items.iter().find(|i| i.path == "/IYAKKI/KERNEL.BIN").unwrap().clone();
    assert_eq!(vol.read_file(&k).unwrap(), vec![3u8; 10], "the failed attempt left the last good contents");
    let keep = items.iter().find(|i| i.path == "/IYAKKI/KEEP.TXT").unwrap().clone();
    assert_eq!(vol.read_file(&keep).unwrap(), b"keep!");
    assert!(items.iter().any(|i| i.path.starts_with("/EFI")), "the other system's files are still there");
    if let Some((ok, out)) = tool("fsck.vfat", &["-n", p.to_str().unwrap()]) {
        assert!(ok, "fsck.vfat:\n{out}");
    }
}

#[test]
fn the_writer_never_opens_a_block_device_in_tests() {
    let e = apply::apply(Path::new("/dev/null"), &DiskModel { name: "x".into(), path: "/dev/null".into(), model: String::new(), serial: String::new(), size_bytes: 0, sector: 512, table: None, notes: vec![], parts: vec![], removable: false, usb: false, read_only: false, running_system: false, booted_from: false, raw_data: false, unusual_gpt: false, disk_guid: None, bios: Local(None), fingerprint: String::new() }, &free_req(), &src(&tmp("nodev")), None, true, &ApplyEnv::default(), &mut |_| {});
    assert!(e.is_err());
}

// ---------------------------------------------------------------------------------------------------------------------------
// the pages, the plan file, a GPT stick as the source

use crate::wizard::{CFocus, Next, Page, WKey, Wizard};

fn type_str(w: &mut Wizard, s: &str) {
    for c in s.chars() {
        w.handle(WKey::Char(c));
    }
}

#[test]
fn the_pages_walk_from_drive_to_a_confirmed_write_and_nothing_earlier_writes() {
    let (dir, _guard) = scratch("wiz");
    let s = src(&dir);
    let (path, _) = windows_disk(&dir, 40 * GIB, 512);
    let h0 = whole_file_hash(&path);
    let internal = model_of(&path, "sda", 40 * GIB, 512, "", false);
    let mut stick = internal.clone();
    stick.name = "sdb".into();
    stick.path = "/dev/sdb".into();
    stick.booted_from = true;
    let mut w = Wizard::new(vec![stick, internal], spec(&s), true);
    assert_eq!(w.disk_sel, 1, "the page starts on the first drive that may be used");
    assert!(w.render(80).contains("not usable: the stick IYAKKI started from"));
    // the stick cannot be chosen
    w.disk_sel = 0;
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Drive);
    assert!(w.error.as_deref().unwrap().contains("cannot be used"));
    w.disk_sel = 1;
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Mode);
    let text = w.render(80);
    for m in ["Whole disk", "Free space only", "Use existing partitions", "Custom sizes"] {
        assert!(text.contains(m), "{m}");
    }
    // free space (the default selection)
    assert_eq!(w.mode(), Mode::FreeSpace);
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Choose);
    assert!(w.render(80).contains("free space at"));
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Plan);
    let plan_text = w.render(80);
    assert!(plan_text.contains("create") && plan_text.contains("keep") && plan_text.contains("nothing is written yet"), "{plan_text}");
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Confirm);
    let screen = w.render(80);
    assert!(screen.contains("Will be created") && screen.contains("Will be KEPT") && screen.contains("Will be ERASED") && screen.contains("[ Install ]"), "{screen}");
    assert!(!screen.contains("Type the drive name"), "no typing any more");
    assert!(!screen.contains("I understand"), "a plan that erases nothing has no box");
    assert_eq!(w.cfocus, CFocus::Install, "nothing is erased: Enter alone installs");
    assert_eq!(whole_file_hash(&path), h0, "the pages never write");
    // Esc goes back, Enter again, Cancel quits without writing
    w.handle(WKey::Esc);
    assert_eq!(w.page, Page::Plan);
    w.handle(WKey::Enter);
    w.handle(WKey::Tab);
    w.handle(WKey::Tab);
    assert_eq!(w.cfocus, CFocus::Cancel);
    assert_eq!(w.handle(WKey::Enter), Next::Quit);
    assert_eq!(whole_file_hash(&path), h0);
    w.cfocus = CFocus::Install;
    // one press of Install writes, and the proof is the button form for this drive
    assert_eq!(w.confirmation(), Some(Confirmation::Button { drive: "sda".into(), size: w.plan.as_ref().unwrap().disk_size, erase_ack: false }));
    assert_eq!(w.handle(WKey::Enter), Next::Write);
    assert_eq!(w.page, Page::Progress);
    w.finished(Ok("done".into()));
    assert_eq!(w.page, Page::Done);
    assert_eq!(w.handle(WKey::Enter), Next::Quit);
}

#[test]
fn the_pages_explain_unavailable_modes_and_let_reuse_pick_boot_and_data() {
    let (dir, _guard) = scratch("wiz2");
    let s = src(&dir);
    let (path, _) = gpt_disk(&dir, "r.img", 4 * GIB, 512, &[ps(1, 600, MSDATA, "a", Fill::Ntfs), ps(601, 1000, MSDATA, "b", Fill::Ext4), ps(1601, 2400, MSDATA, "c", Fill::Ntfs)]);
    let m = model_of(&path, "sda", 4 * GIB, 512, "", true);
    let mut w = Wizard::new(vec![m], spec(&s), true);
    w.handle(WKey::Enter);
    // no free space on this disk, but a partition can be deleted to make some: the mode stays available
    assert!(w.mode_available(Mode::FreeSpace).is_ok());
    assert!(w.mode_available(Mode::Reuse).is_ok());
    assert!(!w.render(80).contains("unavailable"));
    w.handle(WKey::Enter); // free space on a full disk: the page where a partition can be deleted first
    assert_eq!(w.page, Page::Choose);
    assert!(w.render(100).contains("x = delete the partition"));
    w.handle(WKey::Enter); // there is no free space row to choose
    assert_eq!(w.page, Page::Choose);
    assert!(w.error.is_some());
    w.handle(WKey::Esc);
    assert_eq!(w.page, Page::Mode);
    w.mode_sel = 2;
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Choose);
    w.row_sel = 0;
    w.handle(WKey::Char('b'));
    w.row_sel = 1;
    w.handle(WKey::Char('d'));
    assert_eq!((w.req.boot_part, w.req.data_part), (Some(1), Some(2)));
    let t = w.render(80);
    assert!(t.contains("[BOOT]") && t.contains("[DATA]"));
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Plan);
    assert_eq!(w.plan.as_ref().unwrap().destroyed(), ["sda1", "sda2"]);
    w.handle(WKey::Enter);
    let screen = w.render(80);
    assert!(screen.contains("Will be ERASED") && screen.contains("sda1") && screen.contains("sda2"), "{screen}");
}

#[test]
fn custom_sizes_are_typed_in_mib() {
    let (dir, _guard) = scratch("wiz3");
    let s = src(&dir);
    let (path, _) = gpt_disk(&dir, "c.img", 4 * GIB, 512, &[ps(1, 500, MSDATA, "a", Fill::Ntfs)]);
    let m = model_of(&path, "sda", 4 * GIB, 512, "", true);
    let mut w = Wizard::new(vec![m], spec(&s), true);
    w.handle(WKey::Enter);
    w.mode_sel = 3;
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Choose);
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Sizes);
    w.sizes = ["".into(), "".into()];
    w.focus = 0;
    type_str(&mut w, "64");
    // typing one size recalculates the other: together they fill the free space
    let room = w.size_room_mib;
    assert!(room > 3000 && w.sizes[1].parse::<u64>().unwrap() == room - 64, "{:?} of {room}", w.sizes);
    w.focus = 1;
    w.handle(WKey::Backspace);
    let data: u64 = w.sizes[1].parse().unwrap();
    assert_eq!(w.sizes[0].parse::<u64>().unwrap(), room - data, "the boot size follows the data size");
    w.sizes = ["64".into(), "1500".into()];
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Plan, "{:?}", w.error);
    let p = w.plan.as_ref().unwrap();
    let c: Vec<u64> = p.steps.iter().filter(|s| s.action == Action::Create).map(|s| s.bytes).collect();
    assert_eq!(c, [MIB, 64 * MIB, 1500 * MIB]);
}

#[test]
fn the_disk_list_round_trips_as_json_for_the_window_installer() {
    let (dir, _guard) = scratch("json");
    let s = src(&dir);
    let (path, _) = windows_disk(&dir, 40 * GIB, 512);
    let m = model_of(&path, "sda", 40 * GIB, 512, "/dev/sda3 /media/u/OS ntfs rw 0 0\n", false);
    let back = DiskModel::from_json(&crate::json::Json::parse(&m.to_json().pretty()).unwrap()).unwrap();
    assert_eq!(back, m);
    let sp = spec(&s);
    assert_eq!(SourceSpec::from_json(&crate::json::Json::parse(&sp.to_json().pretty()).unwrap()).unwrap(), sp);
    // the window plans on the copy and gets the same plan
    assert_eq!(build_plan(&back, &free_req(), &sp, true).unwrap().digest(), build_plan(&m, &free_req(), &sp, true).unwrap().digest());
}

#[test]
fn a_plan_file_is_written_by_a_dry_run_and_applied_only_with_the_typed_confirmation() {
    use crate::plan_cli::{run_inner, write_plan_file, Machine};
    let (dir, _guard) = scratch("planfile");
    let s = src(&dir);
    let (path, _) = gpt_disk(&dir, "p.img", 3 * GIB, 512, &[ps(1, 500, MSDATA, "other", Fill::Ntfs)]);
    let machine = Machine { sys: "/nonexistent".into(), dev: "/nonexistent".into(), proc_mounts: String::new(), swaps: String::new() };
    let a = |v: &[&str]| -> Vec<String> { v.iter().map(|x| x.to_string()).collect() };
    let (p, t, pf) = (path.display().to_string(), s.path.display().to_string(), dir.join("plan.json").display().to_string());
    let h0 = whole_file_hash(&path);
    // --list --json
    assert_eq!(run_inner(&a(&["--plan", "--list", "--json", "--target-file", &p, &t]), &machine, false).unwrap(), 0);
    // dry run with a plan file: nothing written
    assert_eq!(run_inner(&a(&["--plan", "--disk", "testfile", "--mode", "free-space", "--dry-run", "--plan-file", &pf, "--target-file", &p, &t]), &machine, false).unwrap(), 0);
    assert_eq!(whole_file_hash(&path), h0);
    let j = crate::json::Json::parse(&std::fs::read_to_string(&pf).unwrap()).unwrap();
    assert_eq!(j.get("format").and_then(crate::json::Json::as_str), Some("iyakki-install-plan-1"));
    assert!(j.get("steps").and_then(crate::json::Json::as_arr).unwrap().len() >= 3);
    assert_eq!(j.get("destroys").and_then(crate::json::Json::as_arr).unwrap().len(), 0);
    // apply without a typed confirmation: refused, nothing written
    let e = run_inner(&a(&["--apply-plan", &pf, "--machine", "--target-file", &p, "--template", &t]), &machine, false).unwrap_err();
    assert!(e.contains("confirmation"), "{e}");
    assert_eq!(whole_file_hash(&path), h0);
    // a plan file others can write to is refused
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&pf, std::fs::Permissions::from_mode(0o666)).unwrap();
        let e = run_inner(&a(&["--apply-plan", &pf, "--target-file", &p, "--template", &t]), &machine, false).unwrap_err();
        assert!(e.contains("not writable by others"), "{e}");
    }
    // the model of the file, the plan, and the confirmation stored by the window
    let model = {
        let meta = DiskMeta { name: "testfile".into(), path: p.clone(), model: "test file".into(), removable: true, usb: true, ..Default::default() };
        let mut h = std::fs::File::open(&path).unwrap();
        read_model(&mut h, &meta, 3 * GIB, 512, &[], None)
    };
    let req = Request { mode: Mode::FreeSpace, ..Request::default() };
    let plan = build_plan(&model, &req, &spec(&s), crate::install::booted_uefi()).unwrap();
    let typed = |n: &str, z: &str| Confirmation::Typed { name: n.into(), size: z.into() };
    write_plan_file(Path::new(&pf), &plan, Some(&typed("testfile", "1 GB"))).unwrap();
    let e = run_inner(&a(&["--apply-plan", &pf, "--target-file", &p, "--template", &t]), &machine, false).unwrap_err();
    assert!(e.contains("typed confirmation") && e.contains("does not match"), "{e}");
    assert_eq!(whole_file_hash(&path), h0);
    // the Install button of the screens: the drive and its exact size (a free-space plan erases nothing, so no acknowledgement)
    let wrong_size = Confirmation::Button { drive: "testfile".into(), size: 1, erase_ack: false };
    write_plan_file(Path::new(&pf), &plan, Some(&wrong_size)).unwrap();
    let e = run_inner(&a(&["--apply-plan", &pf, "--target-file", &p, "--template", &t]), &machine, false).unwrap_err();
    assert!(e.contains("different drive or size"), "{e}");
    assert_eq!(whole_file_hash(&path), h0);
    write_plan_file(Path::new(&pf), &plan, Some(&Confirmation::Button { drive: "testfile".into(), size: plan.disk_size, erase_ack: false })).unwrap();
    assert_eq!(run_inner(&a(&["--apply-plan", &pf, "--machine", "--target-file", &p, "--template", &t]), &machine, false).unwrap(), 0);
    assert_ne!(whole_file_hash(&path), h0);
    // (the old typed form is still accepted: see the next lines, on a fresh copy of the disk)
    let (path2, _) = gpt_disk(&dir, "p2.img", 3 * GIB, 512, &[ps(1, 500, MSDATA, "other", Fill::Ntfs)]);
    let p2 = path2.display().to_string();
    let model2 = {
        let meta = DiskMeta { name: "testfile".into(), path: p2.clone(), model: "test file".into(), removable: true, usb: true, ..Default::default() };
        let mut h = std::fs::File::open(&path2).unwrap();
        read_model(&mut h, &meta, 3 * GIB, 512, &[], None)
    };
    let plan2 = build_plan(&model2, &req, &spec(&s), crate::install::booted_uefi()).unwrap();
    write_plan_file(Path::new(&pf), &plan2, Some(&typed("testfile", "3.2 GB"))).unwrap();
    assert_eq!(run_inner(&a(&["--apply-plan", &pf, "--machine", "--target-file", &p2, "--template", &t]), &machine, false).unwrap(), 0);
    let _ = (&path2, &p);
    write_plan_file(Path::new(&pf), &plan, Some(&Confirmation::Button { drive: "testfile".into(), size: plan.disk_size, erase_ack: false })).unwrap();

    assert_ne!(whole_file_hash(&path), h0);
    let parsed = parsed(&path, 512, 3 * GIB);
    assert!(parsed.primary_ok && parsed.backup_ok);
    assert_eq!(parsed.table.entries.len(), 4);
    // the same file now fails the digest: the disk changed since the plan was confirmed
    let e = run_inner(&a(&["--apply-plan", &pf, "--target-file", &p, "--template", &t]), &machine, false).unwrap_err();
    assert!(e.contains("digests differ") || e.contains("no free space") || e.contains("too small"), "{e}");
}

#[test]
fn a_gpt_stick_is_a_source_for_the_boot_time_installer() {
    use crate::install::{execute, plan_stick_gpt};
    let (dir, _guard) = scratch("gptsrc");
    let s = src(&dir);
    let stick = dir.join("stick");
    std::fs::File::create(&stick).unwrap();
    let drive = crate::drives::Drive { path: stick.display().to_string(), name: "testfile".into(), model: "test file".into(), size_bytes: 512 * MIB, removable: true, usb: true, read_only: false, mounts: vec![], swap: false };
    let plan = plan_stick_gpt(&drive, 512, &s, "chosen: GPT").unwrap();
    execute(&plan, false, "", &mut |_| {}).unwrap();
    // the booted stick is read as a source: boot volume, data partition and GRUB's BIOS boot partition by their names
    let from_stick = read_source(&stick).unwrap();
    assert_eq!(from_stick.p1_sectors, s.p1_sectors);
    assert!(from_stick.bios.is_some());
    let sp = spec(&from_stick);
    assert_eq!(sp.boot_bytes, spec(&s).boot_bytes);
    assert_eq!(sp.payload_bytes, spec(&s).payload_bytes, "the kernel and RAM disk are found on the stick");
    // install in free space of another disk from that stick
    let (path, old) = gpt_disk(&dir, "target.img", 3 * GIB, 512, &[ps(1, 500, MSDATA, "other", Fill::Ntfs)]);
    let before = old_partition_bytes(&path, &old, 512);
    let m = model_of(&path, "sdc", 3 * GIB, 512, "", false);
    let plan = build_plan(&m, &free_req(), &sp, true).unwrap();
    run_apply(&path, &m, &free_req(), &from_stick, Some(&plan.digest()), &ApplyEnv::default()).unwrap();
    assert_eq!(old_partition_bytes(&path, &old, 512), before);
    let p = parsed(&path, 512, 3 * GIB);
    let boot = p.table.entries.iter().find(|e| e.name == "IYAKKI-BOOT").unwrap();
    let mut h = std::fs::File::open(&path).unwrap();
    let mut v = Fat32::open(&mut h, boot.first_lba * 512).unwrap();
    let items = v.walk().unwrap();
    let initrd = items.iter().find(|i| i.path == "/BOOT/INITRD").unwrap().clone();
    assert_eq!(v.read_file(&initrd).unwrap(), pseudo(1_500_000, 4));
    // a whole-disk GPT install from the stick carries GRUB's BIOS core over
    let whole = dir.join("whole.img");
    std::fs::File::create(&whole).unwrap().set_len(5 * GIB).unwrap();
    let mw = model_of(&whole, "sdd", 5 * GIB, 512, "", true);
    let req = Request { mode: Mode::WholeDisk, table: TableChoice::Gpt, ..Request::default() };
    let pw = build_plan(&mw, &req, &sp, true).unwrap();
    run_apply(&whole, &mw, &req, &from_stick, Some(&pw.digest()), &ApplyEnv::default()).unwrap();
    let pp = parsed(&whole, 512, 5 * GIB);
    assert_eq!(pp.table.entries.len(), 3);
    let bios_new = bytes_at(&whole, pp.table.entries[0].first_lba * 512, 4096);
    assert!(bios_new.iter().any(|b| *b != 0), "GRUB's core image is in the BIOS boot partition");
    // a whole-disk MBR install needs the template, and says so
    let mbr = Request { table: TableChoice::Mbr, ..req };
    let e = run_apply(&whole, &mw, &mbr, &from_stick, None, &ApplyEnv::default()).unwrap_err();
    assert!(e.contains("needs the install template"), "{e}");
}

/// Not a test: writes a template and a Windows-like disk image into `$IYAKKI_FIXTURE_DIR`, for driving the terminal pages by hand
/// (`cargo test -p iyakki-usb make_fixtures -- --ignored`, then `iyakki-usb --plan --target-file DIR/disk.img DIR/t.img`).
#[test]
#[ignore]
fn make_fixtures_for_a_terminal_smoke_test() {
    let dir = PathBuf::from(std::env::var("IYAKKI_FIXTURE_DIR").expect("set IYAKKI_FIXTURE_DIR to an empty temp folder"));
    std::fs::create_dir_all(&dir).unwrap();
    src(&dir);
    let esp = foreign_esp(&dir);
    gpt_disk(&dir, "disk.img", 3 * GIB, 512, &[ps(1, 100, EFI, "EFI system partition", Fill::Raw(esp)), ps(101, 1500, MSDATA, "Basic data partition", Fill::Ntfs)]);
}

// ---------------------------------------------------------------------------------------------------------------------------
// the screens' confirmation (the Install button; owner's decision 2026-10-24), the fast-start files, the window's pages

fn button(plan: &Plan, ack: bool) -> Confirmation {
    Confirmation::Button { drive: plan.disk_name.clone(), size: plan.disk_size, erase_ack: ack }
}

#[test]
fn a_plan_that_erases_nothing_needs_only_the_button_and_one_that_erases_needs_the_acknowledgement_too() {
    let (dir, _guard) = scratch("button");
    let s = src(&dir);
    let sp = spec(&s);
    let (path, _) = windows_disk(&dir, 40 * GIB, 512);
    let m = model_of(&path, "sda", 40 * GIB, 512, "", false);
    // free space: nothing erased
    let free = build_plan(&m, &free_req(), &sp, true).unwrap();
    assert!(!free.erases_data());
    assert!(check_confirmation(&free, &button(&free, false)).is_ok(), "the button alone");
    assert!(check_confirmation(&free, &button(&free, true)).is_ok());
    // the button must name this drive and its exact size
    let wrong_drive = Confirmation::Button { drive: "sdb".into(), size: free.disk_size, erase_ack: true };
    let wrong_size = Confirmation::Button { drive: "sda".into(), size: free.disk_size + 1, erase_ack: true };
    for c in [wrong_drive, wrong_size, Confirmation::Button { drive: String::new(), size: free.disk_size, erase_ack: true }] {
        assert!(check_confirmation(&free, &c).unwrap_err().contains("different drive or size"), "{c:?}");
    }
    assert!(check_confirmation(&free, &Confirmation::Button { drive: free.disk_path.clone(), size: free.disk_size, erase_ack: false }).is_ok(), "the path works as the name");
    // whole disk: deletes every partition
    let whole = build_plan(&m, &Request::default(), &sp, true).unwrap();
    assert!(whole.erases_data() && whole.destructive());
    let e = check_confirmation(&whole, &button(&whole, false)).unwrap_err();
    assert!(e.contains("erase acknowledgement"), "{e}");
    assert!(check_confirmation(&whole, &button(&whole, true)).is_ok());
    // the old typed form is still accepted for both, and is as good as the acknowledgement
    let typed = Confirmation::Typed { name: "sda".into(), size: "42.9 GB".into() };
    assert!(check_confirmation(&free, &typed).is_ok() && check_confirmation(&whole, &typed).is_ok());
    assert!(check_confirmation(&whole, &Confirmation::Typed { name: "sda".into(), size: "40 GB".into() }).unwrap_err().contains("typed confirmation"));
    // the plan file object: both forms round-trip, and an object with neither is unreadable
    for c in [typed, button(&whole, true), button(&free, false)] {
        assert_eq!(Confirmation::from_json(&crate::json::Json::parse(&c.to_json().pretty()).unwrap()), Some(c));
    }
    let old = crate::json::Json::parse("{\"name\": \"sda\", \"size\": \"42.9 GB\"}").unwrap();
    assert_eq!(Confirmation::from_json(&old), Some(Confirmation::Typed { name: "sda".into(), size: "42.9 GB".into() }));
    assert_eq!(Confirmation::from_json(&crate::json::Json::parse("{\"method\": \"button\", \"drive\": \"sda\", \"size\": 5}").unwrap()), Some(Confirmation::Button { drive: "sda".into(), size: 5, erase_ack: false }), "a missing ack is false");
    assert_eq!(Confirmation::from_json(&crate::json::Json::parse("{\"method\": \"button\"}").unwrap()), None);
    assert_eq!(Confirmation::from_json(&crate::json::Json::parse("{}").unwrap()), None);
}

#[test]
fn reusing_a_blank_partition_needs_only_the_button_and_formatting_a_filesystem_needs_the_acknowledgement() {
    let (dir, _guard) = scratch("reuse-ack");
    let s = src(&dir);
    let sp = spec(&s);
    let (path, _) = gpt_disk(&dir, "r.img", 4 * GIB, 512, &[ps(1, 600, MSDATA, "blank", Fill::Blank), ps(601, 1000, MSDATA, "blank2", Fill::Blank), ps(1601, 800, MSDATA, "data", Fill::Ntfs)]);
    let m = model_of(&path, "sda", 4 * GIB, 512, "", true);
    let blank = Request { mode: Mode::Reuse, boot_part: Some(1), data_part: Some(2), ..Request::default() };
    let p = build_plan(&m, &blank, &sp, true).unwrap();
    assert!(p.destructive() && !p.erases_data(), "formatted, but nothing was there to lose");
    assert!(check_confirmation(&p, &button(&p, false)).is_ok());
    let holds = Request { mode: Mode::Reuse, boot_part: Some(1), data_part: Some(3), ..Request::default() };
    let p = build_plan(&m, &holds, &sp, true).unwrap();
    assert!(p.erases_data());
    assert!(check_confirmation(&p, &button(&p, false)).is_err() && check_confirmation(&p, &button(&p, true)).is_ok());
}

#[test]
fn the_writer_refuses_an_erasing_plan_file_without_the_acknowledgement_and_accepts_it_with() {
    use crate::plan_cli::{run_inner, write_plan_file, Machine};
    let (dir, _guard) = scratch("ack-file");
    let s = src(&dir);
    let (path, _) = gpt_disk(&dir, "e.img", 5 * GIB, 512, &[ps(1, 500, MSDATA, "other", Fill::Ntfs)]);
    let machine = Machine { sys: "/nonexistent".into(), dev: "/nonexistent".into(), proc_mounts: String::new(), swaps: String::new() };
    let a = |v: &[&str]| -> Vec<String> { v.iter().map(|x| x.to_string()).collect() };
    let (p, t, pf) = (path.display().to_string(), s.path.display().to_string(), dir.join("plan.json").display().to_string());
    let h0 = whole_file_hash(&path);
    let meta = DiskMeta { name: "testfile".into(), path: p.clone(), model: "test file".into(), removable: true, usb: true, ..Default::default() };
    let model = read_model(&mut std::fs::File::open(&path).unwrap(), &meta, 5 * GIB, 512, &[], None);
    let plan = build_plan(&model, &Request::default(), &spec(&s), crate::install::booted_uefi()).unwrap();
    assert!(plan.erases_data());
    // no acknowledgement: refused, not one byte written
    write_plan_file(Path::new(&pf), &plan, Some(&button(&plan, false))).unwrap();
    let e = run_inner(&a(&["--apply-plan", &pf, "--machine", "--target-file", &p, "--template", &t]), &machine, false).unwrap_err();
    assert!(e.contains("erase acknowledgement"), "{e}");
    assert_eq!(whole_file_hash(&path), h0);
    // the JSON written by an older window (or by hand) with the field missing is the same as false
    let mut j = crate::json::Json::parse(&std::fs::read_to_string(&pf).unwrap()).unwrap();
    if let crate::json::Json::Obj(f) = &mut j {
        f.retain(|(k, _)| k != "confirmed");
        f.push(("confirmed".into(), crate::json::Json::parse(&format!("{{\"method\": \"button\", \"drive\": \"testfile\", \"size\": {}}}", plan.disk_size)).unwrap()));
    }
    std::fs::write(&pf, j.pretty()).unwrap();
    let e = run_inner(&a(&["--apply-plan", &pf, "--machine", "--target-file", &p, "--template", &t]), &machine, false).unwrap_err();
    assert!(e.contains("erase acknowledgement"), "{e}");
    assert_eq!(whole_file_hash(&path), h0);
    // with the acknowledgement it is written
    write_plan_file(Path::new(&pf), &plan, Some(&button(&plan, true))).unwrap();
    assert_eq!(run_inner(&a(&["--apply-plan", &pf, "--machine", "--target-file", &p, "--template", &t]), &machine, false).unwrap(), 0);
    assert_ne!(whole_file_hash(&path), h0);
}

#[test]
fn the_confirm_page_shows_what_is_created_kept_and_erased_and_an_erasing_plan_needs_the_box() {
    let (dir, _guard) = scratch("wiz-erase");
    let s = src(&dir);
    let (path, _) = windows_disk(&dir, 40 * GIB, 512);
    let h0 = whole_file_hash(&path);
    let internal = model_of(&path, "sda", 40 * GIB, 512, "", false);
    let mut w = Wizard::new(vec![internal], spec(&s), true);
    w.handle(WKey::Enter);
    w.mode_sel = 0; // whole disk
    assert_eq!(w.mode(), Mode::WholeDisk);
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Plan);
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Confirm);
    let c = w.confirm_summary().unwrap();
    assert!(c.needs_ack && c.button == "Erase sda (42.9 GB) and install", "{}", c.button);
    assert!(c.erase.len() == 4 && c.keep.is_empty() && c.create.len() >= 2, "{c:?}");
    assert!(c.warnings.iter().any(|x| x.contains("another system") && x.contains("Windows")), "an internal disk with NTFS: {:?}", c.warnings);
    assert_eq!(w.cfocus, CFocus::Cancel, "the focus starts on Cancel when data is erased");
    assert_eq!(w.confirm_order(), [CFocus::Ack, CFocus::Install, CFocus::Back, CFocus::Cancel]);
    let screen = w.render(100);
    assert!(screen.contains("Erase sda (42.9 GB) and install") && screen.contains("I understand that everything on this drive will be erased") && screen.contains("Will be ERASED"), "{screen}");
    // Install is pressed without the box: nothing happens but a message
    w.cfocus = CFocus::Install;
    assert_eq!(w.handle(WKey::Enter), Next::Nothing);
    assert_eq!(w.page, Page::Confirm);
    assert!(w.error.as_deref().unwrap().contains("Tick the box"));
    assert_eq!(w.confirmation(), None, "no proof exists until the box is ticked");
    assert_eq!(whole_file_hash(&path), h0);
    // Space on the box ticks it (and again clears it); the button writes with the acknowledgement in the proof
    w.cfocus = CFocus::Ack;
    w.handle(WKey::Char(' '));
    assert!(w.erase_ack);
    w.handle(WKey::Char(' '));
    assert!(!w.erase_ack);
    w.handle(WKey::Enter);
    assert!(w.erase_ack, "Enter on the box ticks it too");
    assert!(w.render(100).contains("[x] I understand"));
    w.handle(WKey::Tab);
    assert_eq!(w.cfocus, CFocus::Install);
    let want = Confirmation::Button { drive: "sda".into(), size: w.plan.as_ref().unwrap().disk_size, erase_ack: true };
    assert_eq!(w.confirmation(), Some(want));
    assert_eq!(w.handle(WKey::Enter), Next::Write);
    // coming back to the page clears the box
    let mut w2 = Wizard::new(w.disks.clone(), spec(&s), true);
    w2.plan = w.plan.clone();
    w2.page = Page::Plan;
    w2.erase_ack = true;
    w2.handle(WKey::Enter);
    assert!(!w2.erase_ack && w2.cfocus == CFocus::Cancel);
    // Cancel quits and Back goes to the plan; Esc is Back
    assert_eq!(w2.activate(CFocus::Cancel), Next::Quit);
    assert_eq!(w2.activate(CFocus::Back), Next::Nothing);
    assert_eq!(w2.page, Page::Plan);
    assert_eq!(whole_file_hash(&path), h0, "the pages never write");
}

#[test]
fn the_extra_warnings_are_visible_lines_not_typing() {
    let (dir, _guard) = scratch("wiz-warn");
    let s = src(&dir);
    let (path, _) = windows_disk(&dir, 40 * GIB, 512);
    // the disk of the running system, free-space mode: a warning line, one button, no box
    let running = model_of(&path, "sda", 40 * GIB, 512, "/dev/sda3 / ext4 rw 0 0\n", false);
    let mut w = Wizard::new(vec![running], spec(&s), true);
    w.handle(WKey::Enter);
    assert_eq!(w.mode(), Mode::FreeSpace);
    w.handle(WKey::Enter);
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Plan, "{:?}", w.error);
    w.handle(WKey::Enter);
    let c = w.confirm_summary().unwrap();
    assert!(!c.needs_ack && c.button == "Install");
    assert!(c.warnings.iter().any(|x| x.contains("holds the system that is running now")), "{:?}", c.warnings);
    assert!(c.warnings.iter().any(|x| x.contains("Windows")), "{:?}", c.warnings);
    assert_eq!(w.cfocus, CFocus::Install);
    assert!(w.render(100).contains("! This drive holds the system that is running now"));
    // a mounted partition that the plan unmounts first is a line too
    let mut m = model_of(&path, "sda", 40 * GIB, 512, "", false);
    m.parts.iter_mut().for_each(|p| p.mounts.clear());
    let mut w = Wizard::new(vec![m], spec(&s), true);
    w.handle(WKey::Enter);
    w.handle(WKey::Enter);
    w.handle(WKey::Enter);
    w.plan.as_mut().unwrap().unmount = vec!["sda9".into()];
    w.handle(WKey::Enter);
    assert!(w.confirm_summary().unwrap().warnings.iter().any(|x| x.contains("unmounted first: sda9")));
    // a USB disk with an NTFS partition is not "another system on an internal disk"
    let usb = model_of(&path, "sdb", 40 * GIB, 512, "", true);
    let mut w = Wizard::new(vec![usb], spec(&s), true);
    w.handle(WKey::Enter);
    w.handle(WKey::Enter);
    w.handle(WKey::Enter);
    w.handle(WKey::Enter);
    assert!(w.confirm_summary().unwrap().warnings.is_empty());
}

#[test]
fn arrow_keys_left_and_right_are_read() {
    use crate::wizard::parse_wkeys;
    assert_eq!(parse_wkeys(b"\x1b[C\x1b[D\x1b[A "), [WKey::Right, WKey::Left, WKey::Up, WKey::Char(' ')]);
}

/// The install stick of a machine that ran the fast start: lean entries in the menu, the lean RAM disk, the stage-1 log.
fn src_with_lean(dir: &Path) -> SourceInfo {
    let cfg = crate::leanstrip::tests_sample().as_bytes().to_vec();
    let fat = mini_fat(512, 1, 40 * MIB, &[("BOOT/BZIMAGE", pseudo(700_001, 3)), ("BOOT/INITRD", pseudo(1_500_000, 4)), ("BOOT/initrd-lean", pseudo(200_000, 6)), ("BOOT/STAGE1.LOG", b"stage 1\n".to_vec()), ("BOOT/GRUB/grub.cfg", cfg)]);
    let path = dir.join("lean.img");
    let mut t = template(&path);
    let p1_sectors = (fat.len() / 512) as u32;
    let p2_start = 2048 + p1_sectors;
    t[446 + 12..446 + 16].copy_from_slice(&p1_sectors.to_le_bytes());
    t[462 + 8..462 + 12].copy_from_slice(&p2_start.to_le_bytes());
    t.truncate(2048 * 512);
    t.extend_from_slice(&fat);
    t.resize((p2_start as usize + 2048) * 512, 0);
    std::fs::write(&path, &t).unwrap();
    read_source(&path).unwrap()
}

#[test]
fn an_install_from_a_stick_with_the_fast_start_copies_neither_its_menu_entries_nor_its_files() {
    let (dir, _guard) = scratch("lean-copy");
    let s = src_with_lean(&dir);
    let (path, _) = gpt_disk(&dir, "l.img", 6 * GIB, 512, &[ps(1, 2048, MSDATA, "other", Fill::Ntfs)]);
    let m = model_of(&path, "sda", 6 * GIB, 512, "", true);
    let plan = build_plan(&m, &free_req(), &spec(&s), true).unwrap();
    let rep = run_apply(&path, &m, &free_req(), &s, Some(&plan.digest()), &ApplyEnv::default()).unwrap();
    assert!(rep.verified);
    assert!(rep.notes.iter().any(|n| n.contains("fast-start") && n.contains("default entry is IYAKKI")), "{:?}", rep.notes);
    let p = parsed(&path, 512, 6 * GIB);
    let boot = p.table.entries.iter().find(|e| e.name == "IYAKKI-BOOT").unwrap();
    let mut h = std::fs::File::open(&path).unwrap();
    let mut vol = Fat32::open(&mut h, boot.first_lba * 512).unwrap();
    let items = vol.walk_long().unwrap();
    let names: Vec<&str> = items.iter().map(|i| i.path.as_str()).collect();
    assert_eq!(names, ["/BOOT", "/BOOT/BZIMAGE", "/BOOT/GRUB", "/BOOT/GRUB/grub.cfg", "/BOOT/INITRD"], "no initrd-lean, no stage1.log");
    let cfg = String::from_utf8(vol.read_file(items.iter().find(|i| i.path.ends_with("grub.cfg")).unwrap()).unwrap()).unwrap();
    assert!(!cfg.contains("lean") && !cfg.contains("fast start"), "{cfg}");
    assert_eq!(cfg.lines().find(|l| l.starts_with("set default")), Some("set default=\"0\""));
    assert_eq!(cfg.lines().find(|l| l.starts_with("menuentry")), Some("menuentry \"IYAKKI\" {"), "the first entry, the default, is the normal one");
    assert_eq!(vol.read_file(items.iter().find(|i| i.path == "/BOOT/INITRD").unwrap()).unwrap(), pseudo(1_500_000, 4), "the system files are copied unchanged");
    // a stick without the fast start is copied as before (the normal path, nothing noted)
    let plain = src(&dir);
    let (path2, _) = gpt_disk(&dir, "l2.img", 6 * GIB, 512, &[ps(1, 2048, MSDATA, "other", Fill::Ntfs)]);
    let m2 = model_of(&path2, "sda", 6 * GIB, 512, "", true);
    let plan2 = build_plan(&m2, &free_req(), &spec(&plain), true).unwrap();
    let rep2 = run_apply(&path2, &m2, &free_req(), &plain, Some(&plan2.digest()), &ApplyEnv::default()).unwrap();
    assert!(!rep2.notes.iter().any(|n| n.contains("fast-start")), "{:?}", rep2.notes);
}

// ---------------------------------------------------------------------------------------------------------------------------
// deleting an existing partition first (free-space and custom mode), with the erase acknowledgement

/// A 6 GiB GPT disk with no room left: an EFI System Partition, a Windows data partition and a big Linux one.
fn crowded_gpt(dir: &Path, name: &str) -> (PathBuf, Table) {
    gpt_disk(dir, name, 6 * GIB, 512, &[ps(1, 100, EFI, "EFI system partition", Fill::Raw(foreign_esp(dir))), ps(101, 2048, MSDATA, "Basic data partition", Fill::Ntfs), ps(2149, 3950, gpt::TYPE_LINUX_DATA, "old linux", Fill::Ext4)])
}

#[test]
fn a_partition_chosen_for_deletion_makes_room_and_the_plan_shows_it_as_an_erasing_row() {
    let (dir, _guard) = scratch("del-plan");
    let s = src(&dir);
    let (path, _) = crowded_gpt(&dir, "c.img");
    let m = model_of(&path, "sda", 6 * GIB, 512, "", true);
    // without the deletion there is no room
    assert!(build_plan(&m, &free_req(), &spec(&s), true).is_err());
    let req = Request { delete_parts: vec![3], ..free_req() };
    let plan = build_plan(&m, &req, &spec(&s), true).unwrap();
    let del: Vec<&Step> = plan.steps.iter().filter(|st| st.action == Action::Delete).collect();
    assert_eq!((del.len(), del[0].part.as_str(), del[0].index), (1, "sda3", 3));
    assert_eq!(plan.steps.iter().filter(|st| st.action == Action::Keep).count(), 2, "the other two stay");
    let creates: Vec<&Step> = plan.steps.iter().filter(|st| st.action == Action::Create).collect();
    assert_eq!((creates.len(), creates[0].index, creates[1].index, creates[2].index), (3, 3, 4, 5), "the freed slot is used again (BIOS boot, boot, data)");
    assert!(creates[0].first_lba >= del[0].first_lba, "the new partitions start where the deleted one did");
    assert!(plan.erases_data() && plan.destroyed() == vec!["sda3".to_string()]);
    let text = plan.render();
    for want in ["delete", "sda3", "partition(s) are deleted first"] {
        assert!(text.contains(want) || plan.warnings.iter().any(|w| w.contains(want)), "{want} in\n{text}");
    }
    // one button is not enough for a plan that deletes: the erase acknowledgement is needed too
    assert!(check_confirmation(&plan, &button(&plan, false)).is_err());
    assert!(check_confirmation(&plan, &button(&plan, true)).is_ok());
    // the request survives the plan file
    assert_eq!(Request::from_json(&crate::json::Json::parse(&plan.request.to_json().pretty()).unwrap()).unwrap().delete_parts, vec![3]);
}

#[test]
fn deleting_is_refused_where_it_would_hurt_or_make_no_sense() {
    let (dir, _guard) = scratch("del-refuse");
    let s = src(&dir);
    let (path, _) = crowded_gpt(&dir, "r.img");
    let m = model_of(&path, "sda", 6 * GIB, 512, "", true);
    let try_mode = |mode: Mode, del: Vec<u32>| build_plan(&m, &Request { mode, delete_parts: del, ..Request::default() }, &spec(&s), true);
    assert!(try_mode(Mode::WholeDisk, vec![3]).unwrap_err().contains("free-space and custom"));
    assert!(try_mode(Mode::Reuse, vec![3]).unwrap_err().contains("free-space and custom"));
    assert!(try_mode(Mode::FreeSpace, vec![9]).unwrap_err().contains("does not exist"));
    assert!(try_mode(Mode::FreeSpace, vec![3, 3]).unwrap_err().contains("twice"));
    // the running system's own volume is never deleted
    let mounted = model_of(&path, "sda", 6 * GIB, 512, "/dev/sda3 / ext4 rw 0 0\n", false);
    let e = build_plan(&mounted, &Request { delete_parts: vec![3], ..free_req() }, &spec(&s), true).unwrap_err();
    assert!(e.contains("running system"), "{e}");
    // an MBR logical partition is not touched
    let mbr = mbr_disk(&dir, "m.img", 4 * GIB, &[(1, 1000, 0x07, Fill::Ntfs)]);
    let mm = model_of(&mbr, "sdb", 4 * GIB, 512, "", true);
    assert!(build_plan(&mm, &Request { delete_parts: vec![5], ..free_req() }, &spec(&s), true).is_err());
}

#[test]
fn a_gpt_install_after_deleting_a_partition_removes_only_that_entry_and_keeps_the_others_byte_for_byte() {
    let (dir, _guard) = scratch("del-apply-gpt");
    let s = src(&dir);
    let (path, old) = crowded_gpt(&dir, "g.img");
    let before = old_partition_bytes(&path, &old, 512);
    let sector0 = bytes_at(&path, 0, 512);
    let m = model_of(&path, "sda", 6 * GIB, 512, "", true);
    let req = Request { delete_parts: vec![3], ..free_req() };
    let plan = build_plan(&m, &req, &spec(&s), true).unwrap();
    let rep = run_apply(&path, &m, &req, &s, Some(&plan.digest()), &ApplyEnv::default()).unwrap();
    assert!(rep.verified);
    let p = parsed(&path, 512, 6 * GIB);
    assert!(p.primary_ok && p.backup_ok && p.problems.is_empty(), "{:?}", p.problems);
    assert_eq!(p.table.entries.len(), 5, "two kept, three new");
    for e in old.entries.iter().filter(|e| e.index != 3) {
        assert_eq!(p.table.entries.iter().find(|x| x.index == e.index).unwrap(), e, "entry {} is unchanged", e.index);
    }
    let after = old_partition_bytes(&path, &Table { entries: old.entries.iter().filter(|e| e.index != 3).cloned().collect(), ..old.clone() }, 512);
    assert_eq!(after, before.iter().filter(|(i, _)| *i != 3).cloned().collect::<Vec<_>>(), "the kept partitions: not one byte changed");
    assert_eq!(bytes_at(&path, 0, 512), sector0, "the protective MBR stays");
    let names: Vec<&str> = p.table.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(&names[2..], &["IYAKKI-BIOS", "IYAKKI-BOOT", "IYAKKI-DATA"], "{names:?}");
    assert!(!p.table.entries.iter().any(|e| e.name == "old linux"));
    assert!(tool("sgdisk", &["-v", path.to_str().unwrap()]).is_none_or(|(_, out)| out.contains("No problems found")), "sgdisk");
}

#[test]
fn an_mbr_install_after_deleting_two_partitions_clears_those_entries_and_keeps_boot_code_and_the_rest() {
    let (dir, _guard) = scratch("del-apply-mbr");
    let s = src(&dir);
    let path = mbr_disk(&dir, "d.img", 4 * GIB, &[(1, 1000, 0x07, Fill::Ntfs), (1001, 500, 0x83, Fill::Ext4), (1501, 1000, 0x83, Fill::Ext4), (2501, 1400, 0x07, Fill::Ntfs)]);
    let s0 = bytes_at(&path, 0, 512);
    let m = model_of(&path, "sda", 4 * GIB, 512, "", true);
    assert!(build_plan(&m, &free_req(), &spec(&s), true).is_err(), "four slots are used and no space is left");
    // one deletion frees only one slot, and the install needs two
    assert!(build_plan(&m, &Request { delete_parts: vec![4], ..free_req() }, &spec(&s), true).is_err());
    let req = Request { delete_parts: vec![3, 4], ..free_req() };
    let plan = build_plan(&m, &req, &spec(&s), true).unwrap();
    run_apply(&path, &m, &req, &s, Some(&plan.digest()), &ApplyEnv::default()).unwrap();
    let n0 = bytes_at(&path, 0, 512);
    assert_eq!(n0[..446], s0[..446], "boot code and signature: unchanged");
    assert_eq!(n0[446..478], s0[446..478], "the two kept entries: unchanged");
    assert_eq!((n0[478 + 4], n0[494 + 4]), (0xEF, 0x83), "slots 3 and 4 now hold IYAKKI-BOOT and IYAKKI-DATA");
    let start = u32::from_le_bytes(n0[478 + 8..478 + 12].try_into().unwrap()) as u64;
    assert!(start >= 1501 * 2048 && start % 2048 == 0, "the boot partition starts in the freed space");
    assert_eq!(&n0[510..], &[0x55, 0xAA]);
    assert!(tool("sfdisk", &["--verify", path.to_str().unwrap()]).is_none_or(|(ok, _)| ok));
}

#[test]
fn the_pages_let_a_person_mark_a_partition_for_deletion_and_confirm_it_with_the_erase_box() {
    use crate::wizard::Row;
    let (dir, _guard) = scratch("wiz-del");
    let s = src(&dir);
    let (path, _) = crowded_gpt(&dir, "w.img");
    let h0 = whole_file_hash(&path);
    let internal = model_of(&path, "sda", 6 * GIB, 512, "", false);
    let mut w = Wizard::new(vec![internal], spec(&s), true);
    w.handle(WKey::Enter);
    assert_eq!(w.mode(), Mode::FreeSpace);
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Choose);
    // the partitions are listed with the free space, and the cursor starts on the free space
    let text = w.render(100);
    assert!(text.contains("x = delete the partition under the cursor") && text.contains("old linux"), "{text}");
    assert!(matches!(w.rows()[w.row_sel], Row::Gap { .. }));
    // the little free space is not enough
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Choose);
    assert!(w.error.as_deref().is_some_and(|e| e.contains("too small")), "{:?}", w.error);
    // x on free space explains; on the partition it marks it
    w.handle(WKey::Char('x'));
    assert!(w.error.as_deref().is_some_and(|e| e.contains("Move to a partition")));
    w.row_sel = w.rows().iter().position(|r| matches!(r, Row::Part { index: 3, .. })).unwrap();
    w.handle(WKey::Enter);
    assert!(w.error.as_deref().is_some_and(|e| e.contains("Press x")), "Enter on a partition does not install");
    w.handle(WKey::Char('x'));
    assert_eq!(w.req.delete_parts, vec![3]);
    let text = w.render(100);
    assert!(text.contains("[DELETE]") && text.contains("1 partition(s) marked for deletion"), "{text}");
    // x again takes the mark off, and once more puts it on
    w.handle(WKey::Char('x'));
    assert!(w.req.delete_parts.is_empty());
    w.handle(WKey::Char('x'));
    // the freed space is now a big free-space row: choose it
    w.row_sel = w.rows().iter().position(|r| matches!(r, Row::Gap { bytes, .. } if *bytes > 3 * GIB)).expect("the freed space is listed");
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Plan, "{:?}", w.error);
    let plan_text = w.render(100);
    assert!(plan_text.contains("delete") && plan_text.contains("sda3") && plan_text.contains("create"), "{plan_text}");
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Confirm);
    let screen = w.render(100);
    assert!(screen.contains("Will be ERASED") && screen.contains("sda3") && screen.contains("I understand"), "{screen}");
    // Install without the box is not a write
    w.cfocus = CFocus::Install;
    assert_ne!(w.handle(WKey::Enter), Next::Write, "the erase box has to be ticked first");
    assert_eq!(whole_file_hash(&path), h0, "the pages never write");
    // Esc goes back to the list with the mark still there; Mode again clears it
    w.page = Page::Choose;
    assert_eq!(w.req.delete_parts, vec![3]);
    w.page = Page::Mode;
    w.handle(WKey::Enter);
    assert!(w.req.delete_parts.is_empty(), "a new choice of mode starts without marks");
}

/// An MBR disk with a primary NTFS partition (1000 MiB), an extended partition (1500 MiB at 1001 MiB) holding two logical partitions
/// (NTFS 400 MiB and ext4 400 MiB, each behind its own extended boot record), and free space after it.
fn mbr_with_logicals(dir: &Path, name: &str) -> PathBuf {
    let path = mbr_disk(dir, name, 4 * GIB, &[(1, 1000, 0x07, Fill::Ntfs), (1001, 1500, 0x0F, Fill::Blank)]);
    let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    let k = MIB / 512;
    let ext = 1001 * k;
    let ebr = |rel_start: u64, len: u64, ty: u8, next_rel: u64| -> Vec<u8> {
        let mut b = vec![0u8; 512];
        b[446 + 4] = ty;
        b[446 + 8..446 + 12].copy_from_slice(&(rel_start as u32).to_le_bytes());
        b[446 + 12..446 + 16].copy_from_slice(&(len as u32).to_le_bytes());
        if next_rel != 0 {
            b[462 + 4] = 0x05;
            b[462 + 8..462 + 12].copy_from_slice(&(next_rel as u32).to_le_bytes());
            b[462 + 12..462 + 16].copy_from_slice(&(k as u32).to_le_bytes());
        }
        b[510] = 0x55;
        b[511] = 0xAA;
        b
    };
    f.write_all_at(&ebr(k, 400 * k, 0x07, 500 * k), ext * 512).unwrap();
    f.write_all_at(&ebr(k, 400 * k, 0x83, 0), (ext + 500 * k) * 512).unwrap();
    decorate(&f, 1002 * MIB, 400 * MIB, 5, &Fill::Ntfs);
    decorate(&f, 1502 * MIB, 400 * MIB, 6, &Fill::Ext4);
    path
}

#[test]
fn logical_partitions_in_an_extended_partition_are_listed_with_their_types_and_filesystems() {
    let (dir, _guard) = scratch("logicals");
    let path = mbr_with_logicals(&dir, "l.img");
    let m = model_of(&path, "sdb", 4 * GIB, 512, "/dev/sdb6 /mnt/data ext4 rw 0 0\n", false);
    let names: Vec<&str> = m.parts.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["sdb1", "sdb2", "sdb5", "sdb6"]);
    assert!(m.parts[1].extended);
    let (l5, l6) = (&m.parts[2], &m.parts[3]);
    assert_eq!((l5.index, l5.first_lba, l5.sectors, l5.fs.as_str(), l5.kind.as_str()), (5, 1002 * MIB / 512, 400 * MIB / 512, "ntfs", "NTFS/exFAT"));
    assert_eq!((l6.index, l6.first_lba, l6.fs.as_str(), l6.mbr_type), (6, 1502 * MIB / 512, "ext4", Some(0x83)));
    assert_eq!(l6.mounts, vec!["/mnt/data".to_string()], "mount state is matched for logical partitions too");
    assert!(m.notes.is_empty(), "{:?}", m.notes);
    // the free space is the area after the container; the area inside it is not offered
    let gaps = m.free_gaps(TableKind::Mbr);
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].first_lba, 2501 * MIB / 512);
    // a changed record changes the fingerprint
    let before = m.fingerprint.clone();
    let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.write_all_at(&[0x82], 1001 * MIB + 446 + 4).unwrap();
    assert_ne!(model_of(&path, "sdb", 4 * GIB, 512, "", false).fingerprint, before);
}

#[test]
fn a_broken_or_looping_logical_chain_is_cut_off_and_said_not_followed_forever() {
    let (dir, _guard) = scratch("logicals-loop");
    let path = mbr_with_logicals(&dir, "l.img");
    let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    // the second record links back to the first
    let mut b = bytes_at(&path, (1001 * 2048 + 500 * 2048) * 512, 512);
    b[462 + 4] = 0x05;
    b[462 + 8..462 + 12].copy_from_slice(&0u32.to_le_bytes());
    b[462 + 8..462 + 12].copy_from_slice(&1u32.to_le_bytes());
    f.write_all_at(&b, (1001 * 2048 + 500 * 2048) * 512).unwrap();
    let m = model_of(&path, "sdb", 4 * GIB, 512, "", false);
    assert_eq!(m.parts.len(), 4, "both partitions before the loop are shown");
    assert!(m.notes.iter().any(|n| n.contains("loops")), "{:?}", m.notes);
    // a record without the signature ends the chain
    let mut z = bytes_at(&path, 1001 * MIB, 512);
    z[510] = 0;
    f.write_all_at(&z, 1001 * MIB).unwrap();
    let m = model_of(&path, "sdb", 4 * GIB, 512, "", false);
    assert_eq!(m.parts.len(), 2);
    assert!(m.notes.iter().any(|n| n.contains("signature")), "{:?}", m.notes);
    // a logical partition that reaches past its container is not shown
    let mut o = bytes_at(&path, (1001 * 2048 + 500 * 2048) * 512, 512);
    o[510] = 0x55;
    o[462 + 4] = 0;
    o[446 + 12..446 + 16].copy_from_slice(&(2000u32 * 2048).to_le_bytes());
    f.write_all_at(&o, (1001 * 2048 + 500 * 2048) * 512).unwrap();
    let mut a = bytes_at(&path, 1001 * MIB, 512);
    a[510] = 0x55;
    f.write_all_at(&a, 1001 * MIB).unwrap();
    let m = model_of(&path, "sdb", 4 * GIB, 512, "", false);
    assert!(m.notes.iter().any(|n| n.contains("reaches past")), "{:?}", m.notes);
    assert!(m.parts.iter().all(|p| p.index != 6));
}

#[test]
fn a_free_space_install_beside_logical_partitions_keeps_every_record_and_partition_byte_for_byte() {
    let (dir, _guard) = scratch("logicals-apply");
    let s = src(&dir);
    let path = mbr_with_logicals(&dir, "l.img");
    let ext_before = bytes_at(&path, 1001 * MIB, 502 * MIB as usize);
    let second_before = bytes_at(&path, 1501 * MIB, 502 * MIB as usize);
    let m = model_of(&path, "sdb", 4 * GIB, 512, "", true);
    let plan = build_plan(&m, &free_req(), &spec(&s), true).unwrap();
    assert_eq!(plan.steps.iter().filter(|st| st.action == Action::Create).count(), 2, "{}", plan.render());
    assert_eq!(plan.steps.iter().filter(|st| st.action == Action::Keep).count(), 4, "the container and both logical partitions are kept");
    let rep = run_apply(&path, &m, &free_req(), &s, Some(&plan.digest()), &ApplyEnv::default()).unwrap();
    assert!(rep.verified);
    assert_eq!(bytes_at(&path, 1001 * MIB, 502 * MIB as usize), ext_before);
    assert_eq!(bytes_at(&path, 1501 * MIB, 502 * MIB as usize), second_before);
    // the logical partitions take none of the four primary slots: two primaries and the two new ones fill the table, a new run is refused
    let after = model_of(&path, "sdb", 4 * GIB, 512, "", true);
    assert_eq!(after.parts.len(), 6);
    assert!(after.parts.iter().filter(|p| p.index <= 4).count() == 4);
}

#[test]
fn logical_partitions_do_not_count_as_primary_slots_and_cannot_be_deleted_or_chosen_to_format() {
    let (dir, _guard) = scratch("logicals-rules");
    let s = src(&dir);
    let path = mbr_with_logicals(&dir, "l.img");
    let m = model_of(&path, "sdb", 4 * GIB, 512, "", true);
    assert!(build_plan(&m, &free_req(), &spec(&s), true).is_ok(), "four partitions on the disk, but only two primary slots are used");
    let e = build_plan(&m, &Request { delete_parts: vec![5], ..free_req() }, &spec(&s), true).unwrap_err();
    assert!(e.contains("logical partition"), "{e}");
    let e = build_plan(&m, &Request { delete_parts: vec![2], ..free_req() }, &spec(&s), true).unwrap_err();
    assert!(e.contains("extended"), "{e}");
    // reuse mode: a logical partition cannot become the boot or data partition
    let e = build_plan(&m, &Request { mode: Mode::Reuse, boot_part: Some(5), data_part: Some(6), ..Request::default() }, &spec(&s), true).unwrap_err();
    assert!(e.contains("logical partition"), "{e}");
}

// ---------------------------------------------------------------------------------------------------------------------------
// the raw-terminal loop on a real pseudo-terminal: the test runs this test binary again as the installer, with a pty as its
// terminal, and types at it (arrow keys, Esc, Tab, Enter) like a person would.

/// Not a test of its own: the child half of the pty test below. It does nothing unless the parent set the folder.
#[test]
fn pty_child_the_installer_pages_on_the_terminal_it_was_given() {
    let Ok(dir) = std::env::var("IYAKKI_PTY_DIR") else { return };
    let dir = PathBuf::from(dir);
    let machine = crate::plan_cli::Machine { sys: "/nonexistent".into(), dev: "/nonexistent".into(), proc_mounts: String::new(), swaps: String::new() };
    let args: Vec<String> = ["--plan", "--target-file", dir.join("p.img").to_str().unwrap(), dir.join("t.img").to_str().unwrap()].iter().map(|s| s.to_string()).collect();
    let code = crate::plan_cli::run_inner(&args, &machine, false).unwrap_or(99);
    std::process::exit(code);
}

struct Pty {
    master: std::fs::File,
    child: std::process::Child,
    seen: String,
    at: usize,
}

impl Pty {
    fn start(dir: &Path) -> Pty {
        use std::os::fd::FromRawFd;
        let (mut m, mut s) = (0, 0);
        let ws = libc::winsize { ws_row: 40, ws_col: 100, ws_xpixel: 0, ws_ypixel: 0 };
        // SAFETY: openpty fills two descriptors; the winsize outlives the call.
        assert_eq!(unsafe { libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null_mut(), &ws as *const libc::winsize as *mut libc::winsize) }, 0);
        // SAFETY: both descriptors were just made and are owned here; the slave is duplicated for the child's three streams.
        let (master, slave) = unsafe { (std::fs::File::from_raw_fd(m), std::fs::File::from_raw_fd(s)) };
        let stdio = || std::process::Stdio::from(slave.try_clone().unwrap());
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "planner_tests::pty_child_the_installer_pages_on_the_terminal_it_was_given", "--nocapture", "--test-threads=1"])
            .env("IYAKKI_PTY_DIR", dir)
            .stdin(stdio())
            .stdout(stdio())
            .stderr(stdio())
            .spawn()
            .unwrap();
        drop(slave);
        Pty { master, child, seen: String::new(), at: 0 }
    }

    fn type_keys(&mut self, bytes: &[u8]) {
        use std::io::Write;
        self.master.write_all(bytes).unwrap();
    }

    /// Read until `needle` shows up after the last match; the whole transcript is in the failure message.
    fn expect(&mut self, needle: &str) {
        use std::io::Read;
        use std::os::fd::AsRawFd;
        let t0 = std::time::Instant::now();
        loop {
            if let Some(p) = self.seen[self.at..].find(needle) {
                self.at += p + needle.len();
                return;
            }
            assert!(t0.elapsed() < std::time::Duration::from_secs(60), "waited for {needle:?}; the terminal showed: {}", self.seen[self.at..].replace('\x1b', "<ESC>"));
            let mut fds = libc::pollfd { fd: self.master.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            // SAFETY: one valid pollfd for the duration of the call.
            if unsafe { libc::poll(&mut fds, 1, 100) } > 0 {
                let mut buf = [0u8; 8192];
                match self.master.read(&mut buf) {
                    Ok(n) if n > 0 => self.seen.push_str(&String::from_utf8_lossy(&buf[..n])),
                    _ => {}
                }
            }
        }
    }

    fn finish(self) -> i32 {
        self.finish_seen().0
    }

    fn finish_seen(mut self) -> (i32, String) {
        let t0 = std::time::Instant::now();
        loop {
            // keep the pty drained so the child never blocks on a full buffer
            use std::io::Read;
            use std::os::fd::AsRawFd;
            let mut fds = libc::pollfd { fd: self.master.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            // SAFETY: one valid pollfd for the duration of the call.
            if unsafe { libc::poll(&mut fds, 1, 50) } > 0 {
                let mut buf = [0u8; 8192];
                if let Ok(n) = self.master.read(&mut buf) {
                    self.seen.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            }
            if let Some(st) = self.child.try_wait().unwrap() {
                return (st.code().unwrap_or(-1), self.seen);
            }
            assert!(t0.elapsed() < std::time::Duration::from_secs(60), "the installer did not end; it showed: {}", self.seen.replace('\x1b', "<ESC>"));
        }
    }
}

#[test]
fn the_terminal_loop_on_a_real_pseudo_terminal_reads_arrows_and_esc_cancels_with_nothing_written_and_a_confirmed_install_writes() {
    let (dir, _guard) = scratch("pty");
    src(&dir);
    let (path, _) = gpt_disk(&dir, "p.img", 3 * GIB, 512, &[ps(1, 500, MSDATA, "other", Fill::Ntfs)]);
    let h0 = whole_file_hash(&path);

    // first run: walk to the confirmation, go back with Esc, cancel with Tab and Enter; the drive is untouched and the terminal is given back
    let mut t = Pty::start(&dir);
    t.expect("step 1 of 6");
    t.expect("testfile");
    t.type_keys(b"\r"); // the drive
    t.expect("step 2 of 6");
    t.expect("Free space only");
    t.type_keys(b"\x1b[A"); // arrow up: the mode above the free-space default
    t.expect("> Whole disk");
    t.type_keys(b"\x1b[B"); // and arrow down: back on it
    t.expect("> Free space only");
    t.type_keys(b"\r");
    t.expect("step 3 of 6");
    t.expect("free space at");
    t.type_keys(b"\r");
    t.expect("step 4 of 6");
    t.expect("nothing is written yet");
    t.type_keys(b"\r");
    t.expect("step 5 of 6");
    t.expect("Install");
    t.type_keys(b"\x1b"); // a lone Esc goes back one page (the loop waits briefly to tell it from an arrow)
    t.expect("step 4 of 6");
    t.type_keys(b"\r");
    t.expect("step 5 of 6");
    t.type_keys(b"\t");
    std::thread::sleep(std::time::Duration::from_millis(150));
    t.type_keys(b"\t");
    std::thread::sleep(std::time::Duration::from_millis(150));
    t.type_keys(b"\r"); // Cancel
    let code = t.finish_seen();
    assert_eq!(code.0, 0, "cancelling is not an error");
    assert!(code.1.contains("\x1b[?25l") && code.1.contains("\x1b[?25h"), "the cursor was hidden for the pages and given back");
    assert_eq!(whole_file_hash(&path), h0, "cancelling wrote nothing");

    // second run: the same walk, then Install; the write runs through the loop and the done page ends it
    let mut t = Pty::start(&dir);
    t.expect("testfile");
    for step in 2..=5 {
        t.type_keys(b"\r");
        t.expect(&format!("step {step} of 6"));
    }
    t.expect("Install");
    t.type_keys(b"\r");
    t.expect("written and verified");
    t.type_keys(b"\r"); // the done page
    assert_eq!(t.finish(), 0);
    assert_ne!(whole_file_hash(&path), h0, "the confirmed install wrote the drive");
}

// ---- legacy BIOS boot of a shared disk (F-INST-13), end to end on image files

#[test]
fn the_pages_offer_legacy_bios_boot_for_a_shared_disk_and_choose_a_fresh_save_file() {
    let (dir, _guard) = scratch("wiz-bios");
    let s = src(&dir);
    let path = mbr_disk(&dir, "wb.img", 4 * GIB, &[(1, 1000, 0x07, Fill::Ntfs)]);
    let before = whole_file_hash(&path);
    let m = model_of(&path, "sda", 4 * GIB, 512, "", true);
    let mut w = Wizard::new(vec![m], spec(&s), true);
    w.bios_save_dir = dir.clone();
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Mode);
    assert_eq!(w.mode(), Mode::FreeSpace);
    // off by default; l turns it on, r allows replacing another system's boot code (only while it is on)
    w.handle(WKey::Char('r'));
    assert!(!w.req.bios_replace, "r does nothing while legacy BIOS boot is off");
    assert!(w.render(100).contains("l (off)"));
    w.handle(WKey::Char('l'));
    w.handle(WKey::Char('r'));
    assert!(w.req.bios && w.req.bios_replace);
    assert!(w.render(100).contains("l (ON)") && w.render(100).contains("(ON)"));
    // whole-disk mode sets BIOS boot up on its own: l is ignored there
    w.mode_sel = 0;
    assert_eq!(w.mode(), Mode::WholeDisk);
    w.handle(WKey::Char('l'));
    assert!(w.req.bios, "l is ignored in whole-disk mode");
    w.mode_sel = 1;
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Choose);
    w.handle(WKey::Enter);
    assert_eq!(w.page, Page::Plan, "{:?}", w.error);
    let plan = w.plan.clone().unwrap();
    assert!(matches!(plan.bios, Some(crate::biosshared::Method::MbrGap { .. })), "{:?}", plan.bios);
    let save = std::path::PathBuf::from(&plan.request.bios_save);
    assert!(save.starts_with(&dir) && save.file_name().unwrap().to_str().unwrap().starts_with("iyakki-bios-save-sda-"), "{}", save.display());
    assert!(!save.exists(), "the pages write nothing, not even the save file");
    let text = w.render(100);
    assert!(text.contains("Legacy BIOS boot") && text.contains("iyakki-bios-save-sda-"), "{text}");
    // a name that exists is never chosen again
    std::fs::write(&save, b"x").unwrap();
    assert_ne!(w.fresh_bios_save(), save.display().to_string());
    // turning it off clears the permission; without the permission a foreign boot code refuses the plan in words
    w.handle(WKey::Esc);
    w.handle(WKey::Esc);
    w.handle(WKey::Char('r'));
    assert!(!w.req.bios_replace);
    w.handle(WKey::Enter);
    w.handle(WKey::Enter);
    assert!(w.error.as_deref().is_some_and(|e| e.contains("boot code")), "{:?}", w.error);
    assert_eq!(whole_file_hash(&path), before, "the pages never write the disk");
}

fn bios_req(dir: &Path, name: &str) -> Request {
    Request { bios: true, bios_replace: true, bios_save: dir.join(name).display().to_string(), ..free_req() }
}

#[test]
fn the_bios_option_saves_first_writes_grub_behind_sector_0_keeps_the_table_and_can_be_undone() {
    let (dir, _guard) = scratch("bios-apply");
    let s = src(&dir);
    let path = mbr_disk(&dir, "b.img", 4 * GIB, &[(1, 1000, 0x07, Fill::Ntfs)]);
    let s0 = bytes_at(&path, 0, 512);
    let windows = bytes_at(&path, MIB, 4096);
    let m = model_of(&path, "sda", 4 * GIB, 512, "", true);
    let req = bios_req(&dir, "save.bin");
    // without the explicit permission another system's boot code is not replaced
    let denied = build_plan(&m, &Request { bios_replace: false, ..req.clone() }, &spec(&s), true).unwrap_err();
    assert!(denied.contains("boot code") && denied.contains("explicitly"), "{denied}");
    let plan = build_plan(&m, &req, &spec(&s), true).unwrap();
    assert!(matches!(plan.bios, Some(crate::biosshared::Method::MbrGap { core_lba: 1, .. })), "{:?}", plan.bios);
    let text = plan.render();
    assert!(text.contains("Legacy BIOS boot: GRUB goes into the gap behind sector 0") && text.contains("save.bin"), "{text}");
    assert!(!text.contains("is not set up: IYAKKI starts through UEFI"), "the old warning goes away: {text}");
    assert!(plan.warnings.iter().any(|w| w.contains("chainload")), "{:?}", plan.warnings);
    // the plan page and the dry run write nothing, not even the save file
    run_apply(&path, &m, &req, &s, Some(&plan.digest()), &ApplyEnv { dry_run: true, ..Default::default() }).unwrap();
    assert!(!dir.join("save.bin").exists());
    let rep = run_apply(&path, &m, &req, &s, Some(&plan.digest()), &ApplyEnv::default()).unwrap();
    assert!(rep.notes.iter().any(|n| n.contains("legacy BIOS boot set up") && n.contains("save.bin")), "{:?}", rep.notes);
    // GRUB's boot code is in sector 0 with the address of the core image; the disk signature, the table and 55 AA are as they were or as
    // the install added to them
    let n0 = bytes_at(&path, 0, 512);
    let bits = crate::gpt_install::grub_bits_used(&s, 1).unwrap();
    assert_eq!(&n0[..0x5C], &bits.boot_code[..0x5C]);
    assert_eq!(u64::from_le_bytes(n0[0x5C..0x64].try_into().unwrap()), 1);
    assert_eq!(&n0[0x64..440], &bits.boot_code[0x64..440]);
    assert_eq!(&n0[440..446], &s0[440..446], "the disk signature");
    assert_eq!(&n0[446..462], &s0[446..462], "the other system's partition entry");
    assert_eq!(&n0[510..], &[0x55, 0xAA]);
    assert_eq!(bytes_at(&path, 512, bits.core.len()), bits.core, "the core image behind sector 0");
    assert_eq!(bytes_at(&path, MIB, 4096), windows, "the other system's partition is untouched");
    // the save file holds the sectors as they were just before the write, and puts them back
    let save = crate::biosshared::Backup::decode(&std::fs::read(dir.join("save.bin")).unwrap()).unwrap();
    assert_eq!(save.regions[0].1.len(), 512);
    assert_eq!(&save.regions[0].1[..440], &s0[..440], "the old boot code");
    {
        let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        let mut w: &std::fs::File = &f;
        save.restore(&mut w, 512, 4 * GIB).unwrap();
        let _ = f.sync_all();
    }
    let back = bytes_at(&path, 0, 512);
    assert_eq!(&back[..446], &s0[..446], "the old boot code is back");
    assert_eq!(&back[446..462], &s0[446..462]);
    assert_ne!(back[478 + 4], 0, "and the install's own partition entries are still there");
    assert!(bytes_at(&path, 512, 64).iter().all(|b| *b == 0), "the gap is empty again");
}

#[test]
fn the_bios_option_refuses_instead_of_skipping_when_it_cannot_be_done() {
    let (dir, _guard) = scratch("bios-refuse");
    let s = src(&dir);
    let ok_disk = mbr_disk(&dir, "ok.img", 4 * GIB, &[(1, 1000, 0x07, Fill::Ntfs)]);
    let m = model_of(&ok_disk, "sda", 4 * GIB, 512, "", true);
    let base = bios_req(&dir, "never.bin");
    // the save file: needed, absolute, not existing, in an existing folder
    for (save, want) in [("", "--bios-save"), ("relative.bin", "--bios-save"), ("/no/such/folder/x.bin", "does not exist")] {
        let e = build_plan(&m, &Request { bios_save: save.into(), ..base.clone() }, &spec(&s), true).unwrap_err();
        assert!(e.contains(want), "{save}: {e}");
    }
    std::fs::write(dir.join("exists.bin"), b"x").unwrap();
    let e = build_plan(&m, &Request { bios_save: dir.join("exists.bin").display().to_string(), ..base.clone() }, &spec(&s), true).unwrap_err();
    assert!(e.contains("never overwritten"), "{e}");
    // an image that carries no GRUB core
    let e = build_plan(&m, &base, &SourceSpec { core_bytes: 0, ..spec(&s) }, true).unwrap_err();
    assert!(e.contains("no GRUB core"), "{e}");
    // a gap that is used, a gap that is too small
    let used = mbr_disk(&dir, "used.img", 4 * GIB, &[(1, 1000, 0x07, Fill::Ntfs)]);
    {
        use std::os::unix::fs::FileExt;
        std::fs::OpenOptions::new().write(true).open(&used).unwrap().write_all_at(&[7u8; 64], 512 + 100).unwrap();
    }
    let e = build_plan(&model_of(&used, "sda", 4 * GIB, 512, "", true), &base, &spec(&s), true).unwrap_err();
    assert!(e.contains("not empty"), "{e}");
    // a 4096-byte-sector disk and a GPT disk without a BIOS boot partition
    let (g, _) = gpt_disk(&dir, "g.img", 3 * GIB, 512, &[ps(1, 1024, MSDATA, "other", Fill::Ntfs)]);
    // (a free-space install makes the BIOS boot partition itself, so it is planned, not refused: see
    // `the_bios_option_makes_the_bios_boot_partition_of_a_gpt_disk_with_a_kept_esp_and_boots_nothing_else_changes`)
    let plan = build_plan(&model_of(&g, "sda", 3 * GIB, 512, "", true), &base, &spec(&s), true).unwrap();
    assert!(matches!(plan.bios, Some(crate::biosshared::Method::BiosBootPartition { .. })), "{:?}", plan.bios);
    // nothing was written by any of them
    assert!(!dir.join("never.bin").exists());
}

#[test]
fn the_bios_option_is_kept_in_the_plan_file() {
    let r = Request { bios: true, bios_replace: true, bios_save: "/mnt/other/save.bin".into(), ..free_req() };
    let back = Request::from_json(&r.to_json()).unwrap();
    assert_eq!((back.bios, back.bios_replace, back.bios_save.as_str()), (true, true, "/mnt/other/save.bin"));
    assert!(!Request::from_json(&free_req().to_json()).unwrap().bios);
}

#[test]
fn the_bios_option_makes_the_bios_boot_partition_of_a_gpt_disk_with_a_kept_esp_and_boots_nothing_else_changes() {
    let (dir, _guard) = scratch("bios-gpt-new");
    let s = src(&dir);
    let (path, old) = gpt_disk(&dir, "kb.img", 4 * GIB, 512, &[ps(1, 100, EFI, "EFI system partition", Fill::Raw(foreign_esp(&dir))), ps(101, 1000, MSDATA, "windows", Fill::Ntfs)]);
    let before = old_partition_bytes(&path, &old, 512);
    let m = model_of(&path, "sda", 4 * GIB, 512, "", false);
    let req = Request { keep_esp: true, ..bios_req(&dir, "gsave.bin") };
    let plan = build_plan(&m, &req, &spec(&s), true).unwrap();
    let bios_step = plan.steps.iter().find(|st| st.role == Some(Role::Bios)).expect("a BIOS boot partition is planned");
    assert_eq!((bios_step.action, bios_step.bytes), (Action::Create, MIB));
    assert!(matches!(plan.bios, Some(crate::biosshared::Method::BiosBootPartition { start_lba, .. }) if start_lba == bios_step.first_lba), "{:?}", plan.bios);
    // without --bios nothing is added when the ESP is kept
    let plain = build_plan(&m, &Request { keep_esp: true, ..free_req() }, &spec(&s), true).unwrap();
    assert!(plain.steps.iter().all(|st| st.role != Some(Role::Bios)));
    // the old partitions are untouched by the plan
    for st in plan.steps.iter().filter(|st| st.action == Action::Create) {
        assert!(old.entries.iter().all(|e| st.first_lba > e.last_lba || st.first_lba + st.sectors <= e.first_lba), "a new partition overlaps an old one");
    }
    let rep = run_apply(&path, &m, &req, &s, Some(&plan.digest()), &ApplyEnv::default()).unwrap();
    assert!(rep.notes.iter().any(|n| n.contains("legacy BIOS boot set up")), "{:?}", rep.notes);
    let p = parsed(&path, 512, 4 * GIB);
    assert!(p.primary_ok && p.backup_ok);
    let e = p.table.entries.iter().find(|e| e.type_guid == gpt::TYPE_BIOS_BOOT).expect("the partition is in the table");
    let bits = crate::gpt_install::grub_bits_used(&s, e.first_lba).unwrap();
    assert_eq!(bytes_at(&path, e.first_lba * 512, bits.core.len()), bits.core, "GRUB's core image is at the start of the new partition");
    let now = old_partition_bytes(&path, &old, 512);
    assert_eq!(now[1], before[1], "the other system's partition is untouched");
}

// ---- iyakki-usb --update: the boot files of a stick replaced without mounting it (F-UPG-05) ----

/// A stick whose boot volume holds `boot/KERNEL.BIN`, `boot/INITRD.GZ`, `boot/KEEP.TXT` and the loader `EFI/BOOT/BOOTX64.EFI`
/// (the other system's `EFI/MSFT/BOOT/BOOTMGFW.EFI` is there too); returns the image path.
fn stick_with_boot_files(dir: &Path) -> PathBuf {
    let img = foreign_esp(dir);
    let p = dir.join("stick.img");
    std::fs::write(&p, &img).unwrap();
    let f = std::fs::OpenOptions::new().read(true).write(true).open(&p).unwrap();
    {
        let mut rw = Fat32Rw::open(&f, 0, img.len() as u64).unwrap();
        let (d, _) = rw.mkdir_p(&["boot"]).unwrap();
        for (n, len, seed) in [("KERNEL.BIN", 5000usize, 1u8), ("INITRD.GZ", 9000, 2), ("KEEP.TXT", 40, 3)] {
            rw.create_file(d, n, len as u64, &mut std::io::Cursor::new(pseudo(len, seed)), &mut |_| {}).unwrap();
        }
        rw.flush().unwrap();
        rw.write_fsinfo().unwrap();
    }
    p
}

fn update_folder(dir: &Path) -> PathBuf {
    let from = dir.join("pack");
    for (rel, len, seed) in [("boot/KERNEL.BIN", 7000usize, 11u8), ("boot/INITRD.GZ", 12_000, 12), ("boot/NEW.CFG", 30, 13), ("EFI/BOOT/BOOTX64.EFI", 130_000, 14), ("README.txt", 5, 15), ("other/x.bin", 5, 16)] {
        let p = from.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, pseudo(len, seed)).unwrap();
    }
    from
}

fn on_stick(p: &Path, path: &str) -> Option<Vec<u8>> {
    let mut h = std::fs::File::open(p).unwrap();
    let mut vol = Fat32::open(&mut h, 0).unwrap();
    let items = vol.walk_long().unwrap();
    let it = items.iter().find(|i| !i.dir && i.path.trim_start_matches('/').eq_ignore_ascii_case(path))?.clone();
    Some(vol.read_file(&it).unwrap())
}

#[test]
fn an_update_folder_is_planned_boot_files_first_and_loaders_last_and_planning_writes_nothing() {
    use crate::stickupdate::{plan, Kind};
    let (dir, _guard) = scratch("upd-plan");
    let stick = stick_with_boot_files(&dir);
    let from = update_folder(&dir);
    let before = std::fs::read(&stick).unwrap();
    let p = plan(&stick, &from).unwrap();
    let names: Vec<(&str, &Kind)> = p.steps.iter().map(|s| (s.path.as_str(), &s.kind)).collect();
    assert_eq!(names, vec![("boot/INITRD.GZ", &Kind::Replace), ("boot/KERNEL.BIN", &Kind::Replace), ("boot/NEW.CFG", &Kind::Add), ("EFI/BOOT/BOOTX64.EFI", &Kind::Replace)], "biggest boot file first, the loader last");
    assert_eq!(p.skipped, vec!["README.txt".to_string(), "other/x.bin".to_string()], "what is not under boot/ or EFI/ is not part of an update");
    assert_eq!(std::fs::read(&stick).unwrap(), before, "planning wrote nothing");
}

#[test]
fn applying_replaces_the_boot_files_reads_them_back_and_leaves_everything_else_as_it_was() {
    use crate::stickupdate::{apply, plan};
    let (dir, _guard) = scratch("upd-apply");
    let stick = stick_with_boot_files(&dir);
    let from = update_folder(&dir);
    let msft = on_stick(&stick, "EFI/MSFT/BOOT/BOOTMGFW.EFI").unwrap();
    let keep = on_stick(&stick, "boot/KEEP.TXT").unwrap();
    let p = plan(&stick, &from).unwrap();
    let mut told = Vec::new();
    apply(&stick, &from, &p, &mut |i, n, path| told.push(format!("{}/{n} {path}", i + 1))).unwrap();
    assert_eq!(told.len(), 4);
    assert!(told[3].ends_with("EFI/BOOT/BOOTX64.EFI"), "{told:?}");
    for (rel, len, seed) in [("boot/KERNEL.BIN", 7000usize, 11u8), ("boot/INITRD.GZ", 12_000, 12), ("boot/NEW.CFG", 30, 13), ("EFI/BOOT/BOOTX64.EFI", 130_000, 14)] {
        assert_eq!(on_stick(&stick, rel).unwrap(), pseudo(len, seed), "{rel}");
    }
    assert_eq!(on_stick(&stick, "EFI/MSFT/BOOT/BOOTMGFW.EFI").unwrap(), msft, "the other system's loader is untouched");
    assert_eq!(on_stick(&stick, "boot/KEEP.TXT").unwrap(), keep, "a boot file that is not in the update stays");
    assert!(on_stick(&stick, "README.txt").is_none() && on_stick(&stick, "other/x.bin").is_none());
    if let Some((ok, out)) = tool("fsck.vfat", &["-n", stick.to_str().unwrap()]) {
        assert!(ok, "fsck.vfat:\n{out}");
    }
}

#[test]
fn an_update_that_cannot_be_written_whole_is_refused_before_anything_is_written() {
    use crate::stickupdate::{plan, read_folder};
    let (dir, _guard) = scratch("upd-refuse");
    let stick = stick_with_boot_files(&dir);
    let from = update_folder(&dir);
    let before = std::fs::read(&stick).unwrap();
    // a new file with a long name is created by the writer (long-name entries and a short alias): the plan holds it
    std::fs::write(from.join("boot/vmlinuz-6.12.112"), b"x").unwrap();
    assert!(plan(&stick, &from).unwrap().steps.iter().any(|s| s.path == "boot/vmlinuz-6.12.112"));
    std::fs::remove_file(from.join("boot/vmlinuz-6.12.112")).unwrap();
    // a name with a space is still refused whole, before anything is written
    std::fs::write(from.join("boot/two words.bin"), b"x").unwrap();
    let e = plan(&stick, &from).map(|_| String::new()).unwrap_or_else(|e| e);
    assert!(e.contains("two words.bin") && (e.contains("Nothing was written") || e.contains("not safe")), "{e}");
    std::fs::remove_file(from.join("boot/two words.bin")).unwrap();
    // a link, an unsafe name and an empty folder are refused
    std::os::unix::fs::symlink("/etc/passwd", from.join("boot/LINK.BIN")).unwrap();
    assert!(plan(&stick, &from).unwrap_err().contains("link"));
    std::fs::remove_file(from.join("boot/LINK.BIN")).unwrap();
    std::fs::write(from.join("boot/bad;name"), b"x").unwrap();
    assert!(plan(&stick, &from).unwrap_err().contains("not safe"));
    std::fs::remove_file(from.join("boot/bad;name")).unwrap();
    let empty = dir.join("empty");
    std::fs::create_dir_all(empty.join("misc")).unwrap();
    std::fs::write(empty.join("misc/a"), b"x").unwrap();
    assert!(read_folder(&empty).unwrap_err().contains("not an update"));
    // not a disk with a FAT32 boot volume
    let junk = dir.join("junk.img");
    std::fs::write(&junk, vec![0u8; 8192]).unwrap();
    assert!(plan(&junk, &from).is_err());
    assert_eq!(std::fs::read(&stick).unwrap(), before);
}

#[test]
fn the_command_shows_the_plan_and_writes_only_with_yes() {
    let (dir, _guard) = scratch("upd-cli");
    let stick = stick_with_boot_files(&dir);
    let from = update_folder(&dir);
    let before = std::fs::read(&stick).unwrap();
    let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let (s, f) = (stick.to_str().unwrap(), from.to_str().unwrap());
    assert_eq!(crate::stickupdate::run(&a(&["--update", s])), 2, "a folder is needed");
    assert_eq!(crate::stickupdate::run(&a(&["--update", s, "--from", f])), 0);
    assert_eq!(std::fs::read(&stick).unwrap(), before, "without --yes nothing is written");
    // the folder has no update.json.sig: refused with --yes alone, nothing written
    assert_eq!(crate::stickupdate::run(&a(&["--update", s, "--from", f, "--yes"])), 1);
    assert_eq!(std::fs::read(&stick).unwrap(), before, "an unsigned pack is not written without --allow-unsigned");
    std::fs::write(from.join("update.json"), b"{}").unwrap();
    std::fs::write(from.join("update.json.sig"), "00".repeat(64)).unwrap();
    assert_eq!(crate::stickupdate::run(&a(&["--update", s, "--from", f, "--yes", "--allow-unsigned"])), 1, "a wrong signature is refused even with the flag");
    assert_eq!(std::fs::read(&stick).unwrap(), before);
    std::fs::remove_file(from.join("update.json.sig")).unwrap();
    std::fs::remove_file(from.join("update.json")).unwrap();
    assert_eq!(crate::stickupdate::run(&a(&["--update", s, "--from", f, "--yes", "--allow-unsigned"])), 0);
    assert_eq!(on_stick(&stick, "boot/KERNEL.BIN").unwrap(), pseudo(7000, 11));
    assert_eq!(crate::stickupdate::run(&a(&["--update", "/nonexistent/stick", "--from", f, "--yes"])), 1);
}

#[test]
fn free_space_inside_an_extended_partition_is_named_but_not_offered_and_a_plan_that_needs_it_says_so() {
    let (dir, _guard) = scratch("logicals-inner");
    let s = src(&dir);
    // the container (1500 MiB at 1001) holds logicals at 1002..1402 and 1502..1902; the rest of it is free inside
    let path = mbr_with_logicals(&dir, "l.img");
    let m = model_of(&path, "sdb", 4 * GIB, 512, "", true);
    let spans: Vec<(u64, u64)> = m.container_gaps().iter().map(|g| (g.first_lba * 512 / MIB, (g.first_lba + g.sectors) * 512 / MIB)).collect();
    // 1402..1501 (the record of the second one takes 1501..1502) and 1902..2501
    assert_eq!(spans, vec![(1402, 1501), (1902, 2501)]);
    assert!(m.container_gaps().iter().all(|g| m.free_gaps(TableKind::Mbr).iter().all(|f| f.first_lba != g.first_lba)));
    // a disk with the same container but no room after it: the refusal names the room inside
    let tight = mbr_disk(&dir, "t.img", 2501 * MIB, &[(1, 1000, 0x07, Fill::Ntfs), (1001, 1500, 0x0F, Fill::Blank)]);
    let mt = model_of(&tight, "sdb", 2501 * MIB, 512, "", true);
    assert!(mt.free_gaps(TableKind::Mbr).is_empty());
    let e = build_plan(&mt, &free_req(), &spec(&s), true).unwrap_err();
    assert!(e.contains("no free space"), "{e}");
    // the container here has no logical partitions at all: its whole inside is free, and the message says where it is
    assert!(e.contains("inside an extended partition") && e.contains("the installer does not use it"), "{e}");
}

// ---- legacy BIOS boot on 4096-byte sectors (F-INST-29)

fn code_4k(dir: &Path, name: &str, stock: bool) -> String {
    let mut c = vec![0x90u8; 440];
    if !stock {
        c[424..].copy_from_slice(crate::bios4kn::MARKER);
    }
    let p = dir.join(name);
    std::fs::write(&p, &c).unwrap();
    p.display().to_string()
}

#[test]
fn bios_on_4096_byte_sectors_is_refused_in_words_without_the_4k_boot_code_and_planned_with_it_and_a_bios_boot_partition() {
    let (dir, _guard) = scratch("bios-4k");
    let s = src(&dir);
    let bios_part = ps(1, 1, gpt::TYPE_BIOS_BOOT, "bios", Fill::Blank);
    let (path, _) = gpt_disk(&dir, "k.img", 3 * GIB, 4096, &[bios_part, ps(2, 1024, MSDATA, "other", Fill::Ntfs)]);
    let m = model_of(&path, "nvme1n1", 3 * GIB, 4096, "", false);
    let base = Request { bios: true, bios_replace: true, bios_save: dir.join("ksave.bin").display().to_string(), ..free_req() };
    // no code file: the plain refusal
    let e = build_plan(&m, &base, &spec(&s), true).unwrap_err();
    assert!(e.contains("4096-byte-sector boot code") && e.contains("UEFI"), "{e}");
    // GRUB's stock code does not say it reads 4096-byte sectors
    let e = build_plan(&m, &Request { bios_4k_code: code_4k(&dir, "stock.bin", true), ..base.clone() }, &spec(&s), true).unwrap_err();
    assert!(e.contains("512-byte sectors"), "{e}");
    // our code and an empty BIOS boot partition: planned
    let plan = build_plan(&m, &Request { bios_4k_code: code_4k(&dir, "ours.bin", false), ..base.clone() }, &spec(&s), true).unwrap();
    assert!(matches!(plan.bios, Some(crate::biosshared::Method::BiosBootPartition { .. })), "{:?}", plan.bios);
    // the code file's name is kept in the plan file
    let r = Request { bios_4k_code: "/x/4k.bin".into(), ..base };
    assert_eq!(Request::from_json(&r.to_json()).unwrap().bios_4k_code, "/x/4k.bin");
    assert!(!dir.join("ksave.bin").exists(), "planning writes nothing");
    // applied to the image file: sector 0 names the core image in 4096-byte units, the core image has its block list rewritten, the save undoes it
    let req = Request { bios_4k_code: code_4k(&dir, "ours2.bin", false), ..Request { bios: true, bios_replace: true, bios_save: dir.join("ksave.bin").display().to_string(), ..free_req() } };
    let plan = build_plan(&m, &req, &spec(&s), true).unwrap();
    let start = plan.bios.unwrap().core_lba();
    let rep = run_apply(&path, &m, &req, &s, Some(&plan.digest()), &ApplyEnv::default()).unwrap();
    assert!(rep.notes.iter().any(|n| n.contains("4096-byte sectors") && n.contains("ksave.bin")), "{:?}", rep.notes);
    let s0 = bytes_at(&path, 0, 4096);
    assert_eq!(u64::from_le_bytes(s0[0x5C..0x64].try_into().unwrap()), start);
    let first = bytes_at(&path, start * 4096, 4096);
    assert_eq!(u64::from_le_bytes(first[500..508].try_into().unwrap()), start + 1);
    assert_eq!(u16::from_le_bytes(first[510..512].try_into().unwrap()) % 0x100, 0, "the load segment follows a whole 4096-byte sector");
    crate::biosshared::restore_file(&dir.join("ksave.bin"), &path).unwrap();
    assert!(bytes_at(&path, start * 4096, 4096).iter().all(|b| *b == 0), "the BIOS boot partition is empty again");
}

#[test]
fn prepare_and_write_prepared_plan_without_a_screen_and_write_only_after_the_drive_name_with_the_builtin_4k_code() {
    use crate::plan_cli::{prepare, write_prepared, Machine};
    let (dir, _guard) = scratch("bios-4k-prepared");
    let s = src(&dir);
    let bios_part = ps(1, 1, gpt::TYPE_BIOS_BOOT, "bios", Fill::Blank);
    let (path, _) = gpt_disk(&dir, "k.img", 3 * GIB, 4096, &[bios_part, ps(2, 1024, MSDATA, "other", Fill::Ntfs)]);
    let machine = Machine { sys: "/nonexistent".into(), dev: "/nonexistent".into(), proc_mounts: String::new(), swaps: String::new() };
    let save = dir.join("psave.bin").display().to_string();
    let (p, t) = (path.display().to_string(), s.path.display().to_string());
    let args: Vec<String> = ["--plan", "--disk", "testfile", "--mode", "free-space", "--bios", "--bios-replace-boot-code", "--bios-4k-boot-code", "builtin", "--bios-save", &save, "--sector-size", "4096", "--target-file", &p, "--template", &t].iter().map(|x| x.to_string()).collect();
    let h0 = whole_file_hash(&path);
    let prep = prepare(&args, &machine).unwrap();
    assert!(matches!(prep.plan.bios, Some(crate::biosshared::Method::BiosBootPartition { .. })), "{:?}", prep.plan.bios);
    assert_eq!(whole_file_hash(&path), h0, "planning writes nothing");
    let e = write_prepared(&prep, &machine, "sda", &mut |_| {}).err().unwrap();
    assert!(e.contains("nothing was written"), "{e}");
    assert_eq!(whole_file_hash(&path), h0);
    let rep = write_prepared(&prep, &machine, "testfile", &mut |_| {}).unwrap();
    assert!(rep.notes.iter().any(|n| n.contains("4096-byte sectors")), "{:?}", rep.notes);
    assert_ne!(whole_file_hash(&path), h0);
    // without the 4096-byte-sector boot code the planner refuses in words
    let (path2, _) = gpt_disk(&dir, "k2.img", 3 * GIB, 4096, &[ps(1, 1, gpt::TYPE_BIOS_BOOT, "bios", Fill::Blank), ps(2, 1024, MSDATA, "other", Fill::Ntfs)]);
    let mut a2 = args.clone();
    let i = a2.iter().position(|x| x == "--bios-4k-boot-code").unwrap();
    a2.drain(i..i + 2);
    let k = a2.iter().position(|x| x == &p).unwrap();
    a2[k] = path2.display().to_string();
    let j = a2.iter().position(|x| x == &save).unwrap();
    a2[j] = dir.join("psave2.bin").display().to_string();
    let e = prepare(&a2, &machine).err().unwrap();
    assert!(e.contains("4096-byte-sector boot code"), "{e}");
}

/// F-INST-12: the Windows disk (`windisk::DiskCore` over the sector-aligned Win32 calls, here a file standing in for the drive) gives the
/// planner's readers exactly what a plain file gives them, for both sector sizes, and a table-sized write at odd offsets lands exactly.
#[test]
fn the_windows_disk_reads_tables_and_models_like_a_file_and_takes_unaligned_writes() {
    use crate::windisk::DiskCore;
    use crate::windows::FileDisk;
    for ss in [512u32, 4096] {
        let (dir, _guard) = scratch(&format!("windisk-{ss}"));
        let size = 3 * GIB;
        let (path, old) = gpt_disk(&dir, "w.img", size, ss, &[ps(1, 100, EFI, "EFI system partition", Fill::Raw(foreign_esp(&dir))), ps(101, 16, MSR, "Microsoft reserved", Fill::Blank), ps(117, 1024, MSDATA, "Basic data partition", Fill::Ntfs)]);
        let file = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        let fd = FileDisk { file, elevated: true, sector: ss as u64, events: vec![] };
        let mut core = DiskCore::open(fd, r"\\.\PhysicalDrive7", ss, size, &[r"E:\".to_string()]).unwrap();
        let via_file = parsed(&path, ss, size);
        let via_disk = gpt::read_gpt(&mut core, ss, size / ss as u64).unwrap();
        assert_eq!(via_disk.table, via_file.table);
        assert_eq!(via_disk.table, old);
        assert!(via_disk.primary_ok && via_disk.backup_ok);
        // the planner's own model of the disk is the same
        let meta = DiskMeta { name: "PhysicalDrive7".into(), path: r"\\.\PhysicalDrive7".into(), ..Default::default() };
        let mut f = std::fs::File::open(&path).unwrap();
        let a = read_model(&mut f, &meta, size, ss, &[], None);
        let b = read_model(&mut core, &meta, size, ss, &[], None);
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.parts.len(), b.parts.len());
        // pieces of a table's size at odd offsets through the disk are what a positional write puts in the file
        let piece = pseudo(5000, 3);
        let at = 117 * MIB + 3;
        core.disk.write_at(at, &piece).unwrap();
        drop(core);
        assert_eq!(bytes_at(&path, at, piece.len()), piece);
    }
}
