//! Tests of the GPT install on plain files that stand in for disks (never a real device): the layout maths, the bytes written,
//! a round trip through our own reader, the real tools (`sfdisk`, `sgdisk`, `gdisk`, `parted`) where they are installed,
//! 4096-byte sectors, a 3 TiB sparse disk, damaged tables, and the refusal rules.

use std::path::{Path, PathBuf};

use iyakki_storage::gpt;

use crate::drives::Drive;
use crate::fatclone::{self, tests::mini_fat, Fat32};
use crate::gpt_install::{grub_bits, GptLayout, ESP_4K_BYTES};
use crate::install::tests::{disk, template, tmp};
use crate::install::*;
use crate::writer::Phase;

const GIB: u64 = 1 << 30;
const TIB: u64 = 1 << 40;

/// A template whose boot volume is `fat` (a real FAT32 volume) instead of the fake one.
fn template_with_fat(path: &Path, fat: &[u8]) -> Vec<u8> {
    let mut t = template(path);
    let p1_sectors = (fat.len() / 512) as u32;
    let p2_start = 2048 + p1_sectors;
    t[446 + 12..446 + 16].copy_from_slice(&p1_sectors.to_le_bytes());
    t[462 + 8..462 + 12].copy_from_slice(&p2_start.to_le_bytes());
    t.truncate(2048 * 512);
    t.extend_from_slice(fat);
    t.resize((p2_start as usize + 2048) * 512, 0);
    std::fs::write(path, &t).unwrap();
    t
}

fn source_of(t: &Path) -> SourceInfo {
    read_source(t).unwrap()
}

/// Plan a GPT install onto a file standing in for a disk of `nominal` bytes with `ss`-byte sectors.
fn plan_on_file(dir: &Path, name: &str, nominal: u64, ss: u32, source: &SourceInfo) -> (PathBuf, Plan) {
    let f = dir.join(name);
    std::fs::File::create(&f).unwrap();
    let mut d = disk(name, 8);
    d.drive.path = f.display().to_string();
    d.drive.usb = true;
    d.logical_sector = ss;
    d.gpt = false;
    d.table = String::new();
    d.partitions.clear();
    // the 4 GB minimum is for real disks: small test files are planned as a 5 GB disk, then given their true layout
    d.drive.size_bytes = nominal.max(5 * GIB);
    let c = Choices { zram: "2G,lz4".into(), swapfile: "1G".into(), hostname: Some("gptbox".into()) };
    let mut plan = make_plan_with(&[d], name, source, &c, TableChoice::Gpt, false).unwrap();
    plan.target.drive.size_bytes = nominal;
    plan.gpt = Some(GptLayout::new(nominal, ss, source.p1_sectors * 512).unwrap());
    plan.layout = plan.gpt.as_ref().unwrap().data_layout();
    (f, plan)
}

fn parse(f: &Path, ss: u32, size: u64) -> gpt::Parsed {
    let mut h = std::fs::File::open(f).unwrap();
    gpt::read_gpt(&mut h, ss, size / ss as u64).unwrap()
}

fn bytes_at(f: &Path, off: u64, len: usize) -> Vec<u8> {
    use std::os::unix::fs::FileExt;
    let mut v = vec![0u8; len];
    std::fs::File::open(f).unwrap().read_exact_at(&mut v, off).unwrap();
    v
}

/// Run a real tool on an image file; `None` (with a note) when it is not installed.
fn tool(cmd: &str, args: &[&str]) -> Option<String> {
    match std::process::Command::new(cmd).args(args).output() {
        Ok(o) => Some(format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))),
        Err(_) => {
            eprintln!("{cmd} is not installed: skipped");
            None
        }
    }
}

