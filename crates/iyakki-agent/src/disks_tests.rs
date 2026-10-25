//! Tests of the DISKS engine on a made-up machine (`disks_fake`): no real drive is read, mounted or written.

use std::cell::RefCell;

use iyakki_sec::policy::Role;

use crate::disks::*;
use crate::disks_fake::*;
use crate::disks_part::*;

fn admin() -> Actor {
    Actor { user: "root".into(), role: Role::Admin, uid: 1000, gid: 1000, home: "/home/root".into() }
}

fn who(role: Role) -> Actor {
    Actor { user: "sam".into(), role, uid: 1001, gid: 1001, home: "/home/sam".into() }
}

type Log = RefCell<Vec<(String, String)>>;

fn logger() -> Log {
    RefCell::new(Vec::new())
}

fn sink(l: &Log) -> impl Fn(&str, &str) + '_ {
    move |a, d| l.borrow_mut().push((a.to_string(), d.to_string()))
}

fn actions(l: &Log) -> Vec<String> {
    l.borrow().iter().map(|(a, _)| a.clone()).collect()
}

fn gb(n: u64) -> u64 {
    n * 1_000_000_000
}

#[test]
fn every_filesystem_is_recognised_from_its_superblock_with_label_and_id() {
    let cases: Vec<(Vec<u8>, Fs, &str)> = vec![
        (sb_fat32("STICK"), Fs::Fat32, "STICK"),
        (sb_fat16("OLD", "FAT16"), Fs::Fat16, "OLD"),
        (sb_fat16("TINY", "FAT12"), Fs::Fat12, "TINY"),
        (sb_ext4("data"), Fs::Ext4, "data"),
        (sb_ext("e3", 0x4, 0x2, 0x1), Fs::Ext3, "e3"),
        (sb_ext("e2", 0, 0x2, 0x1), Fs::Ext2, "e2"),
        (sb_ntfs(), Fs::Ntfs, ""),
        (sb_exfat(), Fs::Exfat, ""),
        (sb_btrfs("archive"), Fs::Btrfs, "archive"),
        (sb_xfs("xfsvol"), Fs::Xfs, "xfsvol"),
        (sb_f2fs(), Fs::F2fs, "phone"),
        (sb_iso("INSTALL"), Fs::Iso9660, "INSTALL"),
        (sb_swap(), Fs::Swap, ""),
        (sb_luks(), Fs::Luks, ""),
        (sb_bitlocker(), Fs::BitLocker, ""),
        (sb_apfs(), Fs::Apfs, ""),
        (sb_hfs(), Fs::HfsPlus, ""),
        (blank(), Fs::Empty, ""),
    ];
    for (bytes, fs, label) in cases {
        let p = detect(&bytes);
        assert_eq!((p.fs, p.label.as_str()), (fs, label), "{fs:?}");
    }
    let mut junk = blank();
    junk[100] = 7;
    assert_eq!(detect(&junk).fs, Fs::Unknown);
    assert_eq!(detect(&sb_fat32("X")).uuid, "A1B2-C3D4");
    assert_eq!(detect(&sb_ntfs()).uuid, "0807060504030201");
    assert_eq!(detect(&sb_exfat()).uuid, "12345678");
    assert_eq!(detect(&sb_ext4("d")).uuid, "11223344-5566-7788-99aa-bbccddeeff01");
    assert_eq!(detect(&sb_luks()).uuid, "0a1b2c3d-0000-4000-8000-000000000001");
    assert!(detect(&[0u8; 100]).fs == Fs::Unknown, "a short read is not called empty");
}

#[test]
fn the_listing_shows_system_usb_nvme_sata_and_sd_drives() {
    let f = machine();
    let s = f.disks().scan();
    let names: Vec<&str> = s.disks.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["mmcblk0", "nvme0n1", "nvme1n1", "sda", "sdb"]);
    let sys = s.disk("nvme0n1").unwrap();
    assert!(sys.system && sys.system_why.contains("/boot is mounted"), "{}", sys.system_why);
    assert_eq!((sys.transport, sys.removable, sys.serial.as_str()), (Transport::Nvme, false, "S64DNX0T123456"));
    assert_eq!(sys.part("nvme0n1p2").unwrap().space, Some((300_000_000_000, 500_000_000_000)));
    let root = sys.part("nvme0n1p2").unwrap();
    assert!(root.mounted() && root.system && root.fs == Fs::Ext4 && root.mountpoints == ["/"]);
    let usb = s.disk("sda").unwrap();
    assert_eq!((usb.transport, usb.removable, usb.system), (Transport::Usb, true, false));
    assert_eq!((usb.title().as_str(), usb.serial.as_str(), usb.serial_tail().as_str()), ("SanDisk Ultra", "4C530001230815ABCD", "ABCD"));
    assert_eq!(usb.size_bytes, gb(64));
    let p = usb.part("sda1").unwrap();
    assert_eq!((p.fs, p.label.as_str(), p.mounted(), p.number), (Fs::Fat32, "STICK", false, 1));
    let win = s.disk("nvme1n1").unwrap();
    assert_eq!((win.transport, win.system), (Transport::Nvme, false));
    assert_eq!(win.parts.iter().map(|p| p.fs).collect::<Vec<_>>(), [Fs::Ntfs, Fs::BitLocker]);
    let sata = s.disk("sdb").unwrap();
    assert_eq!(sata.transport, Transport::Sata);
    assert_eq!(sata.parts.iter().map(|p| p.fs).collect::<Vec<_>>(), [Fs::Luks, Fs::Apfs, Fs::HfsPlus, Fs::Btrfs]);
    let sd = s.disk("mmcblk0").unwrap();
    assert_eq!((sd.transport, sd.removable, sd.parts[0].fs), (Transport::Mmc, true, Fs::Exfat), "an SD card is removable even when sysfs says 0");
    assert!(!sd.system);
}

