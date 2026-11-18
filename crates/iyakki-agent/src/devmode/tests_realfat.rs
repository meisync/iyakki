//! The update engine on a REAL FAT32 boot partition and a REAL ext4 data volume, through the real mount calls of `SysBoot` (ignored by default).
//!
//! Needs root, `losetup`, `sfdisk`, `mkfs.vfat`, `mkfs.ext4`, `fsck.fat`, `e2fsck` and `IYAKKI_IN_SANDBOX=1`; run it only through `dist/linux-host/kernel-tests.sh`, which starts it in a
//! throw-away network and mount namespace (`unshare -nm`: nothing it mounts is seen by the machine, and a tmpfs covers `/run` first). A 256 MB disk image on a loop device gets
//! two partitions like a stick or a T7 (FAT32 labelled IYAKKI-BOOT, ext4 for the data); the data volume is mounted, the engine finds the boot partition on the same disk, mounts
//! it itself and installs a signed update pack, then rolls back. After each step the boot partition is mounted again and compared, and `fsck.fat` and `e2fsck` must find the
//! volumes clean. Rows: F-DEV-09, F-UPD-05 (the real FAT mount of the boot partition).

use std::path::{Path, PathBuf};
use std::process::Command;

use super::bootmount::SysBoot;
use super::centre::*;
use super::tests_update::*;
use super::update::*;

fn sh(cmd: &str) -> (bool, String) {
    let o = Command::new("sh").args(["-c", &format!("export PATH=$PATH:/usr/sbin:/sbin; {cmd}")]).output().unwrap();
    (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

fn must(cmd: &str) -> String {
    let (ok, out) = sh(cmd);
    assert!(ok, "{cmd}: {out}");
    out
}

/// Detaches the loop device and unmounts what is left, also when the test fails.
struct Cleanup {
    dev: String,
    dirs: Vec<PathBuf>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for d in &self.dirs {
            let _ = sh(&format!("umount {} 2>/dev/null", d.display()));
        }
        let _ = sh(&format!("losetup -d {}", self.dev));
    }
}

fn key() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[7u8; 32])
}