#[test]
fn the_table_choice_follows_size_sector_size_and_firmware() {
    use TableChoice::*;
    let r = |c, size, ss, uefi| resolve_table(c, size, ss, uefi).map(|x| x.0);
    assert_eq!(r(Auto, 500 * GIB, 512, false), Ok(TableKind::Mbr), "old BIOS-only machines keep MBR");
    assert_eq!(r(Auto, 500 * GIB, 512, true), Ok(TableKind::Gpt), "UEFI machine");
    assert_eq!(r(Auto, 3 * TIB, 512, false), Ok(TableKind::Gpt), "over 2 TiB");
    assert_eq!(r(Auto, 2 * TIB, 512, false), Ok(TableKind::Mbr), "exactly 2 TiB still fits an MBR");
    assert_eq!(r(Auto, 500 * GIB, 4096, false), Ok(TableKind::Gpt), "4Kn");
    assert_eq!(r(Gpt, 8 * GIB, 512, false), Ok(TableKind::Gpt));
    assert_eq!(r(Mbr, 500 * GIB, 512, true), Ok(TableKind::Mbr), "an explicit choice wins over the firmware");
    assert!(r(Mbr, 500 * GIB, 4096, false).unwrap_err().contains("4Kn"));
    assert!(r(Auto, 500 * GIB, 1024, false).unwrap_err().contains("1024-byte"));
    assert!(resolve_table(Auto, 3 * TIB, 512, false).unwrap().1.contains("2 TiB"));
    assert_eq!((TableChoice::parse("GPT"), TableChoice::parse(" mbr "), TableChoice::parse("auto"), TableChoice::parse("")), (Ok(Gpt), Ok(Mbr), Ok(Auto), Ok(Auto)));
    assert!(TableChoice::parse("zfs").is_err());
    assert_eq!((Auto.next(), Gpt.next(), Mbr.next()), (Gpt, Mbr, Auto));
    assert!(Auto.explain().contains("2 TiB") && Gpt.explain().contains("EFI") && Mbr.explain().contains("512"));
}

#[test]
fn the_layout_is_aligned_and_the_data_partition_fills_the_disk() {
    for (size, ss) in [(64 * GIB, 512u32), (64 * GIB, 4096), (3 * TIB, 512), (10 * TIB, 4096), (GIB + 12345, 512)] {
        let g = GptLayout::new(size, ss, 256 * MIB).unwrap();
        let al = MIB / ss as u64;
        assert_eq!(g.total_sectors, size / ss as u64);
        assert_eq!((g.bios.0 % al, g.esp.0 % al, g.data.0 % al, g.data.1 % al), (0, 0, 0, 0), "1 MiB alignment ({size}, {ss})");
        assert_eq!((g.bios.0, g.bios.1 * ss as u64), (al, MIB), "BIOS boot partition: 1 MiB at 1 MiB");
        assert_eq!(g.esp.0, g.bios.0 + g.bios.1);
        assert_eq!(g.data.0, g.esp.0 + g.esp.1);
        assert_eq!(g.esp.1 * ss as u64, if ss == 512 { 256 * MIB } else { ESP_4K_BYTES }, "boot volume size");
        let t = g.table([1; 16], [[2; 16], [3; 16], [4; 16]]);
        t.check().unwrap();
        // the data partition ends within one MiB of the last usable sector: nothing is wasted
        let waste = (t.last_usable() + 1 - (g.data.0 + g.data.1)) * ss as u64;
        assert!(waste < MIB, "{size}/{ss}: {waste} bytes unused at the end");
        assert_eq!(t.entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["IYAKKI-BIOS", "IYAKKI-BOOT", "IYAKKI-DATA"]);
        assert_eq!(t.entries.iter().map(|e| e.type_guid).collect::<Vec<_>>(), [gpt::TYPE_BIOS_BOOT, gpt::TYPE_ESP, gpt::TYPE_LINUX_DATA]);
    }
    assert!(GptLayout::new(300 * MIB, 512, 256 * MIB).unwrap_err().contains("too small"));
    assert!(GptLayout::new(64 * GIB, 1024, 256 * MIB).unwrap_err().contains("not supported"));
    let big = GptLayout::new(3 * TIB, 512, 256 * MIB).unwrap();
    assert!(big.data.1 * 512 > 2 * TIB + 500 * GIB, "the data partition passes the 2 TiB MBR limit");
    assert_eq!(big.lines().len(), 3);
    assert!(big.lines()[0].contains("BIOS boot") && big.lines()[1].contains("EFI System") && big.lines()[2].contains("IYAKKI-DATA"));
    assert_eq!(GptLayout::new(64 * GIB, 4096, 0).unwrap().data_layout().data_start % 2048, 0, "in 512-byte sectors too");
}