#[test]
fn space_comes_from_the_mount_and_a_disk_without_table_is_listed_honestly() {
    let f = machine();
    f.st.lock().unwrap().space.insert("/boot".into(), (1, 2));
    let s = f.disks().scan();
    assert_eq!(s.disk("nvme0n1").unwrap().part("nvme0n1p1").unwrap().space, Some((1, 2)));
    assert_eq!(s.disk("nvme0n1").unwrap().part("nvme0n1p1").unwrap().used_bytes(), Some(1));
    // a file system straight on the disk (no partition table)
    let f2 = Fake::new();
    f2.add_disk("sdd", 100_000, true, "/sys/devices/x/usb1/1-1/block/sdd", "Cheap", "Stick", 512);
    f2.set_dev("sdd", sb_fat32("SUPERFLOPPY"));
    let d = f2.disks().scan();
    let disk = d.disk("sdd").unwrap();
    assert_eq!(disk.parts.len(), 1);
    assert_eq!((disk.parts[0].number, disk.parts[0].name.as_str(), disk.parts[0].fs), (0, "sdd", Fs::Fat32));
    assert_eq!(disk.table, Table::None);
}

#[test]
fn the_kernel_support_list_follows_proc_filesystems_and_modules() {
    let f = machine();
    let s = f.disks().scan();
    assert!(s.kernel_has("vfat") && s.kernel_has("ext4") && !s.kernel_has("ntfs3") && !s.kernel_has("exfat") && !s.kernel_has("btrfs"));
    f.set("/proc/sys/kernel/osrelease", "6.12.1\n");
    f.set("/lib/modules/6.12.1/modules.dep", "kernel/fs/exfat/exfat.ko:\nkernel/fs/ntfs3/ntfs3.ko: \n");
    f.set("/lib/modules/6.12.1/modules.builtin", "kernel/fs/btrfs/btrfs.ko\n");
    let s = f.disks().scan();
    assert!(s.kernel_has("exfat") && s.kernel_has("ntfs3") && s.kernel_has("btrfs") && !s.kernel_has("xfs"));
}

#[test]
fn roles_decide_who_may_touch_which_drive() {
    let s = machine().disks().scan();
    let (usb, sata) = (s.disk("sda").unwrap(), s.disk("sdb").unwrap());
    for r in [Role::Admin, Role::Operator] {
        assert!(may_use(&who(r), usb).is_ok() && may_use(&who(r), sata).is_ok(), "{r:?}");
    }
    assert!(may_use(&who(Role::User), usb).is_ok());
    assert!(may_use(&who(Role::User), s.disk("mmcblk0").unwrap()).is_ok());
    assert!(may_use(&who(Role::User), sata).unwrap_err().contains("removable"));
    assert!(may_use(&who(Role::Guest), usb).unwrap_err().contains("Guests"));
}

#[test]
fn mount_options_per_filesystem_and_role() {
    let f = machine();
    f.set("/proc/filesystems", "");
    f.st.lock().unwrap().fs_list = "\text4\n\tvfat\n\texfat\n\tntfs3\n\tbtrfs\n\txfs\n\tiso9660\n\thfsplus\n".into();
    let d = f.disks();
    let s = d.scan();
    let o = MountOpts::default();
    let me = who(Role::Operator);
    let fat = d.plan_mount(&s, &me, "sda1", &o).unwrap();
    assert_eq!((fat.fstype, fat.target.as_str(), fat.dev.as_str()), ("vfat", "/home/sam/media/STICK", "/dev/sda1"));
    assert_eq!(fat.flags, MountFlags { rdonly: false, nosuid: true, nodev: true, noexec: true });
    assert_eq!(fat.data, "uid=1001,gid=1001,umask=0077,utf8=1,flush,shortname=mixed", "the person owns the files");
    let ex = d.plan_mount(&s, &me, "mmcblk0p1", &o).unwrap();
    assert_eq!((ex.fstype, ex.data.as_str()), ("exfat", "uid=1001,gid=1001,umask=0077,iocharset=utf8"));
    let nt = d.plan_mount(&s, &me, "nvme1n1p1", &o).unwrap();
    assert_eq!((nt.fstype, nt.data.as_str()), ("ntfs3", "uid=1001,gid=1001,umask=0077,windows_names"));
    let bt = d.plan_mount(&s, &me, "sdb4", &o).unwrap();
    assert_eq!((bt.fstype, bt.data.as_str()), ("btrfs", ""));
    let hfs = d.plan_mount(&s, &me, "sdb3", &o).unwrap();
    assert_eq!(hfs.fstype, "hfsplus");
    assert!(hfs.flags.rdonly && hfs.forced_ro.as_deref().unwrap().contains("only ever opened for reading"), "Mac volumes are read-only");
    // the read-only switch and "Allow running programs"
    let ro = d.plan_mount(&s, &me, "sda1", &MountOpts { read_only: true, allow_exec: false }).unwrap();
    assert!(ro.flags.rdonly && ro.forced_ro.is_none());
    let e = d.plan_mount(&s, &me, "sda1", &MountOpts { read_only: false, allow_exec: true }).unwrap_err();
    assert!(e.contains("administrator"), "{e}");
    let ok = d.plan_mount(&s, &admin(), "sda1", &MountOpts { read_only: false, allow_exec: true }).unwrap();
    assert!(!ok.flags.noexec && ok.flags.nosuid && ok.flags.nodev, "nosuid and nodev stay even then");
    // roles
    assert!(d.plan_mount(&s, &who(Role::User), "sda1", &o).is_ok());
    assert!(d.plan_mount(&s, &who(Role::User), "sdb4", &o).unwrap_err().contains("removable"));
    assert!(d.plan_mount(&s, &who(Role::Guest), "sda1", &o).unwrap_err().contains("Guests"));
}

#[test]
fn what_cannot_be_opened_is_explained_not_failed() {
    let f = machine();
    let d = f.disks();
    let s = d.scan();
    let me = admin();
    let o = MountOpts::default();
    let e = |part: &str| d.plan_mount(&s, &me, part, &o).unwrap_err();
    assert!(e("nvme1n1p2").contains("BitLocker") && e("nvme1n1p2").contains("unlock needs the key"));
    assert!(e("sdb1").contains("LUKS") && e("sdb1").contains("unlock needs the key"));
    assert!(e("sdb2").contains("APFS"));
    assert!(e("nvme1n1p1").contains("ntfs3 driver"), "{}", e("nvme1n1p1"));
    assert!(e("sdb4").contains("btrfs driver"));
    assert!(e("mmcblk0p1").contains("exfat driver"));
    assert!(e("nvme0n1p2").contains("running system"));
    assert!(e("nvme9n1p1").contains("no such partition"));
}

