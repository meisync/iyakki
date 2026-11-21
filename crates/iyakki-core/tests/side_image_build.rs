//! The build side of the side image (F-BOOT-25): `dist/buildroot/side-image.sh`, the boot menu switch and the kernel options the device side needs.
//! The device side itself is tested in `iyakki_core::sideimage` against a fake host.

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn have(tool: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {tool} >/dev/null 2>&1")]).status().is_ok_and(|s| s.success())
}

fn script(args: &[&str]) -> std::process::Output {
    Command::new("sh").arg(root().join("dist/buildroot/side-image.sh")).args(args).output().unwrap()
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-side-build-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn tree(d: &Path, release: &str) -> PathBuf {
    let t = d.join("rootfs");
    std::fs::create_dir_all(t.join(format!("lib/modules/{release}"))).unwrap();
    std::fs::create_dir_all(t.join("lib/firmware/brcm")).unwrap();
    std::fs::create_dir_all(t.join("bin")).unwrap();
    std::fs::write(t.join(format!("lib/modules/{release}/modules.dep")), "kernel/x.ko:\n").unwrap();
    std::fs::write(t.join(format!("lib/modules/{release}/x.ko")), vec![7u8; 4096]).unwrap();
    std::fs::write(t.join("lib/firmware/brcm/fw.bin"), vec![9u8; 8192]).unwrap();
    std::fs::write(t.join("bin/init"), "#!/bin/sh\n").unwrap();
    t
}

#[test]
fn the_script_parses_and_refuses_what_it_cannot_split_safely() {
    assert!(Command::new("sh").arg("-n").arg(root().join("dist/buildroot/side-image.sh")).status().unwrap().success());
    assert!(!script(&[]).status.success(), "no command: usage");
    let d = scratch("refuse");
    let t = tree(&d, "6.12.1-iyakki");
    std::fs::remove_file(t.join("lib/modules/6.12.1-iyakki/modules.dep")).unwrap();
    if have("mksquashfs") {
        let o = script(&["split", t.to_str().unwrap(), d.join("out").to_str().unwrap()]);
        assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("modules.dep"), "{}", String::from_utf8_lossy(&o.stderr));
    }
    // two kernels in one tree are not guessed between
    let t2 = tree(&d, "6.12.1-iyakki");
    std::fs::create_dir_all(t2.join("lib/modules/6.13.0")).unwrap();
    if have("mksquashfs") {
        let o = script(&["split", t2.to_str().unwrap(), d.join("out2").to_str().unwrap()]);
        assert!(String::from_utf8_lossy(&o.stderr).contains("exactly one kernel"));
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_split_names_the_side_file_by_the_kernel_release_and_leaves_the_big_folders_out_of_the_small_ram_disk() {
    if !(have("mksquashfs") && have("cpio") && have("gzip")) {
        eprintln!("skipped: mksquashfs, cpio or gzip is not installed");
        return;
    }
    let d = scratch("split");
    let t = tree(&d, "6.12.1-iyakki");
    let out = d.join("out");
    let o = script(&["split", t.to_str().unwrap(), out.to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let side = out.join("side-6.12.1-iyakki.sqfs");
    assert!(std::fs::metadata(&side).unwrap().len() > 0);
    // the file name is the one the device side asks for
    assert_eq!(iyakki_core::sideimage::file_name("6.12.1-iyakki").unwrap(), "side-6.12.1-iyakki.sqfs");
    // a squashfs superblock ("hsqs", little endian)
    assert_eq!(&std::fs::read(&side).unwrap()[..4], b"hsqs");
    if have("unsquashfs") {
        let l = Command::new("unsquashfs").args(["-l"]).arg(&side).output().unwrap();
        let list = String::from_utf8_lossy(&l.stdout).into_owned();
        assert!(list.contains("lib/modules/6.12.1-iyakki/modules.dep") && list.contains("lib/firmware/brcm/fw.bin"), "{list}");
        assert!(!list.contains("bin/init"), "only the two folders are in the side file");
    }
    // the small RAM disk: gzip cpio with the rest of the tree and none of the two folders' files
    let l = Command::new("sh").args(["-c", &format!("gzip -dc '{}' | cpio -t --quiet", out.join("initrd-small").display())]).output().unwrap();
    let list = String::from_utf8_lossy(&l.stdout).into_owned();
    assert!(list.lines().any(|x| x == "bin/init"), "{list}");
    assert!(!list.contains("modules.dep") && !list.contains("fw.bin") && !list.contains("x.ko"), "{list}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_menu_switch_is_written_only_beside_both_files_and_the_boot_size_follows_the_content() {
    let d = scratch("grubcfg");
    let boot = d.join("boot-volume");
    std::fs::create_dir_all(boot.join("boot/grub")).unwrap();
    let b = boot.to_str().unwrap();
    assert!(!script(&["grubcfg", b]).status.success(), "no side file: no switch");
    std::fs::write(boot.join("boot/side-6.12.1-iyakki.sqfs"), b"hsqs").unwrap();
    assert!(!script(&["grubcfg", b]).status.success(), "no small RAM disk: no switch");
    std::fs::write(boot.join("boot/initrd-small"), b"x").unwrap();
    assert!(script(&["grubcfg", b]).status.success());
    let cfg = std::fs::read_to_string(boot.join("boot/grub/iyakki-side.cfg")).unwrap();
    assert!(cfg.contains("iyakki.side=1") && cfg.contains("set iyakki_initrd=/boot/initrd-small") && cfg.contains("[ -f /boot/initrd-small ]"));
    // sizes: at least 256 MiB, in steps of 64, with 25 percent spare
    let size = |o: std::process::Output| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().unwrap();
    assert_eq!(size(script(&["bootsize", b])), 256);
    let f = std::fs::File::create(boot.join("boot/big")).unwrap();
    f.set_len(300 * 1024 * 1024).unwrap();
    assert_eq!(size(script(&["bootsize", b])), 384, "300 MiB + 25 percent = 375, rounded up to a multiple of 64");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_boot_menu_reads_the_side_switch_after_the_fast_start_block_and_the_kernel_has_what_the_device_side_mounts() {
    let cfg = read("dist/buildroot/board/iyakki/grub.cfg");
    let (lean_end, side) = (cfg.find("# END lean-boot").unwrap(), cfg.find("source $prefix/iyakki-side.cfg").unwrap());
    assert!(lean_end < side, "the side switch is read after the fast-start block, so its RAM disk wins");
    assert!(cfg.contains("if [ -f $prefix/iyakki-side.cfg ]"), "and only when the file exists");
    let frag = read("dist/buildroot/board/iyakki/linux.fragment");
    for opt in ["CONFIG_SQUASHFS=y", "CONFIG_SQUASHFS_ZSTD=y", "CONFIG_OVERLAY_FS=y", "CONFIG_BLK_DEV_LOOP=y", "CONFIG_VFAT_FS=y"] {
        assert!(frag.lines().any(|l| l.trim() == opt), "linux.fragment lacks {opt}: the side image cannot be mounted without it");
    }
    // the installer sizes the copy from the image, so a bigger boot volume needs nothing there
    let apply = read("crates/iyakki-usb/src/apply.rs");
    assert!(apply.contains("boot_bytes: src.p1_sectors * 512"));
}

#[test]
fn the_image_build_calls_the_three_steps_only_when_asked_and_stops_when_the_volume_is_too_small() {
    assert!(Command::new("sh").arg("-n").arg(root().join("dist/buildroot/board/iyakki/post-image.sh")).status().unwrap().success());
    let s = read("dist/buildroot/board/iyakki/post-image.sh");
    let at = s.find("if [ \"${IYAKKI_SIDE_IMAGE:-0}\" = 1 ]; then").expect("the side image step is opt in");
    let block = &s[at..s[at..].find("\nfi\n").unwrap() + at];
    assert!(block.contains("side-image.sh\" split") && block.contains("side-image.sh\" grubcfg") && block.contains("side-image.sh\" bootsize"));
    assert!(block.contains("exit 1") && block.contains("raise it"), "a volume that is too small stops the build with the way out");
    // after the install template is staged, before the data marker: the files land in the boot volume
    assert!(s.find("cp \"$BINARIES_DIR/rootfs.cpio.zst\" \"$ROOT/boot/initrd\"").unwrap() < at && at < s.find("IYAKKI-FORMAT-ME-v1").unwrap());
}