#[test]
fn grub_is_patched_for_its_new_place() {
    let d = tmp("grubbits");
    let t = template(&d.join("t.img"));
    let s = source_of(&d.join("t.img"));
    let b = grub_bits(&s, 2048, MIB).unwrap();
    assert_eq!(u64::from_le_bytes(b.boot_code[0x5C..0x64].try_into().unwrap()), 2048, "boot code names the BIOS boot partition's first sector");
    assert_eq!(&b.boot_code[..0x5C], &t[..0x5C]);
    assert_eq!(&b.boot_code[0x64..], &t[0x64..440]);
    assert_eq!(u64::from_le_bytes(b.core[500..508].try_into().unwrap()), 2049, "the block list continues with the next sector");
    assert_eq!(&b.core[508..512], &t[512 + 508..512 + 512], "length and segment untouched");
    assert_eq!(&b.core[..500], &t[512..512 + 500]);
    assert_eq!(&b.core[512..11 * 512], &t[1024..1024 + 10 * 512]);
    assert_eq!(b.core.len(), MIB as usize);
    // a template that does not look like ours is refused, not guessed at
    let mut odd = t.clone();
    odd[0x5C] = 7;
    std::fs::write(d.join("odd.img"), &odd).unwrap();
    assert!(grub_bits(&source_of(&d.join("odd.img")), 2048, MIB).unwrap_err().contains("laid out differently"));
}

#[test]
fn a_gpt_install_on_a_512_byte_disk_is_complete_and_verified() {
    let dir = tmp("gpt512");
    let t = template(&dir.join("t.img"));
    let s = source_of(&dir.join("t.img"));
    let size = 700 * MIB;
    let (f, plan) = plan_on_file(&dir, "target", size, 512, &s);
    assert_eq!(plan.table, TableKind::Gpt);
    let text = plan.render();
    for want in ["Partition table: GPT", "BIOS boot", "EFI System Partition", "IYAKKI-DATA", "backup GPT first", "check both GPT copies"] {
        assert!(text.contains(want), "{want} in:\n{text}");
    }
    let mut phases = Vec::new();
    let r = execute(&plan, false, "", &mut |p| phases.push(p.phase)).unwrap();
    assert!(!r.dry_run && r.written > 0 && r.head_sha256.len() == 64);
    assert!(phases.contains(&Phase::Writing) && phases.contains(&Phase::Verifying) && phases.contains(&Phase::Syncing));
    let out_len = std::fs::metadata(&f).unwrap().len();
    assert_eq!(out_len, size, "the file is exactly the disk: the backup table is in its last sector");
    let g = plan.gpt.clone().unwrap();
    // both copies, parsed by our reader
    let p = parse(&f, 512, size);
    assert!(p.primary_ok && p.backup_ok && p.problems.is_empty(), "{:?}", p.problems);
    let tb = &p.table;
    assert_eq!(tb.entries.len(), 3);
    assert_eq!((tb.entries[0].first_lba, tb.entries[0].sectors()), (2048, 2048));
    assert_eq!((tb.entries[1].first_lba, tb.entries[1].sectors()), (4096, s.p1_sectors.div_ceil(2048) * 2048));
    assert_eq!(tb.entries[2].first_lba, g.data.0);
    assert!(tb.entries.iter().map(|e| e.guid).collect::<std::collections::HashSet<_>>().len() == 3 && tb.disk_guid != [0; 16], "unique GUIDs");
    assert_eq!(tb.backup_entries_lba() + 32 + 1, size / 512, "the backup sits at the true end of the disk");
    // the protective MBR carries GRUB's boot code, patched
    let m = bytes_at(&f, 0, 512);
    assert_eq!((m[450], m[510], m[511]), (0xEE, 0x55, 0xAA));
    assert_eq!(u32::from_le_bytes(m[454..458].try_into().unwrap()), 1);
    assert_eq!(u32::from_le_bytes(m[458..462].try_into().unwrap()) as u64, size / 512 - 1);
    assert_eq!((&m[..0x5C], &m[0x64..440]), (&t[..0x5C], &t[0x64..440]), "GRUB's boot code, byte for byte, except its core image address");
    assert_eq!(u64::from_le_bytes(m[0x5C..0x64].try_into().unwrap()), 2048);
    assert_ne!(&m[440..444], &t[440..444], "a fresh disk signature");
    // GRUB's core image sits at the BIOS boot partition, with its block list moved along
    let core = bytes_at(&f, 2048 * 512, 12 * 512);
    assert_eq!(&core[..500], &t[512..1012]);
    assert_eq!(u64::from_le_bytes(core[500..508].try_into().unwrap()), 2049);
    assert_eq!(&core[512..11 * 512], &t[1024..1024 + 10 * 512]);
    // the boot volume is the template's, verbatim
    let esp = bytes_at(&f, 4096 * 512, (s.p1_sectors * 512) as usize);
    assert_eq!(&esp[..], &t[2048 * 512..(2048 + s.p1_sectors as usize) * 512]);
    // the data partition starts with the marker and the choices
    let part = bytes_at(&f, g.data.0 * 512, 65536);
    assert_eq!(iyakki_storage::classify(&part), iyakki_storage::Content::Pending);
    let tmpf = dir.join("part");
    std::fs::write(&tmpf, &part).unwrap();
    let conf = iyakki_storage::read_pending_conf(&tmpf).unwrap();
    assert!(conf.contains("zram=2G,lz4") && conf.contains("hostname=gptbox") && conf.contains("swapfile=1G"), "{conf}");
    // the source was only read
    assert_eq!(std::fs::read(dir.join("t.img")).unwrap(), t);

    // what the real tools say, when they are installed
    let fs = f.to_str().unwrap();
    if let Some(o) = tool("sfdisk", &["-d", fs]) {
        assert!(o.contains("label: gpt"), "{o}");
        for want in ["type=21686148-6449-6E6F-744E-656564454649", "type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B", "type=0FC63DAF-8483-4772-8E79-3D69D8477DE4", "name=\"IYAKKI-BOOT\"", "name=\"IYAKKI-DATA\"", "start=        2048", "size=        2048"] {
            assert!(o.contains(want), "sfdisk -d: missing {want}:\n{o}");
        }
    }
    if let Some(o) = tool("sfdisk", &["--verify", fs]) {
        assert!(o.contains("No errors"), "sfdisk --verify: {o}");
    }
    if let Some(o) = tool("sgdisk", &["-v", fs]) {
        assert!(o.contains("No problems found"), "sgdisk -v: {o}");
        assert!(!o.contains("Caution") && !o.contains("Warning"), "sgdisk -v: {o}");
    }
    if let Some(o) = tool("gdisk", &["-l", fs]) {
        assert!(o.contains("GPT: present") && o.contains("protective") && o.contains("EF00") && o.contains("EF02") && o.contains("8300"), "gdisk -l: {o}");
    }
    if let Some(o) = tool("parted", &["-s", fs, "unit", "MiB", "print"]) {
        assert!(o.contains("Partition Table: gpt") && o.contains("bios_grub") && o.contains("esp") && o.contains("IYAKKI-DATA"), "parted: {o}");
    }
    if let Some(o) = tool("fdisk", &["-l", fs]) {
        assert!(o.contains("Disklabel type: gpt") && o.contains("BIOS boot") && o.contains("EFI System") && o.contains("Linux filesystem"), "fdisk -l: {o}");
    }
}