#[test]
fn mounting_creates_the_folder_mounts_with_safe_flags_and_audits() {
    let f = machine();
    let d = f.disks();
    let log = logger();
    let m = d.mount(&who(Role::Operator), "sda1", &MountOpts::default(), &sink(&log)).unwrap();
    assert_eq!(m.target, "/home/sam/media/STICK");
    assert!(!m.read_only && m.notes.iter().any(|n| n.contains("cannot be run")), "{:?}", m.notes);
    let calls = f.calls();
    assert_eq!(calls, ["mkdir /home/sam/media/STICK", "chown /home/sam/media 1001 1001", "chown /home/sam/media/STICK 1001 1001", "mount /dev/sda1 /home/sam/media/STICK vfat ro=false"]);
    let (_, _, _, flags, data) = f.last_mount().unwrap();
    assert!(flags.nosuid && flags.nodev && flags.noexec && data.contains("uid=1001"));
    let l = log.borrow();
    assert_eq!(l[0].0, "disks.mount");
    assert!(l[0].1.contains("drive=sda size=64000000000 serial=4C530001230815ABCD") && l[0].1.contains("fs=FAT32"), "{}", l[0].1);
    drop(l);
    let s = d.scan();
    let p = s.disk("sda").unwrap().part("sda1").unwrap();
    assert_eq!(p.mountpoints, ["/home/sam/media/STICK"]);
    assert!(d.plan_mount(&s, &admin(), "sda1", &MountOpts::default()).unwrap_err().contains("already open at /home/sam/media/STICK"));
}

#[test]
fn two_drives_with_the_same_label_get_different_folders_and_odd_labels_are_made_safe() {
    let f = machine();
    f.add_disk("sde", 200_000, true, "/sys/devices/x/usb2/2-1/block/sde", "Other", "Stick", 512);
    f.add_part("sde", "sde1", 1, 2048, 100_000, sb_fat32("STICK"));
    f.add_disk("sdf", 200_000, true, "/sys/devices/x/usb2/2-2/block/sdf", "Other", "Stick", 512);
    f.add_part("sdf", "sdf1", 1, 2048, 100_000, sb_fat32("../My Photo"));
    let d = f.disks();
    let log = logger();
    assert_eq!(d.mount(&admin(), "sda1", &MountOpts::default(), &sink(&log)).unwrap().target, "/home/root/media/STICK");
    assert_eq!(d.mount(&admin(), "sde1", &MountOpts::default(), &sink(&log)).unwrap().target, "/home/root/media/STICK-2");
    let t = d.mount(&admin(), "sdf1", &MountOpts::default(), &sink(&log)).unwrap().target;
    assert_eq!(t, "/home/root/media/_My_Photo", "no slash or dots can leave the managed folder");
    assert!(!t["/home/root/media/".len()..].contains('/'));
}

#[test]
fn a_write_protected_drive_is_opened_read_only_and_says_so() {
    let f = machine();
    f.set("/sys/block/sda/ro", "1\n");
    let d = f.disks();
    let log = logger();
    let m = d.mount(&admin(), "sda1", &MountOpts::default(), &sink(&log)).unwrap();
    assert!(m.read_only && m.notes.iter().any(|n| n.contains("write-protected")), "{:?}", m.notes);
}

#[test]
fn a_dirty_windows_volume_is_opened_read_only_with_a_plain_warning() {
    let f = machine();
    f.st.lock().unwrap().fs_list.push_str("\tntfs3\n");
    f.fail_mount("ntfs3:rw", "Invalid argument");
    let d = f.disks();
    let log = logger();
    let m = d.mount(&admin(), "nvme1n1p1", &MountOpts::default(), &sink(&log)).unwrap();
    assert!(m.read_only);
    let n = m.notes.join(" ");
    assert!(n.contains("Windows did not shut this drive down cleanly") && n.contains("repair needs Windows"), "{n}");
    let calls = f.calls();
    assert!(calls.iter().any(|c| c.ends_with("ntfs3 ro=false")) && calls.iter().any(|c| c.ends_with("ntfs3 ro=true")));
    // both fail: the folder is removed again and the reason is plain
    let f = machine();
    f.st.lock().unwrap().fs_list.push_str("\tntfs3\n");
    f.fail_mount("ntfs3", "Invalid argument");
    let e = f.disks().mount(&admin(), "nvme1n1p1", &MountOpts::default(), &sink(&log)).unwrap_err();
    assert!(e.contains("damaged") && e.contains("NTFS"), "{e}");
    assert!(f.calls().iter().any(|c| c.starts_with("rmdir /home/root/media/")));
    assert!(log.borrow().iter().any(|(a, _)| a == "disks.mount.failed"));
}

#[test]
fn a_busy_drive_is_not_unmounted_and_the_programs_are_named() {
    let f = machine();
    let d = f.disks();
    let log = logger();
    d.mount(&admin(), "sda1", &MountOpts::default(), &sink(&log)).unwrap();
    f.add_process(77, "vlc", "/home/x", &["/home/root/media/STICK/film.mkv"]);
    f.add_process(78, "bash", "/home/root/media/STICK/docs", &[]);
    f.add_process(79, "other", "/home/root/media/STICKY", &[]);
    f.make_busy("/home/root/media/STICK");
    let e = d.unmount(&admin(), "sda1", &sink(&log)).unwrap_err();
    assert!(e.contains("vlc (process 77)") && e.contains("bash (process 78)") && !e.contains("other"), "{e}");
    assert!(e.contains("Nothing was forced"), "{e}");
    let calls = f.calls();
    let (si, ui) = (calls.iter().position(|c| c == "sync").unwrap(), calls.iter().position(|c| c.starts_with("umount")).unwrap());
    assert!(si < ui, "flushed first: {calls:?}");
    assert_eq!(calls.iter().filter(|c| c.starts_with("umount")).count(), 1, "one plain attempt, no retry with force");
    assert!(d.scan().disk("sda").unwrap().mounted(), "still mounted");
    assert!(actions(&log).contains(&"disks.unmount.failed".to_string()));
    // once they are gone it works, and the folder is removed
    f.st.lock().unwrap().busy.clear();
    let ok = d.unmount(&admin(), "sda1", &sink(&log)).unwrap();
    assert!(ok.contains("safe to eject"), "{ok}");
    assert!(f.calls().contains(&"rmdir /home/root/media/STICK".to_string()));
    assert!(!d.scan().disk("sda").unwrap().mounted());
    assert!(actions(&log).contains(&"disks.unmount".to_string()));
}

