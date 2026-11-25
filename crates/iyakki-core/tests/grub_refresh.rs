//! `dist/buildroot/grub-refresh.sh` (Q6): the GRUB images of an install image come from the defconfig's module lists, not from the age of the kernel donor.
//! A donor built before `loadenv` was added gave every repacked image a BIOS GRUB that could not run `load_env`/`save_env`, so the update boot counter never
//! stepped. A stand-in `grub-mkimage` (it writes its own arguments into the output) shows which call the script makes; no GRUB is built here.

use std::path::{Path, PathBuf};

fn dist() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/buildroot")
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-grubrefresh-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn put(p: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// A donor tree: the GRUB build folders, a `.config` with the given EFI module list, and a stand-in grub-mkimage whose output is its argument list.
fn donor(tag: &str, own_efi_mods: &str) -> (PathBuf, PathBuf) {
    let d = tmp(tag);
    for t in ["i386-pc", "x86_64-efi"] {
        std::fs::create_dir_all(d.join(format!("build/grub2-2.12/build-{t}/grub-core"))).unwrap();
    }
    put(&d.join("build/grub2-2.12/build-i386-pc/grub-core/cdboot.img"), b"CDBOOT|");
    put(&d.join(".config"), format!("BR2_TARGET_GRUB2_BUILTIN_MODULES_EFI=\"{own_efi_mods}\"\nBR2_TARGET_GRUB2_BUILTIN_CONFIG_EFI=\"/old/efi.cfg\"\n").as_bytes());
    let mk = d.join("grub-mkimage");
    // the output file holds every argument except the output path itself, so two calls with the same inputs make the same bytes
    put(&mk, b"#!/bin/sh\nout=\"\"\nprev=\"\"\ntxt=\"\"\nfor a in \"$@\"; do if [ \"$prev\" = -o ]; then out=\"$a\"; elif [ \"$a\" != -o ]; then txt=\"$txt$a \"; fi; prev=\"$a\"; done\nprintf '%s' \"$txt\" > \"$out\"\n");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&mk, std::fs::Permissions::from_mode(0o755)).unwrap();
    (d.clone(), mk)
}

fn run(d: &Path, mk: &Path, imgs: &Path) -> (bool, String, String) {
    let o = std::process::Command::new("sh").arg(dist().join("grub-refresh.sh")).arg(d).arg(imgs).env("GRUB_MKIMAGE", mk).output().unwrap();
    (o.status.success(), String::from_utf8_lossy(&o.stdout).to_string(), String::from_utf8_lossy(&o.stderr).to_string())
}

#[test]
fn a_stale_bios_grub_is_made_again_with_loadenv_and_the_eltorito_image_follows() {
    let (d, mk) = donor("bios", "boot linux");
    let imgs = tmp("bios-imgs");
    put(&imgs.join("grub.img"), b"STALE GRUB WITHOUT THE MODULE");
    put(&imgs.join("grub-eltorito.img"), b"STALE");
    let (ok, out, err) = run(&d, &mk, &imgs);
    assert!(ok, "{out}{err}");
    let grub = std::fs::read_to_string(imgs.join("grub.img")).unwrap();
    assert!(grub.contains("-O i386-pc") && grub.contains(" loadenv") && grub.contains("-p (cd)/boot/grub"), "{grub}");
    assert!(grub.contains("grub-pc-builtin.cfg"), "the built-in menu search of the board: {grub}");
    let eltorito = std::fs::read(imgs.join("grub-eltorito.img")).unwrap();
    assert_eq!(eltorito, [b"CDBOOT|".as_slice(), grub.as_bytes()].concat(), "the CD image is the CD boot sector and the new core");
    assert!(out.contains("made again"), "{out}");
    // a second pass changes nothing and says so
    let (ok, out, err) = run(&d, &mk, &imgs);
    assert!(ok && out.contains("grub.img already has"), "{out}{err}");
}

#[test]
fn the_plain_uefi_grub_is_made_again_but_a_signed_or_shim_chained_one_is_left_alone() {
    // plain: the file equals what the donor's own (old) list makes
    let (d, mk) = donor("efi-plain", "boot linux");
    let imgs = tmp("efi-plain-imgs");
    put(&imgs.join("efi-part/EFI/BOOT/bootx64.efi"), format!("-d {}/build/grub2-2.12/build-x86_64-efi/grub-core/ -O x86_64-efi -p /EFI/BOOT -c /old/efi.cfg boot linux ", d.display()).as_bytes());
    put(&imgs.join("grub.img"), b"x");
    let (ok, out, err) = run(&d, &mk, &imgs);
    assert!(ok, "{out}{err}");
    assert!(out.contains("bootx64.efi made again"), "{out}");
    let efi = std::fs::read_to_string(imgs.join("efi-part/EFI/BOOT/bootx64.efi")).unwrap();
    assert!(efi.contains("-O x86_64-efi") && efi.contains(" loadenv") && efi.contains("-p /EFI/BOOT"), "{efi}");
    // signed: some other bytes
    let (d, mk) = donor("efi-signed", "boot linux");
    let imgs = tmp("efi-signed-imgs");
    put(&imgs.join("efi-part/EFI/BOOT/bootx64.efi"), b"SHIM CHAIN SIGNED");
    put(&imgs.join("grub.img"), b"x");
    let (ok, out, err) = run(&d, &mk, &imgs);
    assert!(ok, "{out}{err}");
    assert!(out.contains("left as it is"), "{out}");
    assert_eq!(std::fs::read(imgs.join("efi-part/EFI/BOOT/bootx64.efi")).unwrap(), b"SHIM CHAIN SIGNED");
}

#[test]
fn a_defconfig_without_loadenv_is_refused_not_used() {
    let (d, mk) = donor("nolist", "boot linux");
    let imgs = tmp("nolist-imgs");
    let cfg = tmp("nolist-cfg").join("defconfig");
    put(&cfg, b"BR2_TARGET_GRUB2_BUILTIN_MODULES_PC=\"boot linux normal\"\nBR2_TARGET_GRUB2_BUILTIN_MODULES_EFI=\"boot linux normal\"\n");
    let o = std::process::Command::new("sh").arg(dist().join("grub-refresh.sh")).arg(&d).arg(&imgs).arg(&cfg).env("GRUB_MKIMAGE", &mk).output().unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("no loadenv"), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(!imgs.join("grub.img").exists(), "nothing was written");
}

#[test]
fn repack_runs_the_refresh_on_the_staged_images_and_the_shipped_defconfig_lists_carry_loadenv() {
    let repack = std::fs::read_to_string(dist().join("repack.sh")).unwrap();
    assert!(repack.contains("grub-refresh.sh") && repack.contains("\"$STAGE/bin\""), "repack.sh must refresh the staged GRUB images");
    let at_copy = repack.find("cp -f \"$KFROM/images/$f\" \"$STAGE/bin/$f\"").expect("the donor copy");
    assert!(repack.find("grub-refresh.sh").unwrap() > at_copy, "the refresh comes after the donor's images are staged");
    let def = std::fs::read_to_string(dist().join("configs/iyakki_x86_64_defconfig")).unwrap();
    for key in ["BR2_TARGET_GRUB2_BUILTIN_MODULES_PC", "BR2_TARGET_GRUB2_BUILTIN_MODULES_EFI"] {
        let line = def.lines().find(|l| l.starts_with(key)).unwrap_or_else(|| panic!("{key}"));
        assert!(line.split(['"', ' ']).any(|w| w == "loadenv"), "{line}");
    }
}