#[test]
fn the_disk_tools_agree_on_the_install_next_to_a_stale_table() {
    // an old GPT with a different layout at the start and a stale backup at the end are replaced, not mixed in
    let dir = tmp("gptstale");
    template(&dir.join("t.img"));
    let s = source_of(&dir.join("t.img"));
    let size = 700 * MIB;
    let (f, plan) = plan_on_file(&dir, "target", size, 512, &s);
    let old = GptLayout::new(size, 512, 100 * MIB).unwrap().table([9; 16], [[8; 16], [7; 16], [6; 16]]);
    let mut img = vec![0u8; size as usize];
    img[512..1024].copy_from_slice(&old.header_bytes(false));
    img[1024..1024 + 16384].copy_from_slice(&old.entries_bytes());
    let b = old.backup_entries_lba() as usize * 512;
    img[b..b + 16384].copy_from_slice(&old.entries_bytes());
    let l = size as usize - 512;
    img[l..].copy_from_slice(&old.header_bytes(true));
    std::fs::write(&f, &img).unwrap();
    assert!(parse(&f, 512, size).primary_ok);
    execute(&plan, false, "", &mut |_| {}).unwrap();
    let p = parse(&f, 512, size);
    assert!(p.primary_ok && p.backup_ok && p.problems.is_empty());
    assert_ne!(p.table.disk_guid, [9; 16]);
    assert_eq!(p.table.entries[2].name, "IYAKKI-DATA");
    if let Some(o) = tool("sgdisk", &["-v", f.to_str().unwrap()]) {
        assert!(o.contains("No problems found"), "{o}");
    }
}