#[test]
fn a_busy_drive_nobody_can_name_still_gets_a_plain_message() {
    let f = machine();
    let d = f.disks();
    let log = logger();
    d.mount(&admin(), "sda1", &MountOpts::default(), &sink(&log)).unwrap();
    f.make_busy("/home/root/media/STICK");
    let e = d.unmount(&admin(), "sda1", &sink(&log)).unwrap_err();
    assert!(e.contains("could not tell what") && e.contains("Nothing was forced"), "{e}");
}

#[test]
fn the_running_system_is_never_unmounted_or_ejected() {
    let f = machine();
    let d = f.disks();
    let log = logger();
    let e = d.unmount(&admin(), "nvme0n1p2", &sink(&log)).unwrap_err();
    assert!(e.contains("running system"), "{e}");
    let e = d.unmount(&admin(), "nvme0n1p1", &sink(&log)).unwrap_err();
    assert!(e.contains("running system"), "{e}");
    let e = d.eject(&admin(), "nvme0n1", &sink(&log)).unwrap_err();
    assert!(e.contains("cannot be ejected") && e.contains("holds the running system"), "{e}");
    assert!(!f.calls().iter().any(|c| c.starts_with("umount") || c.starts_with("sys")), "{:?}", f.calls());
    assert!(actions(&log).iter().filter(|a| a.ends_with(".refused")).count() >= 3);
}

#[test]
fn eject_flushes_unmounts_everything_and_switches_a_usb_drive_off() {
    let f = machine();
    f.add_part("sda", "sda2", 2, 4_000_000, 1000, sb_ext4("second"));
    let d = f.disks();
    let log = logger();
    d.mount(&who(Role::User), "sda1", &MountOpts::default(), &sink(&log)).unwrap();
    d.mount(&who(Role::User), "sda2", &MountOpts::default(), &sink(&log)).unwrap();
    let msg = d.eject(&who(Role::User), "sda", &sink(&log)).unwrap();
    assert_eq!(msg, "You can remove it now.");
    let calls = f.calls();
    let um: Vec<&String> = calls.iter().filter(|c| c.starts_with("umount")).collect();
    assert_eq!(um.len(), 2);
    let last_umount = calls.iter().rposition(|c| c.starts_with("umount") || c.starts_with("rmdir")).unwrap();
    let off = calls.iter().position(|c| c == "sys /sys/block/sda/device/delete 1").unwrap();
    assert!(calls[..off].iter().filter(|c| *c == "sync").count() >= 2 && off > last_umount, "{calls:?}");
    assert!(d.scan().disk("sda").is_none(), "the drive is gone after the power-off");
    let l = log.borrow();
    let e = l.iter().find(|(a, _)| a == "disks.eject").unwrap();
    assert!(e.1.contains("drive=sda") && e.1.contains("serial=4C530001230815ABCD"), "{}", e.1);
}

#[test]
fn eject_stops_when_a_partition_is_busy_and_says_which_were_closed() {
    let f = machine();
    f.add_part("sda", "sda2", 2, 4_000_000, 1000, sb_ext4("second"));
    let d = f.disks();
    let log = logger();
    d.mount(&admin(), "sda1", &MountOpts::default(), &sink(&log)).unwrap();
    d.mount(&admin(), "sda2", &MountOpts::default(), &sink(&log)).unwrap();
    f.add_process(5, "cp", "/home/root/media/second", &[]);
    f.make_busy("/home/root/media/second");
    let e = d.eject(&admin(), "sda", &sink(&log)).unwrap_err();
    assert!(e.contains("cp (process 5)") && e.contains("sda1 already closed") && e.contains("not ejected"), "{e}");
    assert!(!f.calls().iter().any(|c| c.starts_with("sys ")), "no power-off");
}

#[test]
fn eject_of_internal_and_sd_drives_and_for_the_wrong_role() {
    let f = machine();
    let d = f.disks();
    let log = logger();
    let m = d.eject(&admin(), "sdb", &sink(&log)).unwrap();
    assert!(m.contains("internal drive"), "{m}");
    let m = d.eject(&admin(), "mmcblk0", &sink(&log)).unwrap();
    assert!(m.contains("You can remove it now") && !f.calls().iter().any(|c| c.starts_with("sys ")), "{m}");
    assert!(d.eject(&who(Role::User), "sdb", &sink(&log)).unwrap_err().contains("removable"));
    assert!(d.eject(&who(Role::Guest), "sda", &sink(&log)).unwrap_err().contains("Guests"));
    assert!(d.eject(&admin(), "sdq", &sink(&log)).unwrap_err().contains("no such drive"));
}

#[test]
fn the_fingerprint_changes_when_a_drive_appears_or_something_is_mounted() {
    let f = machine();
    let d = f.disks();
    let a = d.fingerprint();
    assert_eq!(a, d.fingerprint(), "stable while nothing changes");
    d.mount(&admin(), "sda1", &MountOpts::default(), &|_, _| {}).unwrap();
    let b = d.fingerprint();
    assert_ne!(a, b);
    f.add_disk("sdg", 1000, true, "/sys/devices/x/usb3/block/sdg", "New", "Stick", 512);
    assert_ne!(b, d.fingerprint());
}

// ---------------------------------------------------------------------------------------------------------------------
// the partition manager
// ---------------------------------------------------------------------------------------------------------------------

fn confirm(typed: &str) -> Confirm {
    Confirm { typed: typed.into(), second: true }
}

#[test]
fn the_system_disk_is_refused_by_the_engine_whatever_is_typed() {
    let f = machine();
    let d = f.disks();
    let log = logger();
    for op in [PartOp::NewTable(Table::Gpt), PartOp::Delete { number: 2 }, PartOp::Format { number: 1, fs: FormatFs::Ext4, label: "x".into() }, PartOp::Rename { number: 2, label: "x".into() }, PartOp::Add { size_bytes: None, fs: None, label: String::new() }] {
        let e = d.apply_partition(&admin(), "nvme0n1", &op, &confirm("nvme0n1"), &sink(&log)).unwrap_err();
        assert!(e.contains("running system") && e.contains("never changed"), "{op:?}: {e}");
        assert!(d.plan_partition(&admin(), "nvme0n1", &op).is_err());
    }
    assert!(!f.wrote(), "no byte was written: {:?}", f.calls());
    let l = log.borrow();
    assert!(l.iter().all(|(a, _)| a == "disks.partition.refused") && l.len() == 5);
    assert!(l[0].1.contains("drive=nvme0n1 size=512000000000 serial=S64DNX0T123456"), "{}", l[0].1);
}