#[test]
#[ignore]
fn the_update_engine_installs_and_rolls_back_on_a_real_fat32_boot_partition_through_the_real_mount() {
    // SAFETY: getuid has no preconditions.
    let root = unsafe { libc::getuid() } == 0;
    let tools = ["losetup", "sfdisk", "mkfs.vfat", "mkfs.ext4", "fsck.fat", "e2fsck", "mount", "umount"].iter().all(|t| sh(&format!("command -v {t} >/dev/null")).0);
    if !(root && tools && std::env::var("IYAKKI_IN_SANDBOX").as_deref() == Ok("1")) {
        crate::qeprintln!("skipped: needs root, losetup, sfdisk, mkfs.vfat, mkfs.ext4, fsck.fat, e2fsck and the sandbox of dist/linux-host/kernel-tests.sh");
        return;
    }
    must("mount -t tmpfs tmpfs /run");
    let top = PathBuf::from(format!("/tmp/iyakki-realfat-{}", std::process::id()));
    let (data, bootmnt, rigdir) = (top.join("data"), top.join("bootmnt"), top.join("rig"));
    for d in [&data, &bootmnt, &rigdir] {
        std::fs::create_dir_all(d).unwrap();
    }
    let img = top.join("disk.img");
    must(&format!("truncate -s 512M {i} && printf 'label: dos\\n,200M,c\\n,,83\\n' | sfdisk -q {i}", i = img.display()));
    let dev = must(&format!("losetup -f --show -P {}", img.display())).trim().to_string();
    let mut guard = Cleanup { dev: dev.clone(), dirs: vec![] };
    let (p1, p2) = (format!("{dev}p1"), format!("{dev}p2"));
    for _ in 0..50 {
        if Path::new(&p2).exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(Path::new(&p1).exists() && Path::new(&p2).exists(), "the partitions of {dev} did not appear");
    // the partition nodes can come and go for a moment while the kernel and udev settle after `losetup -P`: try again a few times
    let mut made = (false, String::new());
    for _ in 0..30 {
        made = sh(&format!("mkfs.vfat -F 32 -n IYAKKI-BOOT {p1} >/dev/null && mkfs.ext4 -q -L IYAKKI-DATA {p2}"));
        if made.0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    assert!(made.0, "mkfs on the loop partitions: {}", made.1);
    // the device as it was before the update: the same files as the folder rig of the other tests
    must(&format!("mount -t vfat {p1} {}", bootmnt.display()));
    guard.dirs.push(bootmnt.clone());
    put(&bootmnt, "boot/bzImage", &pseudo(1 << 20, 1));
    put(&bootmnt, "boot/initrd", &pseudo(6 << 20, 2));
    put(&bootmnt, "boot/grub/grub.cfg", board_cfg().as_bytes());
    put(&bootmnt, "boot/grub/themes/iyakki/theme.txt", b"theme v1\n");
    put(&bootmnt, "EFI/BOOT/bootx64.efi", &pseudo(300_000, 3));
    let old = tree_hash(&bootmnt, &[]);
    must(&format!("umount {}", bootmnt.display()));
    must(&format!("mount -t ext4 {p2} {}", data.display()));
    guard.dirs.push(data.clone());
    put(&data, "settings.json", b"{\"mine\":1}");
    put(&data, "home/root/notes.txt", b"my notes");
    put(&data, "vault/system.afv", b"vault bytes");
    let skip = ["devmode", "boot-next", "boot-prev", "boot-rollback", TRIAL_FILE, "lost+found"];
    let data_before = tree_hash(&data, &skip);
    let dm = super::DevMode::open(&data);
    let r = Rig { dir: rigdir.clone(), boot: DirBoot { root: top.join("unused-bootpart"), capacity: 14 << 20, cluster: 4096 }, dm, release: "VERSION=0.1.0-alpha.1\nVARIANT=macweb\nBUILD_DATE=2026-10-20\n".into() };
    let c = Centre::new(&data, Box::new(SysBoot::real(&data)), r.release.clone(), false, Some(key().verifying_key()));
    let located = SysBoot::real(&data).locate().expect("the engine finds the boot partition on the same disk as the data volume");
    assert_eq!(located.node, Path::new("/dev").join(p1.rsplit('/').next().unwrap()));
    assert!(located.mounted.is_none(), "the engine mounts it itself");

    // install a signed pack
    let (dir, _) = payload(&r, "0.1.0-alpha.2", "macweb", false, 7);
    sign_pack(&dir, &key()).unwrap();
    let f = read_pack(&dir, c.key()).unwrap();
    let pv = c.preview(&f).unwrap();
    assert!(pv.confirm.is_empty(), "{:?}", pv.confirm);
    let msg = c.install(&f, false, "admin").unwrap();
    assert!(msg.contains("Restart when you are ready"), "{msg}");
    must(&format!("mount -t vfat -o ro {p1} {}", bootmnt.display()));
    for fe in &f.manifest.files {
        if fe.path != "boot/grub/grub.cfg" {
            assert_eq!(hash(&find_ci(&bootmnt, &fe.path).unwrap()), fe.sha256, "{} on the real FAT volume", fe.path);
        }
    }
    let cfg = std::fs::read_to_string(find_ci(&bootmnt, "boot/grub/grub.cfg").unwrap()).unwrap();
    assert!(cfg.contains(PREV_TITLE), "the menu keeps the previous version");
    let new_tree = tree_hash(&bootmnt, &[]);
    must(&format!("umount {}", bootmnt.display()));
    let fsck = must(&format!("fsck.fat -n {p1} 2>&1; true"));
    assert!(!fsck.to_lowercase().contains("differ") && !fsck.contains("Dirty bit") && !fsck.contains("Start does not point"), "fsck.fat after the install:\n{fsck}");
    assert!(sh(&format!("fsck.fat -n {p1}")).0, "fsck.fat finds the boot partition clean after the install:\n{fsck}");
    assert_eq!(tree_hash(&data, &skip), data_before, "the person's data is untouched");
    let v = c.versions();
    assert_eq!((v.on_disk.as_str(), v.previous.as_deref()), ("0.1.0-alpha.2", Some("0.1.0-alpha.1")));
    assert!(data.join("boot-prev").is_dir(), "the previous kernel and initramfs are on the ext4 data volume");

    // and back
    let msg = c.rollback("admin").unwrap();
    assert!(msg.to_lowercase().contains("restart"), "{msg}");
    must(&format!("mount -t vfat -o ro {p1} {}", bootmnt.display()));
    // the rollback puts the kernel and initramfs back (the menu entry and the other files are the new version's)
    for p in ["boot/bzImage", "boot/initrd", "EFI/BOOT/bootx64.efi"] {
        let was = old.iter().find(|(q, _)| q == p).unwrap_or_else(|| panic!("{p} was not in the first tree")).1.clone();
        assert_eq!(hash(&find_ci(&bootmnt, p).unwrap()), was, "{p} is the old file again after the rollback");
    }
    must(&format!("umount {}", bootmnt.display()));
    assert!(sh(&format!("fsck.fat -n {p1}")).0, "fsck.fat finds the boot partition clean after the rollback");
    let v = c.versions();
    assert_eq!((v.on_disk.as_str(), v.previous.as_deref()), ("0.1.0-alpha.1", Some("0.1.0-alpha.2")), "the version left is kept for rolling forward");
    // and forward again
    c.rollback("admin").unwrap();
    must(&format!("mount -t vfat -o ro {p1} {}", bootmnt.display()));
    for p in ["boot/bzImage", "boot/initrd"] {
        let want = new_tree.iter().find(|(q, _)| q == p).unwrap().1.clone();
        assert_eq!(hash(&find_ci(&bootmnt, p).unwrap()), want, "{p} is the new file again after rolling forward");
    }
    must(&format!("umount {}", bootmnt.display()));
    assert!(sh(&format!("fsck.fat -n {p1}")).0, "fsck.fat finds the boot partition clean after rolling forward");
    must(&format!("umount {}", data.display()));
    assert!(sh(&format!("e2fsck -fn {p2}")).0, "e2fsck finds the data volume clean");
    drop(guard);
    drop(r);
    let _ = std::fs::remove_dir_all(&top);
}