#[test]
fn a_dry_run_writes_nothing_and_a_mount_that_appeared_stops_the_install() {
    let dir = tmp("gptdry");
    template(&dir.join("t.img"));
    let s = source_of(&dir.join("t.img"));
    let (f, plan) = plan_on_file(&dir, "target", 700 * MIB, 512, &s);
    let r = execute(&plan, true, "", &mut |_| panic!("nothing is written")).unwrap();
    assert!(r.dry_run && r.written == 0);
    assert_eq!(std::fs::metadata(&f).unwrap().len(), 0, "not even created or resized");
    let mut named = plan.clone();
    named.target.drive.name = "sda".into();
    named.target.drive.path = "/dev/sda".into();
    assert!(execute(&named, false, "/dev/sda2 /mnt ext4 rw 0 0\n", &mut |_| {}).unwrap_err().contains("was mounted"));
}

#[test]
fn a_4096_byte_sector_disk_gets_a_rebuilt_boot_volume_and_a_valid_table() {
    let dir = tmp("gpt4k");
    let files = fatclone::tests::sample_files();
    let fat = mini_fat(512, 1, 40 * MIB, &files);
    template_with_fat(&dir.join("t.img"), &fat);
    let s = source_of(&dir.join("t.img"));
    let size = 900 * MIB;
    let (f, plan) = plan_on_file(&dir, "target", size, 4096, &s);
    assert!(plan.gpt.as_ref().unwrap().esp_rebuilt);
    let text = plan.render();
    assert!(text.contains("4096-byte sectors") && text.contains("rebuilt for 4096-byte sectors") && text.contains("UEFI only"), "{text}");
    execute(&plan, false, "", &mut |_| {}).unwrap();
    let p = parse(&f, 4096, size);
    assert!(p.primary_ok && p.backup_ok && p.problems.is_empty(), "{:?}", p.problems);
    let g = plan.gpt.clone().unwrap();
    assert_eq!(p.table.entries[1].first_lba, 512, "the boot volume starts at 2 MiB: sector 512 of 4096 bytes");
    assert_eq!(p.table.entries[1].sectors() * 4096, 320 * MIB);
    assert_eq!(p.table.array_sectors(), 4);
    assert_eq!(p.table.backup_entries_lba() + 4 + 1, size / 4096);
    // sector 0 is the protective MBR with no boot code, header at sector 1 (byte 4096)
    let m = bytes_at(&f, 0, 4096);
    assert_eq!((m[450], m[510], m[511], &m[..440]), (0xEE, 0x55, 0xAA, &[0u8; 440][..]));
    assert_eq!(&bytes_at(&f, 4096, 8), b"EFI PART");
    // the boot volume is FAT32 with 4096-byte sectors holding the same files
    let mut h = std::fs::File::open(&f).unwrap();
    let mut vol = Fat32::open(&mut h, g.esp.0 * 4096).unwrap();
    assert_eq!(vol.label, "IYAKKI-BOOT");
    let items = vol.walk().unwrap();
    assert_eq!(items.iter().filter(|i| !i.dir).count(), 5);
    let kernel = items.iter().find(|i| i.path == "/BZIMAGE").unwrap().clone();
    assert_eq!(vol.read_file(&kernel).unwrap(), files[0].1);
    assert_eq!(bytes_at(&f, g.esp.0 * 4096 + 11, 2), 4096u16.to_le_bytes());
    // the data partition marker is at its start
    assert_eq!(iyakki_storage::classify(&bytes_at(&f, g.data.0 * 4096, 65536)), iyakki_storage::Content::Pending);
    if let Some(o) = tool("sfdisk", &["--sector-size", "4096", "-d", f.to_str().unwrap()]) {
        if o.contains("label: gpt") {
            assert!(o.contains("name=\"IYAKKI-DATA\"") && o.contains("start=         512"), "{o}");
        } else {
            eprintln!("this sfdisk cannot read 4096-byte images from a file: {o}");
        }
    }
}