#[test]
fn a_drive_holding_the_iyakki_volumes_is_a_system_disk_even_when_nothing_is_mounted() {
    let f = machine();
    f.add_disk("sdh", 300_000, true, "/sys/devices/x/usb4/4-1/block/sdh", "Boot", "Stick", 512);
    f.add_part("sdh", "sdh1", 1, 2048, 100_000, sb_fat32("IYAKKI-BOOT"));
    f.add_part("sdh", "sdh2", 2, 102_048, 100_000, sb_ext4("IYAKKI-DATA"));
    let d = f.disks();
    let s = d.scan();
    let disk = s.disk("sdh").unwrap();
    assert!(disk.system && disk.system_why.contains("IYAKKI system volume"), "{}", disk.system_why);
    assert!(d.eject(&admin(), "sdh", &|_, _| {}).unwrap_err().contains("cannot be ejected"));
    assert!(d.mount(&admin(), "sdh1", &MountOpts::default(), &|_, _| {}).unwrap_err().contains("running system"));
    assert!(d.apply_partition(&admin(), "sdh", &PartOp::NewTable(Table::Gpt), &confirm("sdh"), &|_, _| {}).is_err());
    assert!(!f.wrote());
}

#[test]
fn a_mounted_busy_swapping_or_write_protected_drive_is_refused() {
    let f = machine();
    blank_stick(&f);
    let d = f.disks();
    let log = logger();
    let op = PartOp::NewTable(Table::Gpt);
    d.mount(&admin(), "sda1", &MountOpts::default(), &sink(&log)).unwrap();
    // sda is open
    let e = d.apply_partition(&admin(), "sda", &op, &confirm("sda"), &sink(&log)).unwrap_err();
    assert!(e.contains("is open at /home/root/media/STICK") && e.contains("Unmount"), "{e}");
    // held by a RAID or encrypted volume
    f.set("/sys/block/sdc/holders/dm-0/x", "");
    let e = d.apply_partition(&admin(), "sdc", &op, &confirm("sdc"), &sink(&log)).unwrap_err();
    assert!(e.contains("in use by dm-0"), "{e}");
    f.st.lock().unwrap().files.remove("/sys/block/sdc/holders/dm-0/x");
    // write-protected
    f.set("/sys/block/sdc/ro", "1\n");
    assert!(d.apply_partition(&admin(), "sdc", &op, &confirm("sdc"), &sink(&log)).unwrap_err().contains("write-protected"));
    f.set("/sys/block/sdc/ro", "0\n");
    // swap in use
    f.set("/proc/swaps", "x");
    f.st.lock().unwrap().swaps = "Filename Type Size Used Priority\n/dev/sdc1 partition 100 0 -2\n".into();
    f.add_part("sdc", "sdc1", 1, 2048, 100_000, sb_swap());
    let e = d.apply_partition(&admin(), "sdc", &op, &confirm("sdc"), &sink(&log)).unwrap_err();
    assert!(e.contains("swap"), "{e}");
    assert!(!f.wrote());
}

#[test]
fn only_an_administrator_changes_partitions() {
    let f = machine();
    blank_stick(&f);
    let d = f.disks();
    for r in [Role::Operator, Role::User, Role::Guest] {
        let e = d.apply_partition(&who(r), "sdc", &PartOp::NewTable(Table::Gpt), &confirm("sdc"), &|_, _| {}).unwrap_err();
        assert!(e.contains("Only an administrator"), "{r:?}: {e}");
    }
    assert!(!f.wrote());
}

#[test]
fn the_confirmation_needs_the_name_or_the_serial_tail_and_the_second_button() {
    let f = machine();
    blank_stick(&f);
    let d = f.disks();
    let log = logger();
    let op = PartOp::NewTable(Table::Gpt);
    let a = admin();
    for (typed, second, needle) in [("", true, "Type the drive's name (sdc)"), ("sdd", true, "not this drive's name"), ("SERIAL-SDC-9F3B", true, "not this drive's name"), ("3A", true, "not this drive's name"), ("sdc", false, "second confirmation")] {
        let e = d.apply_partition(&a, "sdc", &op, &Confirm { typed: typed.into(), second }, &sink(&log)).unwrap_err();
        assert!(e.contains(needle), "{typed:?}: {e}");
    }
    assert!(!f.wrote());
    assert_eq!(log.borrow().len(), 5, "every refusal is audited");
    // the name, in any case and with the path, or the last four of the serial
    for typed in ["sdc", "SDC", "/dev/sdc", "9F3A", "9f3a"] {
        let f2 = machine();
        blank_stick(&f2);
        let r = f2.disks().apply_partition(&a, "sdc", &op, &confirm(typed), &|_, _| {});
        assert!(r.is_ok(), "{typed}: {r:?}");
    }
    let disk = f.disks().scan().disk("sdc").unwrap().clone();
    assert!(check_confirmation(&disk, &confirm("sdc")).is_ok());
    // a drive without a serial: only the name works
    let mut bare = disk.clone();
    bare.serial.clear();
    assert!(check_confirmation(&bare, &confirm("")).unwrap_err().contains("(sdc) to confirm"));
    assert!(check_confirmation(&bare, &confirm("")).is_err() && check_confirmation(&bare, &confirm("9F3A")).is_err());
}

#[test]
fn the_dry_run_shows_the_plan_with_name_size_and_serial_and_writes_nothing() {
    let f = machine();
    blank_stick(&f);
    let d = f.disks();
    let plan = d.plan_partition(&admin(), "sdc", &PartOp::NewTable(Table::Gpt)).unwrap();
    assert!(plan.writes_anything() && !f.wrote());
    let t = plan.render();
    assert!(t.starts_with("Make a new GPT partition table, erasing EVERYTHING on sdc (Generic Flash Drive, 134.2 MB, serial SERIAL-SDC-9F3A)"), "{t}");
    assert!(t.contains("What will be lost:") && t.contains("The plan:") && t.contains("1. Erase the start of the drive") && t.contains("protective MBR"), "{t}");
    assert!(!f.wrote());
    assert!(d.plan_partition(&admin(), "sdc", &PartOp::Reread).unwrap().render().contains("read the partition table of /dev/sdc again"));
}

#[test]
fn the_layout_of_a_whole_flow_gpt_add_format_rename_delete() {
    let f = machine();
    blank_stick(&f);
    let d = f.disks();
    let log = logger();
    let a = admin();
    let c = confirm("sdc");
    let go = |op: PartOp| d.apply_partition(&a, "sdc", &op, &c, &sink(&log));
    // nothing to add before a table exists
    assert!(go(PartOp::Add { size_bytes: None, fs: None, label: String::new() }).unwrap_err().contains("new partition table first"));
    go(PartOp::NewTable(Table::Gpt)).unwrap();
    assert_eq!(d.scan().disk("sdc").unwrap().table, Table::Gpt);
    go(PartOp::Add { size_bytes: Some(40 << 20), fs: Some(FormatFs::Ext4), label: "data".into() }).unwrap();
    go(PartOp::Add { size_bytes: None, fs: Some(FormatFs::Fat32), label: "backup".into() }).unwrap();
    let s = d.scan();
    let disk = s.disk("sdc").unwrap();
    assert_eq!(disk.parts.len(), 2);
    let (p1, p2) = (&disk.parts[0], &disk.parts[1]);
    assert_eq!((p1.name.as_str(), p1.fs, p1.label.as_str(), p1.size_bytes, p1.start_bytes), ("sdc1", Fs::Ext4, "data", 40 << 20, 1 << 20));
    assert_eq!((p2.name.as_str(), p2.fs, p2.label.as_str()), ("sdc2", Fs::Fat32, "BACKUP"), "FAT labels are upper case");
    assert!(p2.start_bytes == p1.start_bytes + p1.size_bytes && p2.start_bytes % (1 << 20) == 0, "1 MiB aligned, no overlap");
    assert!(p2.size_bytes > 80 << 20);
    assert_eq!(p1.type_name, "Linux filesystem");
    assert_eq!(p2.type_name, "Microsoft basic data");
    // the new FAT32 volume is a real one: the boot sector, both tables, the label entry
    let raw = f.dev_bytes("sdc");
    let at = p2.start_bytes as usize;
    assert_eq!(&raw[at + 82..at + 90], b"FAT32   ");
    assert_eq!(u16::from_le_bytes([raw[at + 11], raw[at + 12]]), 512);
    let (spc, res, fat) = (raw[at + 13] as usize, u16::from_le_bytes([raw[at + 14], raw[at + 15]]) as usize, u32::from_le_bytes([raw[at + 36], raw[at + 37], raw[at + 38], raw[at + 39]]) as usize);
    assert_eq!((spc, res), (1, 32));
    assert_eq!(u32::from_le_bytes([raw[at + res * 512], raw[at + res * 512 + 1], raw[at + res * 512 + 2], raw[at + res * 512 + 3]]), 0x0FFF_FFF8);
    assert_eq!(&raw[at + (res + 2 * fat) * 512..at + (res + 2 * fat) * 512 + 6], b"BACKUP");
    assert_eq!(raw[at + (res + 2 * fat) * 512 + 11], 0x08);
    assert_eq!(&raw[at + 6 * 512 + 82..at + 6 * 512 + 90], b"FAT32   ", "the backup boot sector");
    // rename both
    go(PartOp::Rename { number: 2, label: "photos".into() }).unwrap();
    go(PartOp::Rename { number: 1, label: "work".into() }).unwrap();
    let s = d.scan();
    let disk = s.disk("sdc").unwrap();
    assert_eq!((disk.parts[0].label.as_str(), disk.parts[1].label.as_str()), ("work", "PHOTOS"));
    let raw = f.dev_bytes("sdc");
    assert_eq!(&raw[at + (res + 2 * fat) * 512..at + (res + 2 * fat) * 512 + 6], b"PHOTOS", "the root folder's label entry follows");
    // format partition 1 as FAT32, then delete it
    go(PartOp::Format { number: 1, fs: FormatFs::Fat32, label: "FRESH".into() }).unwrap();
    assert_eq!(d.scan().disk("sdc").unwrap().parts[0].fs, Fs::Fat32);
    go(PartOp::Delete { number: 1 }).unwrap();
    let s = d.scan();
    let disk = s.disk("sdc").unwrap();
    assert_eq!(disk.parts.len(), 1);
    assert_eq!(disk.parts[0].number, 2);
    // the audit trail names the drive, size and serial for every step, and never file contents
    let l = log.borrow();
    let done = l.iter().filter(|(a, _)| a == "disks.partition.done").count();
    assert_eq!(done, 7, "{:?}", actions(&log));
    for (a, det) in l.iter() {
        if a.starts_with("disks.partition.") {
            assert!(det.contains("drive=sdc size=134217728 serial=SERIAL-SDC-9F3A"), "{a}: {det}");
        }
    }
    assert!(l.iter().any(|(a, det)| a == "disks.partition.apply" && det.contains("op=add") && det.contains("fs=ext4")));
    assert!(f.calls().iter().any(|c| c.starts_with("mke2fs /dev/sdc1 data ")));
}

#[test]
fn an_mbr_drive_gets_partitions_with_the_right_type_bytes() {
    let f = machine();
    blank_stick(&f);
    let d = f.disks();
    let a = admin();
    let c = confirm("sdc");
    d.apply_partition(&a, "sdc", &PartOp::NewTable(Table::Mbr), &c, &|_, _| {}).unwrap();
    assert_eq!(d.scan().disk("sdc").unwrap().table, Table::Mbr);
    d.apply_partition(&a, "sdc", &PartOp::Add { size_bytes: Some(50 << 20), fs: Some(FormatFs::Fat32), label: "win".into() }, &c, &|_, _| {}).unwrap();
    d.apply_partition(&a, "sdc", &PartOp::Add { size_bytes: None, fs: Some(FormatFs::Ext4), label: "lin".into() }, &c, &|_, _| {}).unwrap();
    let raw = f.dev_bytes("sdc");
    assert_eq!((raw[446 + 4], raw[446 + 16 + 4]), (0x0C, 0x83));
    assert_eq!(&raw[510..512], &[0x55, 0xAA]);
    assert_eq!(u32::from_le_bytes([raw[446 + 8], raw[446 + 9], raw[446 + 10], raw[446 + 11]]), 2048);
    let s = d.scan();
    let disk = s.disk("sdc").unwrap();
    assert_eq!((disk.parts[0].fs, disk.parts[1].fs), (Fs::Fat32, Fs::Ext4));
    d.apply_partition(&a, "sdc", &PartOp::Delete { number: 1 }, &c, &|_, _| {}).unwrap();
    assert_eq!(d.scan().disk("sdc").unwrap().parts.len(), 1);
    // an extended partition is not touched
    let mut raw = f.dev_bytes("sdc");
    raw[446 + 16 + 4] = 0x05;
    f.set_dev("sdc", raw);
    let e = d.plan_partition(&a, "sdc", &PartOp::Delete { number: 2 }).unwrap_err();
    assert!(e.contains("extended partition") && e.contains("not supported yet"), "{e}");
}