#[test]
fn a_disk_over_2_tib_is_used_whole_without_allocating_anything() {
    let dir = tmp("gpt3tib");
    template(&dir.join("t.img"));
    let s = source_of(&dir.join("t.img"));
    let size = 3 * TIB;
    let mut d = disk("big", 8);
    let f = dir.join("big");
    d.drive.path = f.display().to_string();
    d.drive.usb = true;
    d.drive.size_bytes = size;
    d.gpt = false;
    d.table = String::new();
    d.partitions.clear();
    std::fs::File::create(&f).unwrap();
    // auto picks GPT for it and says why; an MBR would stop at 2 TiB
    let plan = make_plan_with(&[d.clone()], "big", &s, &Choices::default(), TableChoice::Auto, false).unwrap();
    assert_eq!(plan.table, TableKind::Gpt);
    assert!(plan.table_why.contains("2 TiB") && plan.warnings.iter().all(|w| !w.contains("only the first 2 TiB")));
    assert!(plan.gpt.as_ref().unwrap().data.1 * 512 > 2 * TIB + 700 * GIB, "the data partition passes 2 TiB");
    let mbr = make_plan_with(&[d], "big", &s, &Choices::default(), TableChoice::Mbr, false).unwrap();
    assert!(mbr.warnings.iter().any(|w| w.contains("2 TiB") && w.contains("Choose GPT")), "{:?}", mbr.warnings);
    execute(&plan, false, "", &mut |_| {}).unwrap();
    let meta = std::fs::metadata(&f).unwrap();
    assert_eq!(meta.len(), size);
    use std::os::unix::fs::MetadataExt;
    assert!(meta.blocks() * 512 < 600 * MIB, "sparse: {} bytes allocated", meta.blocks() * 512);
    let p = parse(&f, 512, size);
    assert!(p.primary_ok && p.backup_ok && p.problems.is_empty());
    let data = &p.table.entries[2];
    assert!(data.sectors() * 512 > 2 * TIB, "{} sectors", data.sectors());
    assert_eq!((data.last_lba + 1) % 2048, 0, "ends on a whole MiB");
    assert!(p.table.last_usable() - data.last_lba < 2048, "and uses everything else");
    assert!(data.last_lba > u32::MAX as u64, "a sector number an MBR could not hold");
    let m = bytes_at(&f, 0, 512);
    assert_eq!(u32::from_le_bytes(m[458..462].try_into().unwrap()), u32::MAX, "the protective MBR is capped at 0xFFFFFFFF");
    if let Some(o) = tool("sgdisk", &["-v", f.to_str().unwrap()]) {
        assert!(o.contains("No problems found"), "sgdisk -v on 3 TiB: {o}");
    }
    if let Some(o) = tool("sfdisk", &["-d", f.to_str().unwrap()]) {
        assert!(o.contains("label: gpt") && o.contains("name=\"IYAKKI-DATA\""), "{o}");
    }
}

#[test]
fn damaged_tables_are_rejected_and_one_good_copy_is_noticed() {
    let dir = tmp("gptbad");
    template(&dir.join("t.img"));
    let s = source_of(&dir.join("t.img"));
    let size = 700 * MIB;
    let (f, plan) = plan_on_file(&dir, "target", size, 512, &s);
    execute(&plan, false, "", &mut |_| {}).unwrap();
    use std::os::unix::fs::FileExt;
    let w = std::fs::OpenOptions::new().write(true).open(&f).unwrap();
    // a flipped bit in the primary entry array
    let mut b = bytes_at(&f, 1024 + 5, 1);
    b[0] ^= 0x10;
    w.write_all_at(&b, 1024 + 5).unwrap();
    let p = parse(&f, 512, size);
    assert!(!p.primary_ok && p.backup_ok && p.problems[0].contains("primary entries") && p.problems[0].contains("CRC32"), "{:?}", p.problems);
    let info = iyakki_storage::gpt::read_disk_table(&mut std::fs::File::open(&f).unwrap(), 512, size);
    assert_eq!(info.parts.len(), 3);
    assert!(info.notes[0].contains("primary GPT is damaged"));
    // both copies damaged: not a table any more
    let last = size - 512;
    let mut h = bytes_at(&f, last + 40, 1);
    h[0] ^= 1;
    w.write_all_at(&h, last + 40).unwrap();
    let mut h2 = std::fs::File::open(&f).unwrap();
    assert!(gpt::read_gpt(&mut h2, 512, size / 512).unwrap_err().contains("CRC32"));
    if let Some(o) = tool("sgdisk", &["-v", f.to_str().unwrap()]) {
        assert!(!o.contains("No problems found"), "the tool sees it too: {o}");
    }
}