#[test]
fn what_is_not_supported_says_so_and_changes_nothing() {
    let f = machine();
    blank_stick(&f);
    let d = f.disks();
    let a = admin();
    let c = confirm("sdc");
    d.apply_partition(&a, "sdc", &PartOp::NewTable(Table::Gpt), &c, &|_, _| {}).unwrap();
    d.apply_partition(&a, "sdc", &PartOp::Add { size_bytes: None, fs: Some(FormatFs::Fat32), label: "x".into() }, &c, &|_, _| {}).unwrap();
    let before = f.dev_bytes("sdc");
    let n = f.calls().len();
    for (op, needle) in [
        (PartOp::Resize { number: 1, size_bytes: 1 << 20 }, "Resizing is not supported yet"),
        (PartOp::Format { number: 1, fs: FormatFs::Exfat, label: "x".into() }, "exFAT is not supported yet"),
        (PartOp::Format { number: 1, fs: FormatFs::Ntfs, label: "x".into() }, "NTFS is not supported yet"),
        (PartOp::Add { size_bytes: None, fs: Some(FormatFs::Ntfs), label: "x".into() }, "NTFS is not supported yet"),
        (PartOp::Rename { number: 1, label: "waytoolonglabelname".into() }, "11 characters"),
        (PartOp::Rename { number: 1, label: "a.b".into() }, "letters, digits"),
        (PartOp::Delete { number: 7 }, "no partition 7"),
    ] {
        let e = d.apply_partition(&a, "sdc", &op, &c, &|_, _| {}).unwrap_err();
        assert!(e.contains(needle), "{op:?}: {e}");
    }
    assert_eq!(f.dev_bytes("sdc"), before);
    assert_eq!(f.calls().len(), n, "not a single call after the refusals");
    // renaming a file system that has no in-place label change
    assert!(d.rename_steps("/dev/sda1", Fs::Ntfs, "x").unwrap_err().contains("not supported yet"));
}

#[test]
fn a_full_table_or_no_free_space_is_reported() {
    let f = machine();
    blank_stick(&f);
    let d = f.disks();
    let a = admin();
    let c = confirm("sdc");
    d.apply_partition(&a, "sdc", &PartOp::NewTable(Table::Mbr), &c, &|_, _| {}).unwrap();
    let e = d.plan_partition(&a, "sdc", &PartOp::Add { size_bytes: Some(gb(1)), fs: None, label: String::new() }).unwrap_err();
    assert!(e.contains("no free space of 1.0 GB"), "{e}");
    for _ in 0..4 {
        d.apply_partition(&a, "sdc", &PartOp::Add { size_bytes: Some(20 << 20), fs: None, label: String::new() }, &c, &|_, _| {}).unwrap();
    }
    assert!(d.plan_partition(&a, "sdc", &PartOp::Add { size_bytes: Some(1 << 20), fs: None, label: String::new() }).unwrap_err().contains("table is full"));
}

#[test]
fn reading_the_table_again_needs_no_confirmation_and_a_failed_reread_is_reported() {
    let f = machine();
    blank_stick(&f);
    let d = f.disks();
    let log = logger();
    d.apply_partition(&admin(), "sdc", &PartOp::Reread, &Confirm::default(), &sink(&log)).unwrap();
    assert!(f.calls().contains(&"reread /dev/sdc".to_string()) && !f.wrote());
    f.st.lock().unwrap().reread_fails = true;
    let e = d.apply_partition(&admin(), "sdc", &PartOp::NewTable(Table::Gpt), &confirm("sdc"), &sink(&log)).unwrap_err();
    assert!(e.contains("Stopped") && e.contains("busy") && e.contains("look at the drive"), "{e}");
    assert!(actions(&log).contains(&"disks.partition.failed".to_string()));
}

#[test]
fn fat32_structures_are_valid_for_both_sector_sizes_and_small_sizes_are_refused() {
    for (size, ss) in [(100u64 << 20, 512u32), (1 << 30, 512), (300 << 20, 4096), (40 << 30, 512)] {
        let steps = fat32_steps("/dev/x1", size, ss, 2048, "VOL", 0xDEADBEEF).unwrap();
        let Step::Write { data, .. } = &steps[1] else { panic!("boot sector first") };
        assert_eq!(data.len(), ss as usize);
        let p = detect(data);
        assert_eq!((p.fs, p.label.as_str(), p.uuid.as_str()), (Fs::Fat32, "VOL", "DEAD-BEEF"), "{size} {ss}");
        let (bps, spc, res, nfats) = (u16::from_le_bytes([data[11], data[12]]) as u64, data[13] as u64, u16::from_le_bytes([data[14], data[15]]) as u64, data[16] as u64);
        let (total, fat) = (u32::from_le_bytes([data[32], data[33], data[34], data[35]]) as u64, u32::from_le_bytes([data[36], data[37], data[38], data[39]]) as u64);
        assert_eq!((bps, nfats, total * bps), (ss as u64, 2, size / bps * bps));
        let clusters = (total - res - nfats * fat) / spc;
        assert!(clusters >= 65_525, "FAT32 needs at least 65525 clusters: {clusters}");
        assert!(fat * bps / 4 >= clusters + 2, "the table covers every cluster");
        // nothing is written beyond the metadata and nothing outside the partition
        for s in &steps {
            match s {
                Step::Zero { offset, len, .. } => assert!(offset + len <= size),
                Step::Write { offset, data, .. } => assert!(offset + data.len() as u64 <= size),
                _ => {}
            }
        }
    }
    assert!(fat32_steps("/dev/x1", 20 << 20, 512, 0, "", 1).unwrap_err().contains("too small for FAT32"));
    assert!(fat32_steps("/dev/x1", 5 << 40, 512, 0, "", 1).unwrap_err().contains("2 TB"));
    assert_eq!(fat_label("my disk").unwrap(), "MY DISK");
    assert!(fat_label("bad*").is_err() && fat_label("123456789012").is_err());
}

#[test]
fn an_ext4_label_change_keeps_the_superblock_checksum_valid() {
    let f = machine();
    f.add_part("sda", "sdz1", 9, 0, 100_000, sb_ext4("old"));
    let d = f.disks();
    let steps = d.rename_steps("/dev/sdz1", Fs::Ext4, "newname").unwrap();
    let Step::Write { offset, data, .. } = &steps[0] else { panic!() };
    assert_eq!((*offset, data.len()), (1024, 1024));
    assert_eq!(&data[120..127], b"newname");
    assert_eq!(data[127], 0);
    let stored = u32::from_le_bytes([data[1020], data[1021], data[1022], data[1023]]);
    assert_eq!(stored, crc32c_raw(0xFFFF_FFFF, &data[..1020]));
    assert_ne!(stored, 0);
    assert!(d.rename_steps("/dev/sdz1", Fs::Ext4, "a-label-that-is-way-too-long").unwrap_err().contains("16 characters"));
}

#[test]
fn the_real_environment_refuses_paths_outside_dev_and_sys() {
    let r = crate::disks_sys::RealEnv;
    assert!(r.dev_read("/etc/passwd", 0, 10).is_err());
    assert!(r.dev_write("/tmp/x", 0, b"x").is_err());
    assert!(r.dev_zero("/dev/../etc/x", 0, 1).is_err());
    assert!(r.write_sys("/etc/x", "1").is_err() && r.write_sys("/sys/../etc/x", "1").is_err());
    assert!(r.mount("/etc/passwd", "/mnt", "ext4", MountFlags::default(), "").is_err());
    assert!(r.mke2fs("/tmp/file", "x", "u").is_err());
}

#[test]
fn drives_open_in_the_signed_in_persons_home_and_a_user_closes_only_their_own() {
    let f = machine();
    f.add_part("sda", "sda2", 2, 4_000_000, 1000, sb_ext4("second"));
    let d = f.disks();
    let log = logger();
    let (sam, root) = (who(Role::User), admin());
    let a = d.mount(&sam, "sda1", &MountOpts::default(), &sink(&log)).unwrap();
    let b = d.mount(&root, "sda2", &MountOpts::default(), &sink(&log)).unwrap();
    assert_eq!((a.target.as_str(), b.target.as_str()), ("/home/sam/media/STICK", "/home/root/media/second"));
    assert!(!f.calls().iter().any(|c| c.contains(" /media")), "nothing under /media");
    // mount options follow the signed-in person
    let (_, _, _, _, data) = f.last_mount().unwrap();
    assert!(data.is_empty(), "ext4 has no owner options");
    assert_eq!(d.mounted_in("/home/sam"), [("sda1".to_string(), a.target.clone())]);
    assert_eq!(d.mounted_in("/home/root"), [("sda2".to_string(), b.target.clone())]);
    // the user cannot close what is open in someone else's folder; the administrator can
    let e = d.unmount(&sam, "sda2", &sink(&log)).unwrap_err();
    assert!(e.contains("another person's folder"), "{e}");
    assert!(d.eject(&sam, "sda", &sink(&log)).unwrap_err().contains("another person's folder"));
    assert!(d.unmount(&root, "sda2", &sink(&log)).is_ok());
    // busy-unmount still names the programs with the new path
    f.add_process(9, "vlc", "/home/sam/media/STICK", &[]);
    f.make_busy("/home/sam/media/STICK");
    assert!(d.unmount(&sam, "sda1", &sink(&log)).unwrap_err().contains("vlc (process 9)"));
    // the folders go when empty after unmount
    f.st.lock().unwrap().busy.clear();
    d.unmount(&sam, "sda1", &sink(&log)).unwrap();
    assert!(f.calls().contains(&"rmdir /home/sam/media/STICK".to_string()));
    // the system drive logic is not fooled by the paths
    assert!(d.scan().disk("nvme0n1").unwrap().system);
}

#[test]
fn sign_out_closes_nothing_by_itself_and_says_a_drive_is_still_open_unless_the_switch_is_on() {
    let tmp = std::env::temp_dir().join(format!("disks-signout-{}", std::process::id()));
    let home = tmp.join("home/sam");
    std::fs::create_dir_all(&home).unwrap();
    let home_s = home.display().to_string();
    let f = machine();
    let d = f.disks();
    let log = logger();
    let mut sam = who(Role::User);
    sam.home = home_s.clone();
    assert!(d.at_sign_out(&sam, &sink(&log)).is_none(), "nothing open, nothing to say");
    d.mount(&sam, "sda1", &MountOpts::default(), &sink(&log)).unwrap();
    // default: off
    assert!(!signout_pref(&home_s));
    let n = f.calls().len();
    let msg = d.at_sign_out(&sam, &sink(&log)).unwrap();
    assert!(msg.contains("still open in your home folder") && msg.contains("STICK"), "{msg}");
    assert!(f.calls()[n..].iter().all(|c| !c.starts_with("umount")), "nothing was unmounted");
    assert!(actions(&log).contains(&"disks.signout.left_open".to_string()));
    // on: closed, plainly
    set_signout_pref(&home_s, true).unwrap();
    assert!(signout_pref(&home_s));
    assert!(d.at_sign_out(&sam, &sink(&log)).is_none());
    assert!(d.mounted_in(&home_s).is_empty());
    // on but busy: nothing forced, the person is told
    d.mount(&sam, "sda1", &MountOpts::default(), &sink(&log)).unwrap();
    f.make_busy(&format!("{home_s}/media/STICK"));
    let msg = d.at_sign_out(&sam, &sink(&log)).unwrap();
    assert!(msg.contains("stay open") && msg.contains("Nothing was forced"), "{msg}");
    set_signout_pref(&home_s, false).unwrap();
    assert!(!signout_pref(&home_s));
    let _ = std::fs::remove_dir_all(&tmp);
    // the account's owner from a passwd file (root's entry gives none)
    let pw = tmp.with_extension("passwd");
    std::fs::write(&pw, "root:x:0:0::/root:/bin/sh\nsam:x:1005:1006::/home/sam:/bin/sh\n").unwrap();
    assert_eq!(account_owner("sam", &pw.display().to_string()), Some((1005, 1006)));
    assert_eq!(account_owner("root", &pw.display().to_string()), None);
    assert_eq!(account_owner("nobody", &pw.display().to_string()), None);
    let _ = std::fs::remove_file(pw);
}