#[test]
fn an_existing_table_is_read_for_the_confirmation_with_types_and_labels() {
    let dir = tmp("probe");
    template(&dir.join("t.img"));
    let s = source_of(&dir.join("t.img"));
    // a disk "nvme0n1" (a file in a made-up /dev) that already holds an IYAKKI-style GPT: the partition names take the p suffix
    let dev = dir.join("dev");
    std::fs::create_dir_all(&dev).unwrap();
    let size = 700 * MIB;
    let old = GptLayout::new(size, 512, 256 * MIB).unwrap();
    let t = old.table([5; 16], [[1; 16], [2; 16], [3; 16]]);
    let mut img = vec![0u8; size as usize];
    img[..512].copy_from_slice(&t.protective_mbr(&[], [0; 4]));
    img[512..1024].copy_from_slice(&t.header_bytes(false));
    img[1024..1024 + 16384].copy_from_slice(&t.entries_bytes());
    let b = t.backup_entries_lba() as usize * 512;
    img[b..b + 16384].copy_from_slice(&t.entries_bytes());
    let l = size as usize - 512;
    img[l..].copy_from_slice(&t.header_bytes(true));
    std::fs::write(dev.join("nvme0n1"), &img).unwrap();
    let mut d = disk("nvme0n1", 8);
    d.drive.size_bytes = size;
    d.gpt = false;
    d.table = String::new();
    d.partitions = (1..=3).map(|n| (partition_name("nvme0n1", n), 1000, String::new())).collect();
    let mut v = vec![d];
    probe_contents(&mut v, &dev);
    assert!(v[0].gpt && v[0].table == "GPT", "{}", v[0].table);
    assert!(v[0].partitions[0].2.contains("BIOS boot") && v[0].partitions[1].2.contains("EFI System") && v[0].partitions[1].2.contains("IYAKKI-BOOT") && v[0].partitions[2].2.contains("Linux filesystem"), "{:?}", v[0].partitions);
    assert_eq!(v[0].partitions[2].0, "nvme0n1p3");
    // the confirmation shows it
    let plan = make_plan_with(&v, "nvme0n1", &s, &Choices::default(), TableChoice::Gpt, false);
    // (the disk is "700 MiB": too small for a real install, and the refusal says so)
    assert!(plan.unwrap_err().contains("at least"));
    v[0].drive.size_bytes = 64 * GIB;
    let plan = make_plan_with(&v, "nvme0n1", &s, &Choices::default(), TableChoice::Auto, false).unwrap();
    let text = plan.render();
    for want in ["(partition table: GPT)", "nvme0n1p2", "EFI System", "“IYAKKI-BOOT”", "Linux filesystem", "the disk has a GPT partition table; it is replaced"] {
        assert!(text.contains(want), "{want} in:\n{text}");
    }
    // an MBR disk and a blank one
    let mut mbr = vec![0u8; 4 << 20];
    mbr[446 + 4] = 0x07;
    mbr[446 + 8..446 + 12].copy_from_slice(&2048u32.to_le_bytes());
    mbr[446 + 12..446 + 16].copy_from_slice(&4096u32.to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    std::fs::write(dev.join("sdb"), &mbr).unwrap();
    std::fs::write(dev.join("sdc"), vec![0u8; 4 << 20]).unwrap();
    let mut two = vec![disk("sdb", 8), disk("sdc", 8)];
    for d in &mut two {
        d.drive.size_bytes = 4 << 20;
        d.table = String::new();
        d.gpt = false;
        d.partitions = vec![(format!("{}1", d.drive.name), 1000, String::new())];
    }
    probe_contents(&mut two, &dev);
    assert_eq!((two[0].table.as_str(), two[1].table.as_str()), ("MBR", "none"));
    assert!(two[0].partitions[0].2.contains("NTFS"), "{:?}", two[0].partitions);
}

#[test]
fn the_refusal_rules_hold_for_gpt_too() {
    let d = tmp("gptrefuse");
    template(&d.join("t.img"));
    let s = source_of(&d.join("t.img"));
    let c = Choices::default();
    let plan = |disks: &[DiskInfo], name: &str, table| make_plan_with(disks, name, &s, &c, table, true);
    let mut mounted = disk("sda", 512);
    mounted.drive.mounts = vec![("/dev/sda2".into(), "/mnt/x".into())];
    assert!(plan(&[mounted], "sda", TableChoice::Gpt).unwrap_err().contains("Unmount it first"));
    let mut sys = disk("nvme0n1", 512);
    sys.drive.mounts = vec![("/dev/nvme0n1p3".into(), "/".into())];
    assert!(plan(&[sys], "nvme0n1", TableChoice::Auto).unwrap_err().contains("running system"));
    let mut ro = disk("sdb", 512);
    ro.drive.read_only = true;
    assert!(plan(&[ro], "sdb", TableChoice::Gpt).unwrap_err().contains("write-protected"));
    let mut swap = disk("sdc", 512);
    swap.drive.swap = true;
    assert!(plan(&[swap], "sdc", TableChoice::Gpt).unwrap_err().contains("swap"));
    assert!(plan(&[disk("sdd", 2)], "sdd", TableChoice::Gpt).unwrap_err().contains("at least"));
    let mut odd = disk("sde", 512);
    odd.logical_sector = 1024;
    assert!(plan(&[odd], "sde", TableChoice::Auto).unwrap_err().contains("1024-byte"));
    let mut fourk = disk("sdf", 512);
    fourk.logical_sector = 4096;
    assert!(plan(&[fourk.clone()], "sdf", TableChoice::Mbr).unwrap_err().contains("4Kn"));
    let p = plan(&[fourk.clone()], "sdf", TableChoice::Auto).unwrap();
    assert_eq!((p.table, p.gpt.as_ref().unwrap().sector_size), (TableKind::Gpt, 4096));
    assert!(p.table_why.contains("4096-byte") && p.warnings.iter().any(|w| w.contains("UEFI only")));
    let mut from = s.clone();
    from.path = PathBuf::from("/dev/sdg");
    assert!(make_plan_with(&[disk("sdg", 512)], "sdg", &from, &c, TableChoice::Gpt, true).unwrap_err().contains("copied from"));
    // the typed confirmation is unchanged
    assert!(confirmation_ok("sdf", "512.0 GB", &fourk.drive) && !confirmation_ok("sdf", "1 TB", &fourk.drive));
    // the plan for a stick made by iyakki-usb keeps no 4 GB rule of its own
    let stick = Drive { path: "/dev/sdz".into(), name: "sdz".into(), model: "Stick".into(), size_bytes: 2 * GIB, removable: true, usb: true, read_only: false, mounts: vec![], swap: false };
    let sp = plan_stick_gpt(&stick, 512, &s, "chosen: GPT").unwrap();
    let bytes = sp.gpt.unwrap().data.1 * 512;
    assert!(bytes <= 2 * GIB && bytes > 2 * GIB - 16 * MIB, "the stick is used whole: {bytes}");
}

#[test]
fn partition_names_for_every_kind_of_disk() {
    for (disk, n, want) in [("sda", 3, "sda3"), ("vdb", 1, "vdb1"), ("nvme0n1", 3, "nvme0n1p3"), ("nvme12n2", 2, "nvme12n2p2"), ("mmcblk0", 3, "mmcblk0p3"), ("xvda", 2, "xvda2")] {
        assert_eq!(partition_name(disk, n), want);
    }
}

#[test]
fn the_stick_writer_makes_the_same_gpt() {
    // `iyakki-usb --install --table gpt` goes through plan_stick_gpt and execute
    let dir = tmp("stick");
    let t = template(&dir.join("t.img"));
    let s = source_of(&dir.join("t.img"));
    let f = dir.join("stick");
    std::fs::File::create(&f).unwrap();
    let drive = Drive { path: f.display().to_string(), name: "testfile".into(), model: "test file".into(), size_bytes: 512 * MIB, removable: true, usb: true, read_only: false, mounts: vec![], swap: false };
    let plan = plan_stick_gpt(&drive, 512, &s, "chosen: GPT").unwrap();
    execute(&plan, false, "", &mut |_| {}).unwrap();
    let p = parse(&f, 512, 512 * MIB);
    assert!(p.primary_ok && p.backup_ok);
    assert_eq!(bytes_at(&f, 4096 * 512, 8192 * 512), t[2048 * 512..(2048 + 8192) * 512]);
    // the marker has no installer choices beyond the defaults
    let part = bytes_at(&f, plan.gpt.as_ref().unwrap().data.0 * 512, 65536);
    assert_eq!(iyakki_storage::classify(&part), iyakki_storage::Content::Pending);
}

#[test]
fn a_4096_byte_sector_disk_gets_a_boot_volume_as_big_as_the_image_needs() {
    let small = GptLayout::new(64 * GIB, 4096, 100 * MIB).unwrap();
    assert_eq!(small.bytes(small.esp.1), ESP_4K_BYTES, "a small image keeps the minimum");
    let slim = GptLayout::new(64 * GIB, 4096, 458 * MIB).unwrap();
    assert_eq!(slim.bytes(slim.esp.1), 458 * MIB, "a bigger image is not cut to the minimum");
}
